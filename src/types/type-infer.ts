// Type inference engine

import type { Expr, LispValue } from '../core/types';
import type { Type, TypeScheme, Constraint, Pattern, MatchBranch, DataType, EffectRow, EffectDeclaration } from './type-ast';
import type { ModuleContext } from '../modules/module-types';
import { emptyModuleContext, fileModuleContext } from '../modules/module-types';
import { resolveModulePath, loadAndParseModule } from '../modules/module-loader';
import * as path from 'path';
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
  effectRowClosed,
  effectRowOpen,
  effectRowEmpty,
  freshEffectRowVar,
  formatEffectRow,
} from './type-ast';
import type { TypeEnv, Substitution } from './type-env';
import {
  extendEnv,
  childEnv,
  lookupVar,
  lookupConstructor,
  lookupDataType,
  lookupEffect,
  lookupEffectOp,
  defineEffect,
  applySubst,
  applySubstEnv,
  composeSubst,
  emptySubst,
  freshTypeVar,
  generalize,
  instantiate,
  defineDataType,
  extendEnvMany,
  mergeEnv,
  type EffectRowSubstitution,
  emptyEffectRowSubst,
  applyEffectRowSubst,
} from './type-env';
import { unify, unifyWithContext, unifyMany, unifyEffectRows, combineEffectRows } from './type-unify';
import type { TypeResult, TypeError } from './type-errors';
import { ok, err, flatMap, sequence, map } from './type-errors';
import { resolveConstraints } from './type-builtins';

// Inference result: type, substitution, constraints, and effects
export type InferResult = {
  type: Type;
  subst: Substitution;
  constraints: Constraint[];
  effects: EffectRow;  // Effects performed by this expression
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

// Helper to create a pure (effect-free) result
const pureResult = (type: Type, subst: Substitution = emptySubst(), constraints: Constraint[] = []): InferResult => ({
  type,
  subst,
  constraints,
  effects: effectRowEmpty,
});

// Infer type of an expression
export const inferExpr = (
  env: TypeEnv,
  expr: Expr
): TypeResult<InferResult> => {
  // Nil
  if (expr.kind === 'nil') {
    return ok(pureResult(tUnit));
  }

  // Atom
  if (expr.kind === 'atom') {
    return inferAtom(env, expr.value);
  }

  // List expression
  const elements = listToArray(expr);
  if (elements.length === 0) {
    return ok(pureResult(tUnit));
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
      case 'effect':
        return inferEffectDef(env, elements);
      case 'perform':
        return inferPerform(env, elements);
      case 'handle':
        return inferHandle(env, elements);
      case 'resume':
        return inferResume(env, elements);
      case 'import':
        // Import is processed at program level in inferProgram
        // Here we just return Unit to avoid "undefined variable" errors
        return ok(pureResult(tUnit));
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
      return ok(pureResult(type));
    }
    case 'string':
      return ok(pureResult(tString));
    case 'symbol': {
      // Boolean literals
      if (value.value === '#t' || value.value === '#f') {
        return ok(pureResult(tBool));
      }

      // Variable lookup
      const scheme = lookupVar(env, value.value);
      if (!scheme) {
        return err({ kind: 'undefined-variable', name: value.value });
      }

      // Instantiate the type scheme
      const { type, constraints } = instantiate(scheme);
      return ok(pureResult(type, emptySubst(), constraints));
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
              effects: valueResult.effects,
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
      effects: result.effects,
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

  // Infer body type and collect effects
  let bodySubst = emptySubst();
  let bodyType: Type = tUnit;
  let allConstraints: Constraint[] = [];
  let bodyEffects: EffectRow = effectRowEmpty;

  for (let i = 0; i < bodyExprs.length; i++) {
    const bodyExpr = bodyExprs[i]!;
    const result = inferExpr(applySubstEnv(bodySubst, paramEnv), bodyExpr);
    if (!result.ok) return result;

    bodySubst = composeSubst(result.value.subst, bodySubst);
    allConstraints = [...allConstraints, ...result.value.constraints];
    // Combine effects from all body expressions
    bodyEffects = combineEffectRows(bodyEffects, result.value.effects);

    if (i === bodyExprs.length - 1) {
      bodyType = result.value.type;
    }
  }

  // Build function type with effects
  const paramTypes = params.map((p) => applySubst(bodySubst, p.type));
  const resultType = applySubst(bodySubst, bodyType);
  // Include effects in the function type if the body has effects
  const fnEffects = bodyEffects.effects.length > 0 || bodyEffects.kind === 'effect-row-open' ? bodyEffects : null;
  const fnType = typeArrow(paramTypes, resultType, fnEffects);

  // Lambda definition itself is pure - the effects are captured in the function type
  return ok({
    type: fnType,
    subst: bodySubst,
    constraints: allConstraints,
    effects: effectRowEmpty,
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
              // Combine effects from all branches
              const allEffects = combineEffectRows(
                combineEffectRows(condResult.effects, thenResult.effects),
                elseResult.effects
              );
              return ok({
                type: applySubst(finalSubst, thenType),
                subst: finalSubst,
                constraints: [...condResult.constraints, ...thenResult.constraints, ...elseResult.constraints],
                effects: allEffects,
              });
            });
          });
        } else {
          // No else branch - result is Unit
          const allEffects = combineEffectRows(condResult.effects, thenResult.effects);
          return ok({
            type: tUnit,
            subst: s2,
            constraints: [...condResult.constraints, ...thenResult.constraints],
            effects: allEffects,
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
  let allEffects: EffectRow = effectRowEmpty;

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
    allEffects = combineEffectRows(allEffects, valueResult.value.effects);

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
    allEffects = combineEffectRows(allEffects, result.value.effects);

    if (i === bodyExprs.length - 1) {
      bodyType = result.value.type;
    }
  }

  return ok({
    type: applySubst(subst, bodyType),
    subst,
    constraints: allConstraints,
    effects: allEffects,
  });
};

// Infer type of begin expression
const inferBegin = (env: TypeEnv, elements: Expr[]): TypeResult<InferResult> => {
  const bodyExprs = elements.slice(1);

  if (bodyExprs.length === 0) {
    return ok(pureResult(tUnit));
  }

  let subst = emptySubst();
  let lastType: Type = tUnit;
  let allConstraints: Constraint[] = [];
  let allEffects: EffectRow = effectRowEmpty;

  for (const expr of bodyExprs) {
    const result = inferExpr(applySubstEnv(subst, env), expr);
    if (!result.ok) return result;

    subst = composeSubst(result.value.subst, subst);
    allConstraints = [...allConstraints, ...result.value.constraints];
    allEffects = combineEffectRows(allEffects, result.value.effects);
    lastType = result.value.type;
  }

  return ok({
    type: applySubst(subst, lastType),
    subst,
    constraints: allConstraints,
    effects: allEffects,
  });
};

// Infer type of cond expression
const inferCond = (env: TypeEnv, elements: Expr[]): TypeResult<InferResult> => {
  const clauses = elements.slice(1);

  if (clauses.length === 0) {
    return ok(pureResult(tUnit));
  }

  let subst = emptySubst();
  let resultType: Type | undefined;
  let allConstraints: Constraint[] = [];
  let allEffects: EffectRow = effectRowEmpty;

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
      allEffects = combineEffectRows(allEffects, testResult.value.effects);
    }

    // Infer result type
    const exprResult = inferExpr(applySubstEnv(subst, env), resultExpr);
    if (!exprResult.ok) return exprResult;

    subst = composeSubst(exprResult.value.subst, subst);
    allConstraints = [...allConstraints, ...exprResult.value.constraints];
    allEffects = combineEffectRows(allEffects, exprResult.value.effects);

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
    effects: allEffects,
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
    let allEffects = scrutineeResult.effects;
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
      allEffects = combineEffectRows(allEffects, bodyResult.value.effects);

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
      effects: allEffects,
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
export const VARIADIC_ARITH_OPS = new Map<string, { minArgs: number }>([
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
    return ok(pureResult(tInt));
  }

  let subst = emptySubst();
  let allConstraints: Constraint[] = [];
  let allEffects: EffectRow = effectRowEmpty;
  const argTypes: Type[] = [];

  // Infer all argument types
  for (const argExpr of argExprs) {
    const argResult = inferExpr(applySubstEnv(subst, env), argExpr);
    if (!argResult.ok) return argResult;
    subst = composeSubst(argResult.value.subst, subst);
    allConstraints = [...allConstraints, ...argResult.value.constraints];
    allEffects = combineEffectRows(allEffects, argResult.value.effects);
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
    effects: allEffects,
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
    let allEffects = fnResult.effects;
    const argTypes: Type[] = [];

    // Infer argument types
    for (const argExpr of argExprs) {
      const argResult = inferExpr(applySubstEnv(subst, env), argExpr);
      if (!argResult.ok) return argResult;

      subst = composeSubst(argResult.value.subst, subst);
      allConstraints = [...allConstraints, ...argResult.value.constraints];
      allEffects = combineEffectRows(allEffects, argResult.value.effects);
      argTypes.push(argResult.value.type);
    }

    // Create fresh result type variable
    const resultType = freshTypeVar();

    // Build expected function type (without effects in the expected type)
    const expectedFnType = typeArrow(argTypes.map((t) => applySubst(subst, t)), resultType);

    // Unify with actual function type
    const fnType = applySubst(subst, fnResult.type);
    return flatMap(unifyWithContext(fnType, expectedFnType, 'function application'), (appSubst) => {
      const finalSubst = composeSubst(appSubst, subst);

      // If the function type has effects, those effects are performed when calling it
      const appliedFnType = applySubst(finalSubst, fnResult.type);
      if (appliedFnType.kind === 'type-arrow' && appliedFnType.effects) {
        allEffects = combineEffectRows(allEffects, appliedFnType.effects);
      }

      return ok({
        type: applySubst(finalSubst, resultType),
        subst: finalSubst,
        constraints: allConstraints,
        effects: allEffects,
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
  let allEffects: EffectRow = effectRowEmpty;

  // Type check all arguments (for side effects and to catch obvious errors)
  for (const argExpr of argExprs) {
    const argResult = inferExpr(applySubstEnv(subst, env), argExpr);
    if (!argResult.ok) return argResult;
    subst = composeSubst(argResult.value.subst, subst);
    allConstraints = [...allConstraints, ...argResult.value.constraints];
    allEffects = combineEffectRows(allEffects, argResult.value.effects);
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
    effects: allEffects,
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
  return ok(pureResult(tUnit));
};

// Infer typeclass definition
const inferClassDef = (_env: TypeEnv, _elements: Expr[]): TypeResult<InferResult> => {
  // (class (ClassName a) method-signatures...)
  // This is just validated, not inferred
  return ok(pureResult(tUnit));
};

// Infer instance definition
const inferInstanceDef = (_env: TypeEnv, _elements: Expr[]): TypeResult<InferResult> => {
  // (instance (ClassName Type) method-implementations...)
  // This is just validated, not inferred
  return ok(pureResult(tUnit));
};

// Infer type alias
const inferTypeAlias = (_env: TypeEnv, _elements: Expr[]): TypeResult<InferResult> => {
  // (type AliasName Type)
  return ok(pureResult(tUnit));
};

// ===== Effect System =====

// Parse an effect operation from (op-name (-> ParamTypes... ResultType))
// or with constraints: (op-name (=> (Show a) (-> a Unit)))
const parseEffectOp = (
  expr: Expr,
  env: TypeEnv
): TypeResult<{ name: string; typeParams: string[]; constraints: Constraint[]; params: Type[]; result: Type }> => {
  if (expr.kind !== 'list') {
    return err({ kind: 'invalid-type-annotation', message: 'Effect operation must be a list' });
  }

  const elements = listToArray(expr);
  // Format: (op-name (-> Type1 Type2)) or (op-name (=> (Constraint a) (-> Type1 Type2)))
  if (elements.length !== 2) {
    return err({ kind: 'invalid-type-annotation', message: 'Effect operation must be (name (-> ParamType ResultType))' });
  }

  const opName = getSymbol(elements[0]!);
  if (!opName) {
    return err({ kind: 'invalid-type-annotation', message: 'Effect operation name must be a symbol' });
  }

  const typeExpr = elements[1]!;
  let constraints: Constraint[] = [];
  let arrowExpr: Expr = typeExpr;

  // Check if this is a constrained type: (=> constraints... arrow-type)
  if (typeExpr.kind === 'list') {
    const typeElements = listToArray(typeExpr);
    if (typeElements.length > 0 && isSymbol(typeElements[0]!, '=>')) {
      // Parse constraints
      if (typeElements.length < 3) {
        return err({ kind: 'invalid-type-annotation', message: 'Constrained type needs constraints and a body' });
      }

      const constraintExprs = typeElements.slice(1, -1);
      arrowExpr = typeElements[typeElements.length - 1]!;

      for (const cExpr of constraintExprs) {
        const constraintResult = parseConstraint(cExpr, env);
        if (!constraintResult.ok) return constraintResult;
        constraints.push(constraintResult.value);
      }
    }
  }

  // Parse the arrow type
  const typeResult = parseTypeExpr(arrowExpr, env);
  if (!typeResult.ok) return typeResult;

  const fnType = typeResult.value;
  if (fnType.kind !== 'type-arrow') {
    return err({ kind: 'invalid-type-annotation', message: 'Effect operation type must be a function type (-> ...)' });
  }

  // Collect type parameters from the constraints and arrow type
  const typeParams: string[] = [];
  const seenParams = new Set<string>();

  // Collect from constraints
  for (const c of constraints) {
    const freeVars = freeTypeVars(c.typeArg);
    for (const v of freeVars) {
      if (!seenParams.has(v)) {
        seenParams.add(v);
        typeParams.push(v);
      }
    }
  }

  // Collect from function type
  for (const param of fnType.params) {
    const freeVars = freeTypeVars(param);
    for (const v of freeVars) {
      if (!seenParams.has(v)) {
        seenParams.add(v);
        typeParams.push(v);
      }
    }
  }
  const resultFreeVars = freeTypeVars(fnType.result);
  for (const v of resultFreeVars) {
    if (!seenParams.has(v)) {
      seenParams.add(v);
      typeParams.push(v);
    }
  }

  return ok({ name: opName, typeParams, constraints, params: fnType.params, result: fnType.result });
};

// Infer effect definition
// Syntax: (effect EffectName (op1 : Type1 -> Type2) ...)
const inferEffectDef = (env: TypeEnv, elements: Expr[]): TypeResult<InferResult> => {
  if (elements.length < 2) {
    return err({ kind: 'arity-mismatch', expected: 1, got: elements.length - 1, context: 'effect' });
  }

  // Get effect name
  const nameExpr = elements[1]!;
  if (nameExpr.kind !== 'atom' || nameExpr.value.kind !== 'symbol') {
    return err({ kind: 'invalid-type-annotation', message: 'effect name must be a symbol' });
  }
  const effectName = nameExpr.value.value;

  // Parse operations
  const operations: { name: string; params: Type[]; result: Type }[] = [];
  for (let i = 2; i < elements.length; i++) {
    const opResult = parseEffectOp(elements[i]!, env);
    if (!opResult.ok) return opResult;
    operations.push(opResult.value);
  }

  // Effect declarations return unit and are pure
  // The actual registration happens in inferProgram
  return ok(pureResult(tUnit));
};

// Infer perform expression
// Syntax: (perform op-name arg?)
const inferPerform = (env: TypeEnv, elements: Expr[]): TypeResult<InferResult> => {
  if (elements.length < 2) {
    return err({ kind: 'arity-mismatch', expected: 1, got: elements.length - 1, context: 'perform' });
  }

  // Get operation name
  const opNameExpr = elements[1]!;
  const opName = getSymbol(opNameExpr);
  if (!opName) {
    return err({ kind: 'invalid-type-annotation', message: 'perform operation name must be a symbol' });
  }

  // Look up the effect operation in the environment
  const opInfo = lookupEffectOp(env, opName);

  let subst = emptySubst();
  let allConstraints: Constraint[] = [];
  let allEffects: EffectRow = effectRowEmpty;

  // Infer argument types if any
  const argExprs = elements.slice(2);
  const argTypes: Type[] = [];
  for (const argExpr of argExprs) {
    const argResult = inferExpr(applySubstEnv(subst, env), argExpr);
    if (!argResult.ok) return argResult;
    subst = composeSubst(argResult.value.subst, subst);
    allConstraints = [...allConstraints, ...argResult.value.constraints];
    allEffects = combineEffectRows(allEffects, argResult.value.effects);
    argTypes.push(argResult.value.type);
  }

  let resultType: Type;

  if (opInfo) {
    // Found the effect operation - use its type
    const { effect, op } = opInfo;

    // Check arity
    if (argTypes.length !== op.params.length) {
      return err({
        kind: 'arity-mismatch',
        expected: op.params.length,
        got: argTypes.length,
        context: `effect operation ${opName}`,
      });
    }

    // Instantiate operation's type parameters with fresh type variables
    const typeParamSubst = new Map<string, Type>();
    for (const tv of op.typeParams) {
      typeParamSubst.set(tv, freshTypeVar());
    }

    // Apply substitution to get instantiated param types and result type
    const instantiatedParams = op.params.map(p => applySubst(typeParamSubst, p));
    const instantiatedResult = applySubst(typeParamSubst, op.result);

    // Instantiate and add constraints from the operation
    for (const c of op.constraints) {
      allConstraints.push({
        class: c.class,
        typeArg: applySubst(typeParamSubst, c.typeArg),
      });
    }

    // Unify argument types with instantiated parameter types
    for (let i = 0; i < argTypes.length; i++) {
      const argType = applySubst(subst, argTypes[i]!);
      const paramType = instantiatedParams[i]!;
      const unifyResult = unifyWithContext(argType, paramType, `argument ${i + 1} of ${opName}`);
      if (!unifyResult.ok) return unifyResult;
      subst = composeSubst(unifyResult.value, subst);
    }

    resultType = applySubst(subst, instantiatedResult);

    // Add this effect to the performed effects
    allEffects = combineEffectRows(allEffects, effectRowClosed([effect.name]));
  } else {
    // Effect operation not found - require declaration
    return err({
      kind: 'undefined-variable',
      name: `effect operation '${opName}' (declare with (effect EffectName (${opName} : ArgType -> ResultType)))`,
    });
  }

  return ok({
    type: resultType,
    subst,
    constraints: allConstraints,
    effects: allEffects,
  });
};

// Infer handle expression
// Syntax: (handle expr ((op arg k) -> body) ((op2 arg k) -> body2) ...)
// Each handler clause is wrapped: ((pattern) -> body)
const inferHandle = (env: TypeEnv, elements: Expr[]): TypeResult<InferResult> => {
  if (elements.length < 2) {
    return err({ kind: 'arity-mismatch', expected: 1, got: elements.length - 1, context: 'handle' });
  }

  // Infer the type of the handled expression
  const bodyExpr = elements[1]!;
  const bodyResult = inferExpr(env, bodyExpr);
  if (!bodyResult.ok) return bodyResult;

  let subst = bodyResult.value.subst;
  let allConstraints = [...bodyResult.value.constraints];
  let resultType: Type | undefined;
  const handledEffects: string[] = [];

  // Parse handler clauses - each is ((pattern) -> body)
  const handlerClauses = elements.slice(2);
  for (const clause of handlerClauses) {
    const clauseElements = listToArray(clause);
    if (clauseElements.length !== 3) {
      return err({ kind: 'invalid-type-annotation', message: 'handle clause must be ((pattern) -> body)' });
    }
    const arrow = clauseElements[1];
    if (!arrow || !isSymbol(arrow, '->')) {
      return err({ kind: 'invalid-type-annotation', message: 'handle clause must be ((pattern) -> body)' });
    }

    const pattern = clauseElements[0]!;
    const body = clauseElements[2]!;

    // Parse handler pattern: (op-name args... k)
    if (pattern.kind === 'list') {
      const patternElements = listToArray(pattern);
      if (patternElements.length < 1) {
        return err({ kind: 'invalid-type-annotation', message: 'Handler pattern must have operation name' });
      }

      const opName = getSymbol(patternElements[0]!);
      if (opName) {
        // Check for return handler
        if (opName === 'return') {
          // Return handler: (return x) -> body
          // x is bound to the value returned by the body
          let handlerEnv = applySubstEnv(subst, env);
          if (patternElements.length >= 2) {
            const returnVarName = getSymbol(patternElements[1]!);
            if (returnVarName) {
              const returnType = applySubst(subst, bodyResult.value.type);
              handlerEnv = extendEnv(handlerEnv, returnVarName, {
                quantifiedVars: [],
                qualType: { constraints: [], type: returnType },
              });
            }
          }

          const handlerResult = inferExpr(handlerEnv, body);
          if (!handlerResult.ok) return handlerResult;

          subst = composeSubst(handlerResult.value.subst, subst);
          allConstraints = [...allConstraints, ...handlerResult.value.constraints];

          const handlerType = applySubst(subst, handlerResult.value.type);
          if (resultType === undefined) {
            resultType = handlerType;
          } else {
            const unifyResult = unifyWithContext(resultType, handlerType, 'handler return types');
            if (!unifyResult.ok) return unifyResult;
            subst = composeSubst(unifyResult.value, subst);
            resultType = applySubst(subst, resultType);
          }
          continue;
        }

        // Effect handler: (op-name args... k) -> body
        handledEffects.push(opName);

        // Look up the effect operation
        const opInfo = lookupEffectOp(env, opName);

        let handlerEnv = applySubstEnv(subst, env);

        // Bind arguments from pattern
        const argPatterns = patternElements.slice(1, -1); // All except first (name) and last (continuation)
        const continuationPattern = patternElements[patternElements.length - 1];

        if (opInfo) {
          // Bind each argument to its type from the effect operation
          for (let i = 0; i < argPatterns.length; i++) {
            const argName = getSymbol(argPatterns[i]!);
            if (argName && i < opInfo.op.params.length) {
              handlerEnv = extendEnv(handlerEnv, argName, {
                quantifiedVars: [],
                qualType: { constraints: [], type: opInfo.op.params[i]! },
              });
            }
          }

          // Bind continuation: k has type (ResultType -> BodyType)
          // The continuation returns the body's result type when resumed
          if (continuationPattern) {
            const kName = getSymbol(continuationPattern);
            if (kName) {
              // The continuation's return type is the body's type
              const kType = typeArrow([opInfo.op.result], applySubst(subst, bodyResult.value.type));
              handlerEnv = extendEnv(handlerEnv, kName, {
                quantifiedVars: [],
                qualType: { constraints: [], type: kType },
              });
            }
          }
        } else {
          // Unknown operation - require declaration
          return err({
            kind: 'undefined-variable',
            name: `effect operation '${opName}' in handler (declare with (effect EffectName (${opName} : ArgType -> ResultType)))`,
          });
        }

        // Infer handler body
        const handlerResult = inferExpr(handlerEnv, body);
        if (!handlerResult.ok) return handlerResult;

        subst = composeSubst(handlerResult.value.subst, subst);
        allConstraints = [...allConstraints, ...handlerResult.value.constraints];

        const handlerType = applySubst(subst, handlerResult.value.type);
        if (resultType === undefined) {
          resultType = handlerType;
        } else {
          const unifyResult = unifyWithContext(resultType, handlerType, 'handler return types');
          if (!unifyResult.ok) return unifyResult;
          subst = composeSubst(unifyResult.value, subst);
          resultType = applySubst(subst, resultType);
        }
      }
    }
  }

  // If no handlers provided result type, use body type
  if (resultType === undefined) {
    resultType = bodyResult.value.type;
  }

  // Remove handled effects from the body's effects
  let remainingEffects = bodyResult.value.effects;
  if (handledEffects.length > 0) {
    // Filter out handled effects
    const remaining = remainingEffects.effects.filter((e) => !handledEffects.includes(e));
    if (remainingEffects.kind === 'effect-row-open') {
      remainingEffects = effectRowOpen(remaining, remainingEffects.rowVar);
    } else {
      remainingEffects = effectRowClosed(remaining);
    }
  }

  return ok({
    type: applySubst(subst, resultType),
    subst,
    constraints: allConstraints,
    effects: remainingEffects,
  });
};

// Infer resume expression
// Syntax: (resume k value)
// k is a continuation (function from value type to result type)
// value is the value to pass to the continuation
const inferResume = (env: TypeEnv, elements: Expr[]): TypeResult<InferResult> => {
  if (elements.length !== 3) {
    return err({ kind: 'arity-mismatch', expected: 2, got: elements.length - 1, context: 'resume' });
  }

  // Infer the continuation type
  const contExpr = elements[1]!;
  const contResult = inferExpr(env, contExpr);
  if (!contResult.ok) return contResult;

  let subst = contResult.value.subst;
  let allConstraints = contResult.value.constraints;
  let allEffects = contResult.value.effects;

  // Infer the value type
  const valueExpr = elements[2]!;
  const valueResult = inferExpr(applySubstEnv(subst, env), valueExpr);
  if (!valueResult.ok) return valueResult;

  subst = composeSubst(valueResult.value.subst, subst);
  allConstraints = [...allConstraints, ...valueResult.value.constraints];
  allEffects = combineEffectRows(allEffects, valueResult.value.effects);

  // The continuation should be a function type (A -> B)
  // We create a fresh return type and unify the continuation with (valueType -> returnType)
  const returnType = freshTypeVar();
  const valueType = applySubst(subst, valueResult.value.type);
  const contType = applySubst(subst, contResult.value.type);

  // Expected: k : (valueType -> returnType)
  const expectedContType = typeArrow([valueType], returnType);
  const unifyResult = unifyWithContext(contType, expectedContType, 'resume continuation');
  if (!unifyResult.ok) return unifyResult;

  subst = composeSubst(unifyResult.value, subst);

  return ok({
    type: applySubst(subst, returnType),
    subst,
    constraints: allConstraints,
    effects: allEffects,
  });
};

// ===== Main Entry Point =====

// Type module cache - stores TypeEnv for loaded modules
type TypeModuleState =
  | { status: 'loading' }
  | { status: 'loaded'; typeEnv: TypeEnv };
type TypeModuleCache = Map<string, TypeModuleState>;

// Module context for type checking
type TypeModuleContext = {
  filePath: string | null;
  cache: TypeModuleCache;
};

const emptyTypeModuleContext = (): TypeModuleContext => ({
  filePath: null,
  cache: new Map(),
});

const fileTypeModuleContext = (filePath: string): TypeModuleContext => ({
  filePath,
  cache: new Map(),
});

// Process an import statement during type checking
const processImport = (
  env: TypeEnv,
  elements: Expr[],
  moduleContext: TypeModuleContext
): TypeResult<TypeEnv> => {
  if (elements.length < 2) {
    return err({ kind: 'invalid-type-annotation', message: 'import expects at least a path' });
  }

  // Get import path
  const pathExpr = elements[1];
  if (pathExpr?.kind !== 'atom' || pathExpr.value.kind !== 'string') {
    return err({ kind: 'invalid-type-annotation', message: 'import path must be a string literal' });
  }
  const importPath = pathExpr.value.value;

  // Determine import form
  let mode: 'all' | 'selective' | 'qualified' = 'all';
  let selectedNames: string[] = [];
  let qualifier = '';

  if (elements.length >= 3) {
    const second = elements[2];
    if (second?.kind === 'atom' && second.value.kind === 'symbol' && second.value.value === 'as') {
      if (elements.length !== 4) {
        return err({ kind: 'invalid-type-annotation', message: 'import as expects a qualifier name' });
      }
      const qualExpr = elements[3];
      if (qualExpr?.kind !== 'atom' || qualExpr.value.kind !== 'symbol') {
        return err({ kind: 'invalid-type-annotation', message: 'import qualifier must be a symbol' });
      }
      mode = 'qualified';
      qualifier = qualExpr.value.value;
    } else if (second?.kind === 'list') {
      mode = 'selective';
      const names = listToArray(second);
      for (const n of names) {
        if (n.kind !== 'atom' || n.value.kind !== 'symbol') {
          return err({ kind: 'invalid-type-annotation', message: 'import names must be symbols' });
        }
        selectedNames.push(n.value.value);
      }
    } else {
      return err({ kind: 'invalid-type-annotation', message: 'Invalid import syntax' });
    }
  }

  // Resolve module path
  if (!moduleContext.filePath) {
    return err({ kind: 'invalid-type-annotation', message: 'Cannot use import without a file context' });
  }
  const absolutePath = resolveModulePath(importPath, moduleContext.filePath);

  // Check cache
  const cached = moduleContext.cache.get(absolutePath);
  if (cached) {
    if (cached.status === 'loading') {
      return err({ kind: 'invalid-type-annotation', message: `Circular dependency detected: ${absolutePath}` });
    }
    // Module already type-checked - merge TypeEnv
    return ok(mergeEnv(env, cached.typeEnv, { mode, selectedNames, qualifier }));
  }

  // Mark as loading
  moduleContext.cache.set(absolutePath, { status: 'loading' });

  // Load and parse
  let moduleExprs: Expr[];
  try {
    moduleExprs = loadAndParseModule(absolutePath);
  } catch (e) {
    return err({ kind: 'invalid-type-annotation', message: `Failed to load module: ${e instanceof Error ? e.message : String(e)}` });
  }

  // Type check the module recursively
  const moduleResult = inferProgramWithContext(
    env, // Start with same base env (builtins)
    moduleExprs,
    { filePath: absolutePath, cache: moduleContext.cache }
  );

  if (!moduleResult.ok) return moduleResult;

  // Cache the result
  moduleContext.cache.set(absolutePath, { status: 'loaded', typeEnv: moduleResult.value.env });

  // Merge into current env
  return ok(mergeEnv(env, moduleResult.value.env, { mode, selectedNames, qualifier }));
};

// Infer types for a list of expressions with module context
export const inferProgramWithContext = (
  env: TypeEnv,
  exprs: Expr[],
  moduleContext: TypeModuleContext
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

      // Handle effect declarations - they extend the environment
      if (elements.length >= 2 && isSymbol(elements[0]!, 'effect')) {
        const effectResult = processEffectDef(currentEnv, elements);
        if (!effectResult.ok) return effectResult;
        currentEnv = effectResult.value;
        types.push(tUnit);
        continue;
      }

      // Handle import statements - they extend the environment with imported bindings
      if (elements.length >= 2 && isSymbol(elements[0]!, 'import')) {
        const importResult = processImport(currentEnv, elements, moduleContext);
        if (!importResult.ok) return importResult;
        currentEnv = importResult.value;
        types.push(tUnit);
        continue;
      }
    }

    // Regular expression inference
    const result = inferExpr(applySubstEnv(globalSubst, currentEnv), expr);
    if (!result.ok) return result;

    globalSubst = composeSubst(result.value.subst, globalSubst);
    types.push(applySubst(globalSubst, result.value.type));

    // Apply substitution to constraints before checking
    const substitutedConstraints = result.value.constraints.map(c => ({
      class: c.class,
      typeArg: applySubst(globalSubst, c.typeArg),
    }));

    // Check constraints
    const resolvedConstraints = resolveConstraints(substitutedConstraints);
    for (const c of resolvedConstraints) {
      // If constraint is on a concrete type with no instance, error
      if (c.typeArg.kind !== 'type-var') {
        return err({ kind: 'no-instance', constraint: c });
      }
    }
  }

  return ok({ env: currentEnv, types });
};

// Infer types for a list of expressions (backwards-compatible version without module context)
export const inferProgram = (
  env: TypeEnv,
  exprs: Expr[],
  filePath?: string
): TypeResult<{ env: TypeEnv; types: Type[] }> => {
  // For browser compatibility, only use path.resolve for non-absolute paths
  // Absolute paths (starting with /) are used as-is
  const resolvedPath = filePath
    ? (filePath.startsWith('/') ? filePath : path.resolve(filePath))
    : null;
  const moduleContext = resolvedPath
    ? fileTypeModuleContext(resolvedPath)
    : emptyTypeModuleContext();
  return inferProgramWithContext(env, exprs, moduleContext);
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

// Process effect definition and update environment
const processEffectDef = (env: TypeEnv, elements: Expr[]): TypeResult<TypeEnv> => {
  if (elements.length < 2) {
    return err({ kind: 'arity-mismatch', expected: 1, got: elements.length - 1, context: 'effect' });
  }

  // Get effect name
  const nameExpr = elements[1]!;
  if (nameExpr.kind !== 'atom' || nameExpr.value.kind !== 'symbol') {
    return err({ kind: 'invalid-type-annotation', message: 'effect name must be a symbol' });
  }
  const effectName = nameExpr.value.value;

  // Parse operations (now includes typeParams and constraints)
  const operations: { name: string; typeParams: string[]; constraints: Constraint[]; params: Type[]; result: Type }[] = [];
  for (let i = 2; i < elements.length; i++) {
    const opResult = parseEffectOp(elements[i]!, env);
    if (!opResult.ok) return opResult;
    operations.push(opResult.value);
  }

  // Create effect declaration
  const effectDecl: EffectDeclaration = {
    name: effectName,
    typeParams: [],  // TODO: Support parameterized effects like (State a)
    operations,
  };

  // Register effect in environment
  return ok(defineEffect(env, effectDecl));
};
