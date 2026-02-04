// Type inference engine

import type { Expr, LispValue } from './types';
import type { Type, TypeScheme, Constraint, Pattern, MatchBranch, DataType } from './type-ast';
import {
  typeVar,
  typeConst,
  typeArrow,
  typeApp,
  typeRec,
  tInt,
  tFloat,
  tBool,
  tString,
  tUnit,
  tFFILibrary,
  tFFIStructDef,
  tFFIStruct,
  formatType,
  freeTypeVars,
} from './type-ast';
import type { TypeEnv, Substitution } from './type-env';
import {
  extendEnv,
  childEnv,
  lookupVar,
  lookupConstructor,
  lookupDataType,
  applySubst,
  applySubstEnv,
  composeSubst,
  emptySubst,
  freshTypeVar,
  generalize,
  instantiate,
  defineDataType,
  extendEnvMany,
} from './type-env';
import { unify, unifyWithContext, unifyMany } from './type-unify';
import type { TypeResult, TypeError } from './type-errors';
import { ok, err, flatMap, sequence, map } from './type-errors';
import { resolveConstraints } from './type-builtins';

// Inference result: type, substitution, and constraints
export type InferResult = {
  type: Type;
  subst: Substitution;
  constraints: Constraint[];
};

// Convert list Expr to array of Expr
const listToArray = (expr: Expr): Expr[] => {
  const result: Expr[] = [];
  let current = expr;
  while (current.kind === 'list') {
    result.push(current.head);
    current = current.tail;
  }
  return result;
};

// Get symbol value from atom
const getSymbol = (expr: Expr): string | undefined => {
  if (expr.kind === 'atom' && expr.value.kind === 'symbol') {
    return expr.value.value;
  }
  return undefined;
};

// Check if expression is a specific symbol
const isSymbol = (expr: Expr, name: string): boolean => {
  return getSymbol(expr) === name;
};

// ===== Type Expression Parsing =====

// Parse a type expression from AST
export const parseTypeExpr = (expr: Expr, env: TypeEnv): TypeResult<Type> => {
  // Atom: type constant or type variable
  if (expr.kind === 'atom') {
    if (expr.value.kind === 'symbol') {
      const name = expr.value.value;

      // Built-in type constants
      if (name === 'Int') return ok(tInt);
      if (name === 'Float') return ok(tFloat);
      if (name === 'Bool') return ok(tBool);
      if (name === 'String') return ok(tString);
      if (name === 'Unit') return ok(tUnit);

      // Check for user-defined data type
      const dt = lookupDataType(env, name);
      if (dt) {
        if (dt.typeParams.length === 0) {
          return ok(typeConst(name));
        }
        // Data type with params used without args - it's a type constructor
        return ok(typeConst(name));
      }

      // Lowercase = type variable, uppercase = type constant
      if (name[0] && name[0] === name[0].toLowerCase()) {
        return ok(typeVar(name));
      }

      return ok(typeConst(name));
    }
    return err({ kind: 'invalid-type-annotation', message: `Invalid type expression: ${JSON.stringify(expr)}` });
  }

  // List: type application or arrow type
  if (expr.kind === 'list') {
    const elements = listToArray(expr);
    if (elements.length === 0) {
      return ok(tUnit);
    }

    const first = elements[0]!;
    const firstSym = getSymbol(first);

    // Arrow type: (-> a b c) means a -> b -> c
    if (firstSym === '->') {
      if (elements.length < 2) {
        return err({ kind: 'invalid-type-annotation', message: 'Arrow type needs at least a result type' });
      }
      const typeResults = elements.slice(1).map((e) => parseTypeExpr(e, env));
      return flatMap(sequence(typeResults), (types) => {
        if (types.length === 1) {
          return ok(typeArrow([], types[0]!));
        }
        const params = types.slice(0, -1);
        const result = types[types.length - 1]!;
        return ok(typeArrow(params, result));
      });
    }

    // Constrained type: (=> constraints type)
    if (firstSym === '=>') {
      // For now, just extract the type part (constraints handled elsewhere)
      if (elements.length < 2) {
        return err({ kind: 'invalid-type-annotation', message: 'Constrained type needs a body type' });
      }
      // Last element is the type
      return parseTypeExpr(elements[elements.length - 1]!, env);
    }

    // Recursive type: (mu a T)
    if (firstSym === 'mu') {
      if (elements.length !== 3) {
        return err({ kind: 'invalid-type-annotation', message: 'mu type needs: (mu var body)' });
      }
      const varName = getSymbol(elements[1]!);
      if (!varName) {
        return err({ kind: 'invalid-type-annotation', message: 'mu variable must be a symbol' });
      }
      return flatMap(parseTypeExpr(elements[2]!, env), (body) => ok(typeRec(varName, body)));
    }

    // Type application: (List a), (Option Int)
    return flatMap(parseTypeExpr(first, env), (constructor) => {
      const argResults = elements.slice(1).map((e) => parseTypeExpr(e, env));
      return flatMap(sequence(argResults), (args) => {
        if (args.length === 0) {
          return ok(constructor);
        }
        return ok(typeApp(constructor, args));
      });
    });
  }

  // Nil = Unit
  if (expr.kind === 'nil') {
    return ok(tUnit);
  }

  return err({ kind: 'invalid-type-annotation', message: `Invalid type expression` });
};

// Parse constraint: (Num a)
export const parseConstraint = (expr: Expr, env: TypeEnv): TypeResult<Constraint> => {
  if (expr.kind !== 'list') {
    return err({ kind: 'invalid-type-annotation', message: 'Constraint must be a list' });
  }

  const elements = listToArray(expr);
  if (elements.length !== 2) {
    return err({ kind: 'invalid-type-annotation', message: 'Constraint must be (ClassName typevar)' });
  }

  const className = getSymbol(elements[0]!);
  if (!className) {
    return err({ kind: 'invalid-type-annotation', message: 'Class name must be a symbol' });
  }

  return flatMap(parseTypeExpr(elements[1]!, env), (typeArg) => {
    return ok({ class: className, typeArg });
  });
};

// Parse type annotation with constraints: (=> (Num a) (-> a a))
export const parseTypeAnnotation = (expr: Expr, env: TypeEnv): TypeResult<{ type: Type; constraints: Constraint[] }> => {
  if (expr.kind === 'list') {
    const elements = listToArray(expr);
    const firstSym = getSymbol(elements[0] || { kind: 'nil' });

    if (firstSym === '=>') {
      // (=> constraints... type)
      if (elements.length < 3) {
        return err({ kind: 'invalid-type-annotation', message: 'Constrained type needs constraints and a body' });
      }

      const constraintExprs = elements.slice(1, -1);
      const typeExpr = elements[elements.length - 1]!;

      const constraintResults = constraintExprs.map((e) => parseConstraint(e, env));
      return flatMap(sequence(constraintResults), (constraints) => {
        return flatMap(parseTypeExpr(typeExpr, env), (type) => {
          return ok({ type, constraints });
        });
      });
    }
  }

  // No constraints
  return flatMap(parseTypeExpr(expr, env), (type) => {
    return ok({ type, constraints: [] });
  });
};

// ===== Pattern Parsing =====

export const parsePattern = (expr: Expr, env: TypeEnv): TypeResult<Pattern> => {
  // Wildcard
  if (expr.kind === 'atom' && expr.value.kind === 'symbol' && expr.value.value === '_') {
    return ok({ kind: 'pattern-wildcard' });
  }

  // Variable or constructor with no args
  if (expr.kind === 'atom' && expr.value.kind === 'symbol') {
    const name = expr.value.value;
    // Check if it's a constructor
    const ctor = lookupConstructor(env, name);
    if (ctor && ctor.constructor.params.length === 0) {
      return ok({ kind: 'pattern-constructor', name, args: [] });
    }
    // Otherwise it's a variable
    return ok({ kind: 'pattern-var', name });
  }

  // Literal patterns
  if (expr.kind === 'atom' && expr.value.kind === 'number') {
    return ok({ kind: 'pattern-literal', value: expr.value.value });
  }

  if (expr.kind === 'atom' && expr.value.kind === 'string') {
    return ok({ kind: 'pattern-literal', value: expr.value.value });
  }

  // Constructor pattern: (Some x), (Cons h t)
  if (expr.kind === 'list') {
    const elements = listToArray(expr);
    if (elements.length === 0) {
      return err({ kind: 'invalid-type-annotation', message: 'Empty pattern' });
    }

    const first = elements[0]!;
    const ctorName = getSymbol(first);
    if (!ctorName) {
      return err({ kind: 'invalid-type-annotation', message: 'Pattern constructor must be a symbol' });
    }

    const argResults = elements.slice(1).map((e) => parsePattern(e, env));
    return flatMap(sequence(argResults), (args) => {
      return ok({ kind: 'pattern-constructor', name: ctorName, args });
    });
  }

  // Nil pattern
  if (expr.kind === 'nil') {
    return ok({ kind: 'pattern-constructor', name: 'Nil', args: [] });
  }

  return err({ kind: 'invalid-type-annotation', message: 'Invalid pattern' });
};

// ===== Main Inference =====

// Infer type of an expression
export const inferExpr = (
  env: TypeEnv,
  expr: Expr
): TypeResult<InferResult> => {
  // Nil
  if (expr.kind === 'nil') {
    return ok({ type: tUnit, subst: emptySubst(), constraints: [] });
  }

  // Atom
  if (expr.kind === 'atom') {
    return inferAtom(env, expr.value);
  }

  // List expression
  const elements = listToArray(expr);
  if (elements.length === 0) {
    return ok({ type: tUnit, subst: emptySubst(), constraints: [] });
  }

  const first = elements[0]!;
  const firstSym = getSymbol(first);

  // Special forms
  if (firstSym) {
    switch (firstSym) {
      case 'define':
        return inferDefine(env, elements);
      case 'lambda':
        return inferLambda(env, elements);
      case 'if':
        return inferIf(env, elements);
      case 'let':
        return inferLet(env, elements);
      case 'begin':
        return inferBegin(env, elements);
      case 'cond':
        return inferCond(env, elements);
      case 'match':
        return inferMatch(env, elements);
      case 'data':
        return inferDataDef(env, elements);
      case 'class':
        return inferClassDef(env, elements);
      case 'instance':
        return inferInstanceDef(env, elements);
      case 'type':
        return inferTypeAlias(env, elements);
    }
  }

  // Function application
  return inferApplication(env, elements);
};

// Infer type of atom
const inferAtom = (env: TypeEnv, value: LispValue): TypeResult<InferResult> => {
  switch (value.kind) {
    case 'number': {
      // Use the isFloat flag from the lexer to determine the type
      const type = value.isFloat ? tFloat : tInt;
      return ok({ type, subst: emptySubst(), constraints: [] });
    }
    case 'string':
      return ok({ type: tString, subst: emptySubst(), constraints: [] });
    case 'symbol': {
      // Boolean literals
      if (value.value === '#t' || value.value === '#f') {
        return ok({ type: tBool, subst: emptySubst(), constraints: [] });
      }

      // Variable lookup
      const scheme = lookupVar(env, value.value);
      if (!scheme) {
        return err({ kind: 'undefined-variable', name: value.value });
      }

      // Instantiate the type scheme
      const { type, constraints } = instantiate(scheme);
      return ok({ type, subst: emptySubst(), constraints });
    }
  }
};

// Infer type of define expression
const inferDefine = (env: TypeEnv, elements: Expr[]): TypeResult<InferResult> => {
  if (elements.length < 3) {
    return err({ kind: 'arity-mismatch', expected: 2, got: elements.length - 1, context: 'define' });
  }

  const nameExpr = elements[1]!;
  const valueExpr = elements[2]!;

  // Check for type annotation: (define (name : Type) value)
  if (nameExpr.kind === 'list') {
    const nameElements = listToArray(nameExpr);
    if (nameElements.length === 3 && isSymbol(nameElements[1]!, ':')) {
      const name = getSymbol(nameElements[0]!);
      if (!name) {
        return err({ kind: 'invalid-type-annotation', message: 'Expected symbol before :' });
      }

      // Parse the type annotation
      const typeExpr = nameElements[2]!;
      return flatMap(parseTypeAnnotation(typeExpr, env), ({ type: annotatedType, constraints: annotatedConstraints }) => {
        // Infer the value type
        return flatMap(inferExpr(env, valueExpr), (valueResult) => {
          // Unify annotated type with inferred type
          return flatMap(unifyWithContext(annotatedType, valueResult.type, `definition of ${name}`), (unifySubst) => {
            const finalSubst = composeSubst(unifySubst, valueResult.subst);
            const finalType = applySubst(finalSubst, annotatedType);
            const allConstraints = [...annotatedConstraints, ...valueResult.constraints];

            return ok({
              type: tUnit,
              subst: finalSubst,
              constraints: resolveConstraints(allConstraints),
            });
          });
        });
      });
    }
  }

  // Regular define without annotation
  const name = getSymbol(nameExpr);
  if (!name) {
    return err({ kind: 'invalid-type-annotation', message: 'define expects a symbol name' });
  }

  return flatMap(inferExpr(env, valueExpr), (result) => {
    return ok({
      type: tUnit,
      subst: result.subst,
      constraints: result.constraints,
    });
  });
};

// Infer type of lambda expression
const inferLambda = (env: TypeEnv, elements: Expr[]): TypeResult<InferResult> => {
  if (elements.length < 3) {
    return err({ kind: 'arity-mismatch', expected: 2, got: elements.length - 1, context: 'lambda' });
  }

  const paramsExpr = elements[1]!;
  const bodyExprs = elements.slice(2);

  // Parse parameters (may have type annotations)
  const paramList = listToArray(paramsExpr);
  const params: { name: string; type: Type }[] = [];
  let paramEnv = childEnv(env);

  for (const param of paramList) {
    // Check for annotated param: (x : Int)
    if (param.kind === 'list') {
      const paramElements = listToArray(param);
      if (paramElements.length === 3 && isSymbol(paramElements[1]!, ':')) {
        const name = getSymbol(paramElements[0]!);
        if (!name) {
          return err({ kind: 'invalid-type-annotation', message: 'Parameter name must be a symbol' });
        }
        const typeResult = parseTypeExpr(paramElements[2]!, env);
        if (!typeResult.ok) return typeResult;
        params.push({ name, type: typeResult.value });
        paramEnv = extendEnv(paramEnv, name, { quantifiedVars: [], qualType: { constraints: [], type: typeResult.value } });
        continue;
      }
    }

    // Unannotated param
    const name = getSymbol(param);
    if (!name) {
      return err({ kind: 'invalid-type-annotation', message: 'Parameter must be a symbol' });
    }
    const paramType = freshTypeVar();
    params.push({ name, type: paramType });
    paramEnv = extendEnv(paramEnv, name, { quantifiedVars: [], qualType: { constraints: [], type: paramType } });
  }

  // Infer body type
  let bodySubst = emptySubst();
  let bodyType: Type = tUnit;
  let allConstraints: Constraint[] = [];

  for (let i = 0; i < bodyExprs.length; i++) {
    const bodyExpr = bodyExprs[i]!;
    const result = inferExpr(applySubstEnv(bodySubst, paramEnv), bodyExpr);
    if (!result.ok) return result;

    bodySubst = composeSubst(result.value.subst, bodySubst);
    allConstraints = [...allConstraints, ...result.value.constraints];

    if (i === bodyExprs.length - 1) {
      bodyType = result.value.type;
    }
  }

  // Build function type
  const paramTypes = params.map((p) => applySubst(bodySubst, p.type));
  const resultType = applySubst(bodySubst, bodyType);
  const fnType = typeArrow(paramTypes, resultType);

  return ok({
    type: fnType,
    subst: bodySubst,
    constraints: allConstraints,
  });
};

// Infer type of if expression
const inferIf = (env: TypeEnv, elements: Expr[]): TypeResult<InferResult> => {
  if (elements.length < 3 || elements.length > 4) {
    return err({ kind: 'arity-mismatch', expected: 3, got: elements.length - 1, context: 'if' });
  }

  const condExpr = elements[1]!;
  const thenExpr = elements[2]!;
  const elseExpr = elements[3];

  // Infer condition type
  return flatMap(inferExpr(env, condExpr), (condResult) => {
    // Condition must be Bool
    return flatMap(unifyWithContext(condResult.type, tBool, 'if condition'), (condUnify) => {
      const s1 = composeSubst(condUnify, condResult.subst);
      const env1 = applySubstEnv(s1, env);

      // Infer then branch
      return flatMap(inferExpr(env1, thenExpr), (thenResult) => {
        const s2 = composeSubst(thenResult.subst, s1);

        if (elseExpr) {
          // Infer else branch
          const env2 = applySubstEnv(s2, env);
          return flatMap(inferExpr(env2, elseExpr), (elseResult) => {
            const s3 = composeSubst(elseResult.subst, s2);

            // Then and else must have same type
            const thenType = applySubst(s3, thenResult.type);
            const elseType = applySubst(s3, elseResult.type);

            return flatMap(unifyWithContext(thenType, elseType, 'if branches'), (branchUnify) => {
              const finalSubst = composeSubst(branchUnify, s3);
              return ok({
                type: applySubst(finalSubst, thenType),
                subst: finalSubst,
                constraints: [...condResult.constraints, ...thenResult.constraints, ...elseResult.constraints],
              });
            });
          });
        } else {
          // No else branch - result is Unit
          return ok({
            type: tUnit,
            subst: s2,
            constraints: [...condResult.constraints, ...thenResult.constraints],
          });
        }
      });
    });
  });
};

// Infer type of let expression
const inferLet = (env: TypeEnv, elements: Expr[]): TypeResult<InferResult> => {
  if (elements.length < 3) {
    return err({ kind: 'arity-mismatch', expected: 2, got: elements.length - 1, context: 'let' });
  }

  const bindingsExpr = elements[1]!;
  const bodyExprs = elements.slice(2);

  const bindings = listToArray(bindingsExpr);
  let letEnv = childEnv(env);
  let subst = emptySubst();
  let allConstraints: Constraint[] = [];

  for (const binding of bindings) {
    const pair = listToArray(binding);
    if (pair.length !== 2) {
      return err({ kind: 'invalid-type-annotation', message: 'let binding must be (name value)' });
    }

    const nameExpr = pair[0]!;
    const valueExpr = pair[1]!;

    // Check for type annotation
    let name: string;
    let annotatedType: Type | undefined;

    if (nameExpr.kind === 'list') {
      const nameElements = listToArray(nameExpr);
      if (nameElements.length === 3 && isSymbol(nameElements[1]!, ':')) {
        const n = getSymbol(nameElements[0]!);
        if (!n) {
          return err({ kind: 'invalid-type-annotation', message: 'Expected symbol before :' });
        }
        name = n;
        const typeResult = parseTypeExpr(nameElements[2]!, env);
        if (!typeResult.ok) return typeResult;
        annotatedType = typeResult.value;
      } else {
        return err({ kind: 'invalid-type-annotation', message: 'Invalid let binding name' });
      }
    } else {
      const n = getSymbol(nameExpr);
      if (!n) {
        return err({ kind: 'invalid-type-annotation', message: 'let binding name must be a symbol' });
      }
      name = n;
    }

    // Infer value type
    const valueResult = inferExpr(applySubstEnv(subst, letEnv), valueExpr);
    if (!valueResult.ok) return valueResult;

    subst = composeSubst(valueResult.value.subst, subst);
    allConstraints = [...allConstraints, ...valueResult.value.constraints];

    let bindingType = valueResult.value.type;

    // Unify with annotation if present
    if (annotatedType) {
      const unifyResult = unifyWithContext(annotatedType, bindingType, `let binding ${name}`);
      if (!unifyResult.ok) return unifyResult;
      subst = composeSubst(unifyResult.value, subst);
      bindingType = applySubst(subst, annotatedType);
    }

    // Generalize and add to environment
    const scheme = generalize(applySubstEnv(subst, letEnv), allConstraints, applySubst(subst, bindingType));
    letEnv = extendEnv(letEnv, name, scheme);
  }

  // Infer body type
  let bodyType: Type = tUnit;
  for (let i = 0; i < bodyExprs.length; i++) {
    const bodyExpr = bodyExprs[i]!;
    const result = inferExpr(applySubstEnv(subst, letEnv), bodyExpr);
    if (!result.ok) return result;

    subst = composeSubst(result.value.subst, subst);
    allConstraints = [...allConstraints, ...result.value.constraints];

    if (i === bodyExprs.length - 1) {
      bodyType = result.value.type;
    }
  }

  return ok({
    type: applySubst(subst, bodyType),
    subst,
    constraints: allConstraints,
  });
};

// Infer type of begin expression
const inferBegin = (env: TypeEnv, elements: Expr[]): TypeResult<InferResult> => {
  const bodyExprs = elements.slice(1);

  if (bodyExprs.length === 0) {
    return ok({ type: tUnit, subst: emptySubst(), constraints: [] });
  }

  let subst = emptySubst();
  let lastType: Type = tUnit;
  let allConstraints: Constraint[] = [];

  for (const expr of bodyExprs) {
    const result = inferExpr(applySubstEnv(subst, env), expr);
    if (!result.ok) return result;

    subst = composeSubst(result.value.subst, subst);
    allConstraints = [...allConstraints, ...result.value.constraints];
    lastType = result.value.type;
  }

  return ok({
    type: applySubst(subst, lastType),
    subst,
    constraints: allConstraints,
  });
};

// Infer type of cond expression
const inferCond = (env: TypeEnv, elements: Expr[]): TypeResult<InferResult> => {
  const clauses = elements.slice(1);

  if (clauses.length === 0) {
    return ok({ type: tUnit, subst: emptySubst(), constraints: [] });
  }

  let subst = emptySubst();
  let resultType: Type | undefined;
  let allConstraints: Constraint[] = [];

  for (const clauseExpr of clauses) {
    const clause = listToArray(clauseExpr);
    if (clause.length !== 2) {
      return err({ kind: 'invalid-type-annotation', message: 'cond clause must be (test result)' });
    }

    const testExpr = clause[0]!;
    const resultExpr = clause[1]!;

    // Check for else clause
    if (!isSymbol(testExpr, 'else')) {
      // Infer test type (must be Bool)
      const testResult = inferExpr(applySubstEnv(subst, env), testExpr);
      if (!testResult.ok) return testResult;

      const testUnify = unifyWithContext(testResult.value.type, tBool, 'cond test');
      if (!testUnify.ok) return testUnify;

      subst = composeSubst(testUnify.value, composeSubst(testResult.value.subst, subst));
      allConstraints = [...allConstraints, ...testResult.value.constraints];
    }

    // Infer result type
    const exprResult = inferExpr(applySubstEnv(subst, env), resultExpr);
    if (!exprResult.ok) return exprResult;

    subst = composeSubst(exprResult.value.subst, subst);
    allConstraints = [...allConstraints, ...exprResult.value.constraints];

    const clauseType = applySubst(subst, exprResult.value.type);

    if (resultType === undefined) {
      resultType = clauseType;
    } else {
      // All clauses must have same type
      const branchUnify = unifyWithContext(resultType, clauseType, 'cond branches');
      if (!branchUnify.ok) return branchUnify;
      subst = composeSubst(branchUnify.value, subst);
      resultType = applySubst(subst, resultType);
    }
  }

  return ok({
    type: resultType || tUnit,
    subst,
    constraints: allConstraints,
  });
};

// Get the DataType from a Type (handles type-const and type-app)
const getDataTypeFromType = (env: TypeEnv, t: Type): DataType | undefined => {
  if (t.kind === 'type-const') {
    return lookupDataType(env, t.name);
  }
  if (t.kind === 'type-app' && t.constructor.kind === 'type-const') {
    return lookupDataType(env, t.constructor.name);
  }
  return undefined;
};

// Check if a pattern covers all constructors (wildcard or variable)
const patternCoversAll = (pattern: Pattern): boolean => {
  return pattern.kind === 'pattern-wildcard' || pattern.kind === 'pattern-var';
};

// Get the constructor name from a pattern (if it's a constructor pattern)
const getPatternConstructor = (pattern: Pattern): string | undefined => {
  if (pattern.kind === 'pattern-constructor') {
    return pattern.name;
  }
  return undefined;
};

// Check exhaustiveness of match patterns
const checkExhaustiveness = (
  env: TypeEnv,
  scrutineeType: Type,
  patterns: Pattern[]
): TypeResult<void> => {
  // Get the DataType for the scrutinee
  const dataType = getDataTypeFromType(env, scrutineeType);

  // If not an ADT (e.g., Int, String, type variable), skip exhaustiveness check
  if (!dataType) {
    return ok(undefined);
  }

  // Check if any pattern covers all cases (wildcard or variable)
  for (const pattern of patterns) {
    if (patternCoversAll(pattern)) {
      return ok(undefined);
    }
  }

  // Collect all covered constructor names
  const coveredConstructors = new Set<string>();
  for (const pattern of patterns) {
    const ctorName = getPatternConstructor(pattern);
    if (ctorName) {
      coveredConstructors.add(ctorName);
    }
  }

  // Find missing constructors
  const missingConstructors: string[] = [];
  for (const ctor of dataType.constructors) {
    if (!coveredConstructors.has(ctor.name)) {
      missingConstructors.push(ctor.name);
    }
  }

  if (missingConstructors.length > 0) {
    return err({ kind: 'non-exhaustive-patterns', missing: missingConstructors });
  }

  return ok(undefined);
};

// Infer type of match expression
const inferMatch = (env: TypeEnv, elements: Expr[]): TypeResult<InferResult> => {
  if (elements.length < 3) {
    return err({ kind: 'arity-mismatch', expected: 2, got: elements.length - 1, context: 'match' });
  }

  const scrutineeExpr = elements[1]!;
  const branches = elements.slice(2);

  // Infer scrutinee type
  return flatMap(inferExpr(env, scrutineeExpr), (scrutineeResult) => {
    let subst = scrutineeResult.subst;
    let scrutineeType = scrutineeResult.type;
    let resultType: Type | undefined;
    let allConstraints = [...scrutineeResult.constraints];
    const parsedPatterns: Pattern[] = [];

    for (const branchExpr of branches) {
      const branchElements = listToArray(branchExpr);
      if (branchElements.length !== 2) {
        return err({ kind: 'invalid-type-annotation', message: 'match branch must be (pattern body)' });
      }

      const patternExpr = branchElements[0]!;
      const bodyExpr = branchElements[1]!;

      // Parse pattern
      const patternResult = parsePattern(patternExpr, env);
      if (!patternResult.ok) return patternResult;

      // Collect parsed patterns for exhaustiveness check
      parsedPatterns.push(patternResult.value);

      // Type check pattern and get bindings
      const patternCheck = inferPattern(env, patternResult.value, applySubst(subst, scrutineeType));
      if (!patternCheck.ok) return patternCheck;

      const patternUnify = unifyWithContext(
        applySubst(subst, scrutineeType),
        patternCheck.value.type,
        'match pattern'
      );
      if (!patternUnify.ok) return patternUnify;

      subst = composeSubst(patternUnify.value, composeSubst(patternCheck.value.subst, subst));
      scrutineeType = applySubst(subst, scrutineeType);

      // Extend environment with pattern bindings
      let branchEnv = applySubstEnv(subst, env);
      for (const [name, type] of patternCheck.value.bindings) {
        const scheme = { quantifiedVars: [], qualType: { constraints: [], type: applySubst(subst, type) } };
        branchEnv = extendEnv(branchEnv, name, scheme);
      }

      // Infer body type
      const bodyResult = inferExpr(branchEnv, bodyExpr);
      if (!bodyResult.ok) return bodyResult;

      subst = composeSubst(bodyResult.value.subst, subst);
      allConstraints = [...allConstraints, ...bodyResult.value.constraints];

      const branchType = applySubst(subst, bodyResult.value.type);

      if (resultType === undefined) {
        resultType = branchType;
      } else {
        // All branches must have same type
        const branchUnify = unifyWithContext(resultType, branchType, 'match branches');
        if (!branchUnify.ok) return branchUnify;
        subst = composeSubst(branchUnify.value, subst);
        resultType = applySubst(subst, resultType);
      }
    }

    // Check exhaustiveness with final scrutinee type
    const exhaustivenessCheck = checkExhaustiveness(env, applySubst(subst, scrutineeType), parsedPatterns);
    if (!exhaustivenessCheck.ok) return exhaustivenessCheck;

    return ok({
      type: resultType || tUnit,
      subst,
      constraints: allConstraints,
    });
  });
};

// Infer type of pattern and get variable bindings
const inferPattern = (
  env: TypeEnv,
  pattern: Pattern,
  expectedType: Type
): TypeResult<{ type: Type; subst: Substitution; bindings: Map<string, Type> }> => {
  switch (pattern.kind) {
    case 'pattern-wildcard':
      return ok({ type: expectedType, subst: emptySubst(), bindings: new Map() });

    case 'pattern-var':
      return ok({ type: expectedType, subst: emptySubst(), bindings: new Map([[pattern.name, expectedType]]) });

    case 'pattern-literal': {
      let literalType: Type;
      if (typeof pattern.value === 'number') {
        literalType = Number.isInteger(pattern.value) ? tInt : tFloat;
      } else if (typeof pattern.value === 'string') {
        literalType = tString;
      } else {
        literalType = tBool;
      }

      return flatMap(unify(expectedType, literalType), (subst) => {
        return ok({ type: literalType, subst, bindings: new Map() });
      });
    }

    case 'pattern-constructor': {
      const ctorInfo = lookupConstructor(env, pattern.name);
      if (!ctorInfo) {
        return err({ kind: 'undefined-constructor', name: pattern.name });
      }

      const { dataType, constructor } = ctorInfo;

      if (pattern.args.length !== constructor.params.length) {
        return err({
          kind: 'arity-mismatch',
          expected: constructor.params.length,
          got: pattern.args.length,
          context: `constructor ${pattern.name}`,
        });
      }

      // Instantiate constructor type with fresh variables for data type params
      const typeVarMap = new Map<string, Type>();
      for (const tv of dataType.typeParams) {
        typeVarMap.set(tv, freshTypeVar());
      }

      // For data types with no parameters, use typeConst; otherwise use typeApp
      const instantiatedResult = dataType.typeParams.length === 0
        ? typeConst(dataType.name)
        : typeApp(
            typeConst(dataType.name),
            dataType.typeParams.map((tv) => typeVarMap.get(tv)!)
          );

      // Unify with expected type
      return flatMap(unify(expectedType, instantiatedResult), (s1) => {
        let subst = s1;
        const allBindings = new Map<string, Type>();

        for (let i = 0; i < pattern.args.length; i++) {
          let paramType = constructor.params[i]!;
          // Apply type variable substitution
          for (const [tv, replacement] of typeVarMap) {
            paramType = applySubst(new Map([[tv, replacement]]), paramType);
          }
          paramType = applySubst(subst, paramType);

          const argResult = inferPattern(env, pattern.args[i]!, paramType);
          if (!argResult.ok) return argResult;

          subst = composeSubst(argResult.value.subst, subst);
          for (const [name, type] of argResult.value.bindings) {
            allBindings.set(name, type);
          }
        }

        return ok({
          type: applySubst(subst, instantiatedResult),
          subst,
          bindings: allBindings,
        });
      });
    }
  }
};

// Set of FFI functions that are variadic and bypass strict arity checking
const FFI_VARIADIC_FUNCTIONS = new Set(['ffi-call', 'struct-new', 'struct-get', 'struct-set', 'list']);

// Variadic arithmetic operators with their minimum argument requirements
const VARIADIC_ARITH_OPS = new Map<string, { minArgs: number }>([
  ['+', { minArgs: 0 }],  // (+) returns 0
  ['*', { minArgs: 0 }],  // (*) returns 1
  ['-', { minArgs: 1 }],  // (- x) = -x, requires at least 1 arg
  ['/', { minArgs: 2 }],  // division requires at least 2 args
]);

// Infer type for variadic arithmetic operators (+, -, *, /)
const inferVariadicArithApplication = (
  env: TypeEnv,
  opName: string,
  argExprs: Expr[]
): TypeResult<InferResult> => {
  const opInfo = VARIADIC_ARITH_OPS.get(opName)!;

  // Check minimum arity
  if (argExprs.length < opInfo.minArgs) {
    return err({
      kind: 'arity-mismatch',
      expected: opInfo.minArgs,
      got: argExprs.length,
      context: opName,
    });
  }

  // Handle 0-arg case for + and *
  if (argExprs.length === 0) {
    return ok({ type: tInt, subst: emptySubst(), constraints: [] });
  }

  let subst = emptySubst();
  let allConstraints: Constraint[] = [];
  const argTypes: Type[] = [];

  // Infer all argument types
  for (const argExpr of argExprs) {
    const argResult = inferExpr(applySubstEnv(subst, env), argExpr);
    if (!argResult.ok) return argResult;
    subst = composeSubst(argResult.value.subst, subst);
    allConstraints = [...allConstraints, ...argResult.value.constraints];
    argTypes.push(argResult.value.type);
  }

  // Unify all argument types together
  let unifiedType = applySubst(subst, argTypes[0]!);
  for (let i = 1; i < argTypes.length; i++) {
    const argType = applySubst(subst, argTypes[i]!);
    const unifyResult = unifyWithContext(unifiedType, argType, `${opName} argument ${i + 1}`);
    if (!unifyResult.ok) return unifyResult;
    subst = composeSubst(unifyResult.value, subst);
    unifiedType = applySubst(subst, unifiedType);
  }

  // Add Num constraint
  allConstraints.push({ class: 'Num', typeArg: unifiedType });

  return ok({
    type: unifiedType,
    subst,
    constraints: allConstraints,
  });
};

// Infer type of function application
const inferApplication = (env: TypeEnv, elements: Expr[]): TypeResult<InferResult> => {
  const fnExpr = elements[0]!;
  const argExprs = elements.slice(1);

  // Check for variadic arithmetic operators
  const fnName = getSymbol(fnExpr);
  if (fnName && VARIADIC_ARITH_OPS.has(fnName)) {
    return inferVariadicArithApplication(env, fnName, argExprs);
  }

  // Check for variadic FFI functions that bypass normal type checking
  if (fnName && FFI_VARIADIC_FUNCTIONS.has(fnName)) {
    return inferFFIApplication(env, fnName, argExprs);
  }

  // Infer function type
  return flatMap(inferExpr(env, fnExpr), (fnResult) => {
    let subst = fnResult.subst;
    let allConstraints = [...fnResult.constraints];
    const argTypes: Type[] = [];

    // Infer argument types
    for (const argExpr of argExprs) {
      const argResult = inferExpr(applySubstEnv(subst, env), argExpr);
      if (!argResult.ok) return argResult;

      subst = composeSubst(argResult.value.subst, subst);
      allConstraints = [...allConstraints, ...argResult.value.constraints];
      argTypes.push(argResult.value.type);
    }

    // Create fresh result type variable
    const resultType = freshTypeVar();

    // Build expected function type
    const expectedFnType = typeArrow(argTypes.map((t) => applySubst(subst, t)), resultType);

    // Unify with actual function type
    const fnType = applySubst(subst, fnResult.type);
    return flatMap(unifyWithContext(fnType, expectedFnType, 'function application'), (appSubst) => {
      const finalSubst = composeSubst(appSubst, subst);
      return ok({
        type: applySubst(finalSubst, resultType),
        subst: finalSubst,
        constraints: allConstraints,
      });
    });
  });
};

// Infer type for variadic FFI functions
// These functions bypass normal arity checking since FFI is inherently unsafe
const inferFFIApplication = (
  env: TypeEnv,
  fnName: string,
  argExprs: Expr[]
): TypeResult<InferResult> => {
  let subst = emptySubst();
  let allConstraints: Constraint[] = [];

  // Type check all arguments (for side effects and to catch obvious errors)
  for (const argExpr of argExprs) {
    const argResult = inferExpr(applySubstEnv(subst, env), argExpr);
    if (!argResult.ok) return argResult;
    subst = composeSubst(argResult.value.subst, subst);
    allConstraints = [...allConstraints, ...argResult.value.constraints];
  }

  // Return appropriate type based on the FFI function
  let resultType: Type;
  switch (fnName) {
    case 'ffi-call':
      // ffi-call returns a polymorphic type (caller knows the actual type)
      resultType = freshTypeVar();
      break;
    case 'struct-new':
      // struct-new returns FFIStruct
      resultType = tFFIStruct;
      break;
    case 'struct-get':
      // struct-get returns a polymorphic type
      resultType = freshTypeVar();
      break;
    case 'struct-set':
      // struct-set returns Unit
      resultType = tUnit;
      break;
    case 'list':
      // list returns List of the element type
      resultType = typeApp(typeConst('List'), [freshTypeVar()]);
      break;
    default:
      resultType = freshTypeVar();
  }

  return ok({
    type: resultType,
    subst,
    constraints: allConstraints,
  });
};

// Infer data type definition
const inferDataDef = (env: TypeEnv, elements: Expr[]): TypeResult<InferResult> => {
  if (elements.length < 3) {
    return err({ kind: 'arity-mismatch', expected: 2, got: elements.length - 1, context: 'data' });
  }

  // Parse: (data (TypeName params...) constructor...)
  const typeExpr = elements[1]!;
  const ctorExprs = elements.slice(2);

  // Parse type name and parameters
  let typeName: string;
  let typeParams: string[] = [];

  if (typeExpr.kind === 'list') {
    const typeElements = listToArray(typeExpr);
    const name = getSymbol(typeElements[0]!);
    if (!name) {
      return err({ kind: 'invalid-type-annotation', message: 'data type name must be a symbol' });
    }
    typeName = name;
    for (let i = 1; i < typeElements.length; i++) {
      const param = getSymbol(typeElements[i]!);
      if (!param) {
        return err({ kind: 'invalid-type-annotation', message: 'type parameter must be a symbol' });
      }
      typeParams.push(param);
    }
  } else {
    const name = getSymbol(typeExpr);
    if (!name) {
      return err({ kind: 'invalid-type-annotation', message: 'data type name must be a symbol' });
    }
    typeName = name;
  }

  // Build result type
  const resultType = typeParams.length > 0
    ? typeApp(typeConst(typeName), typeParams.map(typeVar))
    : typeConst(typeName);

  // Parse constructors
  const constructors: Array<{ name: string; params: Type[]; resultType: Type }> = [];

  for (const ctorExpr of ctorExprs) {
    if (ctorExpr.kind === 'atom') {
      // Nullary constructor
      const ctorName = getSymbol(ctorExpr);
      if (!ctorName) {
        return err({ kind: 'invalid-type-annotation', message: 'constructor name must be a symbol' });
      }
      constructors.push({ name: ctorName, params: [], resultType });
    } else if (ctorExpr.kind === 'list') {
      // Constructor with parameters
      const ctorElements = listToArray(ctorExpr);
      const ctorName = getSymbol(ctorElements[0]!);
      if (!ctorName) {
        return err({ kind: 'invalid-type-annotation', message: 'constructor name must be a symbol' });
      }

      const params: Type[] = [];
      for (let i = 1; i < ctorElements.length; i++) {
        const paramTypeResult = parseTypeExpr(ctorElements[i]!, env);
        if (!paramTypeResult.ok) return paramTypeResult;
        params.push(paramTypeResult.value);
      }

      constructors.push({ name: ctorName, params, resultType });
    }
  }

  // This is a declaration, actual environment update happens in type checker
  return ok({
    type: tUnit,
    subst: emptySubst(),
    constraints: [],
  });
};

// Infer typeclass definition
const inferClassDef = (env: TypeEnv, elements: Expr[]): TypeResult<InferResult> => {
  // (class (ClassName a) method-signatures...)
  // This is just validated, not inferred
  return ok({
    type: tUnit,
    subst: emptySubst(),
    constraints: [],
  });
};

// Infer instance definition
const inferInstanceDef = (env: TypeEnv, elements: Expr[]): TypeResult<InferResult> => {
  // (instance (ClassName Type) method-implementations...)
  // This is just validated, not inferred
  return ok({
    type: tUnit,
    subst: emptySubst(),
    constraints: [],
  });
};

// Infer type alias
const inferTypeAlias = (env: TypeEnv, elements: Expr[]): TypeResult<InferResult> => {
  // (type AliasName Type)
  return ok({
    type: tUnit,
    subst: emptySubst(),
    constraints: [],
  });
};

// ===== Main Entry Point =====

// Infer types for a list of expressions
export const inferProgram = (
  env: TypeEnv,
  exprs: Expr[]
): TypeResult<{ env: TypeEnv; types: Type[] }> => {
  let currentEnv = env;
  const types: Type[] = [];
  let globalSubst = emptySubst();

  for (const expr of exprs) {
    // Handle data definitions specially - they extend the environment
    if (expr.kind === 'list') {
      const elements = listToArray(expr);
      if (elements.length > 0 && isSymbol(elements[0]!, 'data')) {
        const dataResult = processDataDef(currentEnv, elements);
        if (!dataResult.ok) return dataResult;
        currentEnv = dataResult.value;
        types.push(tUnit);
        continue;
      }

      // Handle define specially - they extend the environment
      if (elements.length >= 3 && isSymbol(elements[0]!, 'define')) {
        const defineResult = processDefine(currentEnv, elements, globalSubst);
        if (!defineResult.ok) return defineResult;
        currentEnv = defineResult.value.env;
        globalSubst = defineResult.value.subst;
        types.push(tUnit);
        continue;
      }
    }

    // Regular expression inference
    const result = inferExpr(applySubstEnv(globalSubst, currentEnv), expr);
    if (!result.ok) return result;

    globalSubst = composeSubst(result.value.subst, globalSubst);
    types.push(applySubst(globalSubst, result.value.type));

    // Check constraints
    const resolvedConstraints = resolveConstraints(result.value.constraints);
    for (const c of resolvedConstraints) {
      // If constraint is on a concrete type with no instance, error
      if (c.typeArg.kind !== 'type-var') {
        return err({ kind: 'no-instance', constraint: c });
      }
    }
  }

  return ok({ env: currentEnv, types });
};

// Process data definition and update environment
const processDataDef = (env: TypeEnv, elements: Expr[]): TypeResult<TypeEnv> => {
  const typeExpr = elements[1]!;
  const ctorExprs = elements.slice(2);

  // Parse type name and parameters
  let typeName: string;
  let typeParams: string[] = [];

  if (typeExpr.kind === 'list') {
    const typeElements = listToArray(typeExpr);
    const name = getSymbol(typeElements[0]!);
    if (!name) {
      return err({ kind: 'invalid-type-annotation', message: 'data type name must be a symbol' });
    }
    typeName = name;
    for (let i = 1; i < typeElements.length; i++) {
      const param = getSymbol(typeElements[i]!);
      if (!param) {
        return err({ kind: 'invalid-type-annotation', message: 'type parameter must be a symbol' });
      }
      typeParams.push(param);
    }
  } else {
    const name = getSymbol(typeExpr);
    if (!name) {
      return err({ kind: 'invalid-type-annotation', message: 'data type name must be a symbol' });
    }
    typeName = name;
  }

  // Build result type
  const resultType = typeParams.length > 0
    ? typeApp(typeConst(typeName), typeParams.map(typeVar))
    : typeConst(typeName);

  // Parse constructors
  const constructors: Array<{ name: string; params: Type[]; resultType: Type }> = [];

  for (const ctorExpr of ctorExprs) {
    if (ctorExpr.kind === 'atom') {
      const ctorName = getSymbol(ctorExpr);
      if (!ctorName) {
        return err({ kind: 'invalid-type-annotation', message: 'constructor name must be a symbol' });
      }
      constructors.push({ name: ctorName, params: [], resultType });
    } else if (ctorExpr.kind === 'list') {
      const ctorElements = listToArray(ctorExpr);
      const ctorName = getSymbol(ctorElements[0]!);
      if (!ctorName) {
        return err({ kind: 'invalid-type-annotation', message: 'constructor name must be a symbol' });
      }

      const params: Type[] = [];
      for (let i = 1; i < ctorElements.length; i++) {
        const paramTypeResult = parseTypeExpr(ctorElements[i]!, env);
        if (!paramTypeResult.ok) return paramTypeResult;
        params.push(paramTypeResult.value);
      }

      constructors.push({ name: ctorName, params, resultType });
    }
  }

  // Create data type definition
  const dataType: DataType = { name: typeName, typeParams, constructors };

  // Update environment with data type
  let newEnv = defineDataType(env, dataType);

  // Add constructor type schemes
  for (const ctor of constructors) {
    let ctorType: Type;
    if (ctor.params.length === 0) {
      ctorType = ctor.resultType;
    } else {
      ctorType = typeArrow(ctor.params, ctor.resultType);
    }

    const scheme: TypeScheme = {
      quantifiedVars: typeParams,
      qualType: { constraints: [], type: ctorType },
    };

    newEnv = extendEnv(newEnv, ctor.name, scheme);
  }

  return ok(newEnv);
};

// Process define and update environment
const processDefine = (
  env: TypeEnv,
  elements: Expr[],
  currentSubst: Substitution
): TypeResult<{ env: TypeEnv; subst: Substitution }> => {
  const nameExpr = elements[1]!;
  const valueExpr = elements[2]!;

  let name: string;
  let annotatedType: Type | undefined;
  let annotatedConstraints: Constraint[] = [];

  // Check for type annotation
  if (nameExpr.kind === 'list') {
    const nameElements = listToArray(nameExpr);
    if (nameElements.length === 3 && isSymbol(nameElements[1]!, ':')) {
      const n = getSymbol(nameElements[0]!);
      if (!n) {
        return err({ kind: 'invalid-type-annotation', message: 'Expected symbol before :' });
      }
      name = n;

      const typeResult = parseTypeAnnotation(nameElements[2]!, env);
      if (!typeResult.ok) return typeResult;
      annotatedType = typeResult.value.type;
      annotatedConstraints = typeResult.value.constraints;
    } else {
      return err({ kind: 'invalid-type-annotation', message: 'Invalid define syntax' });
    }
  } else {
    const n = getSymbol(nameExpr);
    if (!n) {
      return err({ kind: 'invalid-type-annotation', message: 'define expects a symbol name' });
    }
    name = n;
  }

  // For recursive functions, add a placeholder with fresh type var
  let inferEnv = env;
  const placeholderType = annotatedType || freshTypeVar();
  inferEnv = extendEnv(inferEnv, name, {
    quantifiedVars: [],
    qualType: { constraints: annotatedConstraints, type: placeholderType },
  });

  // Infer value type
  const valueResult = inferExpr(applySubstEnv(currentSubst, inferEnv), valueExpr);
  if (!valueResult.ok) return valueResult;

  let subst = composeSubst(valueResult.value.subst, currentSubst);
  let valueType = valueResult.value.type;

  // Unify with annotation if present
  if (annotatedType) {
    const unifyResult = unifyWithContext(annotatedType, valueType, `definition of ${name}`);
    if (!unifyResult.ok) return unifyResult;
    subst = composeSubst(unifyResult.value, subst);
    valueType = applySubst(subst, annotatedType);
  }

  // Generalize the type
  const allConstraints = [...annotatedConstraints, ...valueResult.value.constraints];
  const scheme = generalize(applySubstEnv(subst, env), allConstraints, applySubst(subst, valueType));

  // Update environment
  const newEnv = extendEnv(env, name, scheme);

  return ok({ env: newEnv, subst });
};
