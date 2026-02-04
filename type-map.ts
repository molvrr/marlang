// Type mapping for AST nodes
// Provides type information for every sub-expression in the AST

import type { Expr, LispValue } from './types';
import type { Type, Constraint, TypeScheme } from './type-ast';
import {
  typeVar,
  typeConst,
  typeArrow,
  typeApp,
  tInt,
  tFloat,
  tBool,
  tString,
  tUnit,
  tFFILibrary,
  tFFIStructDef,
  tFFIStruct,
  formatType,
} from './type-ast';
import type { TypeEnv, Substitution } from './type-env';
import {
  extendEnv,
  childEnv,
  lookupVar,
  lookupConstructor,
  applySubst,
  applySubstEnv,
  composeSubst,
  emptySubst,
  freshTypeVar,
  generalize,
  instantiate,
  defineDataType,
} from './type-env';
import { unify, unifyWithContext } from './type-unify';
import type { TypeResult, TypeError } from './type-errors';
import { ok, err, flatMap, sequence } from './type-errors';
import { resolveConstraints } from './type-builtins';
import { createBuiltinEnv } from './type-builtins';
import { resetTypeVarCounter } from './type-env';
import { parseTypeExpr, parseTypeAnnotation, parsePattern, VARIADIC_ARITH_OPS } from './type-infer';

// Path identifies a node in the AST (e.g., [0, 1, 2] = first expr, second child, third child)
export type NodePath = number[];

// Convert path to string key
export const pathKey = (path: NodePath): string => path.join('.');

// Type information for a node
export type NodeTypeInfo = {
  type: Type;
  formattedType: string;
};

// Error information for a node
export type NodeErrorInfo = {
  path: NodePath;
  message: string;
};

// Result of type mapping
export type TypeMapResult = {
  ok: true;
  types: Map<string, NodeTypeInfo>;  // path key -> type info
  topLevelTypes: Type[];             // types for top-level expressions
} | {
  ok: false;
  error: TypeError;
  errorPath: NodePath | null;        // path to the node that caused the error
  types: Map<string, NodeTypeInfo>;  // partial type info collected before error
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

// Inference context that accumulates type info
type InferContext = {
  env: TypeEnv;
  subst: Substitution;
  types: Map<string, NodeTypeInfo>;
  constraints: Constraint[];
};

// Record a type for a path
const recordType = (ctx: InferContext, path: NodePath, type: Type): void => {
  const finalType = applySubst(ctx.subst, type);
  ctx.types.set(pathKey(path), {
    type: finalType,
    formattedType: formatType(finalType),
  });
};

// Update all recorded types with current substitution
const finalizeTypes = (ctx: InferContext): void => {
  for (const [key, info] of ctx.types) {
    const finalType = applySubst(ctx.subst, info.type);
    ctx.types.set(key, {
      type: finalType,
      formattedType: formatType(finalType),
    });
  }
};

// Infer type and record it
const inferAndRecord = (
  ctx: InferContext,
  expr: Expr,
  path: NodePath
): TypeResult<Type> => {
  const result = inferExprWithMap(ctx, expr, path);
  if (result.ok) {
    recordType(ctx, path, result.value);
  }
  return result;
};

// Main inference function that builds type map
const inferExprWithMap = (
  ctx: InferContext,
  expr: Expr,
  path: NodePath
): TypeResult<Type> => {
  // Nil
  if (expr.kind === 'nil') {
    return ok(tUnit);
  }

  // Atom
  if (expr.kind === 'atom') {
    return inferAtomWithMap(ctx, expr.value);
  }

  // List expression
  const elements = listToArray(expr);
  if (elements.length === 0) {
    return ok(tUnit);
  }

  const first = elements[0]!;
  const firstSym = getSymbol(first);

  // Special forms
  if (firstSym) {
    switch (firstSym) {
      case 'define':
        return inferDefineWithMap(ctx, elements, path);
      case 'lambda':
        return inferLambdaWithMap(ctx, elements, path);
      case 'if':
        return inferIfWithMap(ctx, elements, path);
      case 'let':
        return inferLetWithMap(ctx, elements, path);
      case 'begin':
        return inferBeginWithMap(ctx, elements, path);
      case 'cond':
        return inferCondWithMap(ctx, elements, path);
      case 'match':
        return inferMatchWithMap(ctx, elements, path);
      case 'data':
        return inferDataDefWithMap(ctx, elements, path);
      case 'class':
      case 'instance':
      case 'type':
      case 'effect':
        return ok(tUnit);
      case 'perform': {
        // Perform returns fresh type var for now
        const resultType = freshTypeVar();
        return ok(resultType);
      }
      case 'handle': {
        // Handle has the type of its body expression
        if (elements.length < 2) return ok(tUnit);
        const bodyPath = [...path, 1];
        return inferExprWithMap(ctx, elements[1]!, bodyPath);
      }
    }
  }

  // Function application
  return inferApplicationWithMap(ctx, elements, path);
};

// Infer atom type
const inferAtomWithMap = (ctx: InferContext, value: LispValue): TypeResult<Type> => {
  switch (value.kind) {
    case 'number':
      return ok(value.isFloat ? tFloat : tInt);
    case 'string':
      return ok(tString);
    case 'symbol': {
      if (value.value === '#t' || value.value === '#f') {
        return ok(tBool);
      }
      const scheme = lookupVar(ctx.env, value.value);
      if (!scheme) {
        return err({ kind: 'undefined-variable', name: value.value });
      }
      const { type, constraints } = instantiate(scheme);
      ctx.constraints.push(...constraints);
      return ok(type);
    }
  }
};

// Infer define
const inferDefineWithMap = (
  ctx: InferContext,
  elements: Expr[],
  path: NodePath
): TypeResult<Type> => {
  if (elements.length < 3) {
    return err({ kind: 'arity-mismatch', expected: 2, got: elements.length - 1, context: 'define' });
  }

  const nameExpr = elements[1]!;
  const valueExpr = elements[2]!;

  // Check for type annotation
  let name: string;
  let annotatedType: Type | undefined;
  let annotatedConstraints: Constraint[] = [];

  if (nameExpr.kind === 'list') {
    const nameElements = listToArray(nameExpr);
    if (nameElements.length === 3 && isSymbol(nameElements[1]!, ':')) {
      const n = getSymbol(nameElements[0]!);
      if (!n) {
        return err({ kind: 'invalid-type-annotation', message: 'Expected symbol before :' });
      }
      name = n;
      const typeResult = parseTypeAnnotation(nameElements[2]!, ctx.env);
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

  // For recursive functions, add placeholder
  const placeholderType = annotatedType || freshTypeVar();
  ctx.env = extendEnv(ctx.env, name, {
    quantifiedVars: [],
    qualType: { constraints: annotatedConstraints, type: placeholderType },
  });

  // Infer value type
  const valuePath = [...path, 2];
  const valueResult = inferAndRecord(ctx, valueExpr, valuePath);
  if (!valueResult.ok) return valueResult;

  let valueType = valueResult.value;

  // Unify with annotation if present
  if (annotatedType) {
    const unifyResult = unifyWithContext(annotatedType, valueType, `definition of ${name}`);
    if (!unifyResult.ok) return unifyResult;
    ctx.subst = composeSubst(unifyResult.value, ctx.subst);
    valueType = applySubst(ctx.subst, annotatedType);
  }

  // Generalize and update environment
  const allConstraints = [...annotatedConstraints, ...ctx.constraints];
  const scheme = generalize(applySubstEnv(ctx.subst, ctx.env), allConstraints, applySubst(ctx.subst, valueType));
  ctx.env = extendEnv(ctx.env, name, scheme);

  return ok(tUnit);
};

// Infer lambda
const inferLambdaWithMap = (
  ctx: InferContext,
  elements: Expr[],
  path: NodePath
): TypeResult<Type> => {
  if (elements.length < 3) {
    return err({ kind: 'arity-mismatch', expected: 2, got: elements.length - 1, context: 'lambda' });
  }

  const paramsExpr = elements[1]!;
  const bodyExprs = elements.slice(2);

  // Parse parameters
  const paramList = listToArray(paramsExpr);
  const params: { name: string; type: Type }[] = [];
  let paramEnv = childEnv(ctx.env);

  for (const param of paramList) {
    if (param.kind === 'list') {
      const paramElements = listToArray(param);
      if (paramElements.length === 3 && isSymbol(paramElements[1]!, ':')) {
        const name = getSymbol(paramElements[0]!);
        if (!name) {
          return err({ kind: 'invalid-type-annotation', message: 'Parameter name must be a symbol' });
        }
        const typeResult = parseTypeExpr(paramElements[2]!, ctx.env);
        if (!typeResult.ok) return typeResult;
        params.push({ name, type: typeResult.value });
        paramEnv = extendEnv(paramEnv, name, { quantifiedVars: [], qualType: { constraints: [], type: typeResult.value } });
        continue;
      }
    }

    const name = getSymbol(param);
    if (!name) {
      return err({ kind: 'invalid-type-annotation', message: 'Parameter must be a symbol' });
    }
    const paramType = freshTypeVar();
    params.push({ name, type: paramType });
    paramEnv = extendEnv(paramEnv, name, { quantifiedVars: [], qualType: { constraints: [], type: paramType } });
  }

  // Create child context for body
  const bodyCtx: InferContext = {
    env: paramEnv,
    subst: ctx.subst,
    types: ctx.types,
    constraints: [...ctx.constraints],
  };

  // Infer body types
  let bodyType: Type = tUnit;
  for (let i = 0; i < bodyExprs.length; i++) {
    const bodyExpr = bodyExprs[i]!;
    const bodyPath = [...path, 2 + i];
    const result = inferAndRecord(bodyCtx, bodyExpr, bodyPath);
    if (!result.ok) return result;

    if (i === bodyExprs.length - 1) {
      bodyType = result.value;
    }
  }

  // Propagate substitution back
  ctx.subst = bodyCtx.subst;
  ctx.constraints = bodyCtx.constraints;

  // Build function type
  const paramTypes = params.map((p) => applySubst(ctx.subst, p.type));
  const resultType = applySubst(ctx.subst, bodyType);
  return ok(typeArrow(paramTypes, resultType));
};

// Infer if
const inferIfWithMap = (
  ctx: InferContext,
  elements: Expr[],
  path: NodePath
): TypeResult<Type> => {
  if (elements.length < 3 || elements.length > 4) {
    return err({ kind: 'arity-mismatch', expected: 3, got: elements.length - 1, context: 'if' });
  }

  const condExpr = elements[1]!;
  const thenExpr = elements[2]!;
  const elseExpr = elements[3];

  // Infer condition
  const condPath = [...path, 1];
  const condResult = inferAndRecord(ctx, condExpr, condPath);
  if (!condResult.ok) return condResult;

  // Condition must be Bool
  const condUnify = unifyWithContext(condResult.value, tBool, 'if condition');
  if (!condUnify.ok) return condUnify;
  ctx.subst = composeSubst(condUnify.value, ctx.subst);

  // Infer then branch
  const thenPath = [...path, 2];
  const thenResult = inferAndRecord(ctx, thenExpr, thenPath);
  if (!thenResult.ok) return thenResult;

  if (elseExpr) {
    // Infer else branch
    const elsePath = [...path, 3];
    const elseResult = inferAndRecord(ctx, elseExpr, elsePath);
    if (!elseResult.ok) return elseResult;

    // Branches must have same type
    const thenType = applySubst(ctx.subst, thenResult.value);
    const elseType = applySubst(ctx.subst, elseResult.value);

    const branchUnify = unifyWithContext(thenType, elseType, 'if branches');
    if (!branchUnify.ok) return branchUnify;
    ctx.subst = composeSubst(branchUnify.value, ctx.subst);

    return ok(applySubst(ctx.subst, thenType));
  } else {
    return ok(tUnit);
  }
};

// Infer let
const inferLetWithMap = (
  ctx: InferContext,
  elements: Expr[],
  path: NodePath
): TypeResult<Type> => {
  if (elements.length < 3) {
    return err({ kind: 'arity-mismatch', expected: 2, got: elements.length - 1, context: 'let' });
  }

  const bindingsExpr = elements[1]!;
  const bodyExprs = elements.slice(2);

  const bindings = listToArray(bindingsExpr);
  let letEnv = childEnv(ctx.env);

  for (let i = 0; i < bindings.length; i++) {
    const binding = bindings[i]!;
    const pair = listToArray(binding);
    if (pair.length !== 2) {
      return err({ kind: 'invalid-type-annotation', message: 'let binding must be (name value)' });
    }

    const nameExpr = pair[0]!;
    const valueExpr = pair[1]!;

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
        const typeResult = parseTypeExpr(nameElements[2]!, ctx.env);
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

    // Use a temporary context with letEnv for inferring the value
    const valueCtx: InferContext = {
      env: applySubstEnv(ctx.subst, letEnv),
      subst: ctx.subst,
      types: ctx.types,
      constraints: [...ctx.constraints],
    };

    // Infer value (path into bindings is complex, skip for now)
    const valueResult = inferExprWithMap(valueCtx, valueExpr, [...path, 1, i, 1]);
    if (!valueResult.ok) return valueResult;
    recordType(valueCtx, [...path, 1, i, 1], valueResult.value);

    ctx.subst = valueCtx.subst;
    ctx.constraints = valueCtx.constraints;

    let bindingType = valueResult.value;

    if (annotatedType) {
      const unifyResult = unifyWithContext(annotatedType, bindingType, `let binding ${name}`);
      if (!unifyResult.ok) return unifyResult;
      ctx.subst = composeSubst(unifyResult.value, ctx.subst);
      bindingType = applySubst(ctx.subst, annotatedType);
    }

    const scheme = generalize(applySubstEnv(ctx.subst, letEnv), ctx.constraints, applySubst(ctx.subst, bindingType));
    letEnv = extendEnv(letEnv, name, scheme);
  }

  // Infer body
  const bodyCtx: InferContext = {
    env: applySubstEnv(ctx.subst, letEnv),
    subst: ctx.subst,
    types: ctx.types,
    constraints: [...ctx.constraints],
  };

  let bodyType: Type = tUnit;
  for (let i = 0; i < bodyExprs.length; i++) {
    const bodyExpr = bodyExprs[i]!;
    const bodyPath = [...path, 2 + i];
    const result = inferAndRecord(bodyCtx, bodyExpr, bodyPath);
    if (!result.ok) return result;

    if (i === bodyExprs.length - 1) {
      bodyType = result.value;
    }
  }

  ctx.subst = bodyCtx.subst;
  ctx.constraints = bodyCtx.constraints;

  return ok(applySubst(ctx.subst, bodyType));
};

// Infer begin
const inferBeginWithMap = (
  ctx: InferContext,
  elements: Expr[],
  path: NodePath
): TypeResult<Type> => {
  const bodyExprs = elements.slice(1);

  if (bodyExprs.length === 0) {
    return ok(tUnit);
  }

  let lastType: Type = tUnit;
  for (let i = 0; i < bodyExprs.length; i++) {
    const expr = bodyExprs[i]!;
    const exprPath = [...path, 1 + i];
    const result = inferAndRecord(ctx, expr, exprPath);
    if (!result.ok) return result;
    lastType = result.value;
  }

  return ok(applySubst(ctx.subst, lastType));
};

// Infer cond
const inferCondWithMap = (
  ctx: InferContext,
  elements: Expr[],
  path: NodePath
): TypeResult<Type> => {
  const clauses = elements.slice(1);

  if (clauses.length === 0) {
    return ok(tUnit);
  }

  let resultType: Type | undefined;

  for (let i = 0; i < clauses.length; i++) {
    const clauseExpr = clauses[i]!;
    const clause = listToArray(clauseExpr);
    if (clause.length !== 2) {
      return err({ kind: 'invalid-type-annotation', message: 'cond clause must be (test result)' });
    }

    const testExpr = clause[0]!;
    const resultExpr = clause[1]!;

    // Check for else clause
    if (!isSymbol(testExpr, 'else')) {
      const testPath = [...path, 1 + i, 0];
      const testResult = inferAndRecord(ctx, testExpr, testPath);
      if (!testResult.ok) return testResult;

      const testUnify = unifyWithContext(testResult.value, tBool, 'cond test');
      if (!testUnify.ok) return testUnify;
      ctx.subst = composeSubst(testUnify.value, ctx.subst);
    }

    // Infer result
    const resultPath = [...path, 1 + i, 1];
    const exprResult = inferAndRecord(ctx, resultExpr, resultPath);
    if (!exprResult.ok) return exprResult;

    const clauseType = applySubst(ctx.subst, exprResult.value);

    if (resultType === undefined) {
      resultType = clauseType;
    } else {
      const branchUnify = unifyWithContext(resultType, clauseType, 'cond branches');
      if (!branchUnify.ok) return branchUnify;
      ctx.subst = composeSubst(branchUnify.value, ctx.subst);
      resultType = applySubst(ctx.subst, resultType);
    }
  }

  return ok(resultType || tUnit);
};

// Infer match
const inferMatchWithMap = (
  ctx: InferContext,
  elements: Expr[],
  path: NodePath
): TypeResult<Type> => {
  if (elements.length < 3) {
    return err({ kind: 'arity-mismatch', expected: 2, got: elements.length - 1, context: 'match' });
  }

  const scrutineeExpr = elements[1]!;
  const branches = elements.slice(2);

  // Infer scrutinee
  const scrutineePath = [...path, 1];
  const scrutineeResult = inferAndRecord(ctx, scrutineeExpr, scrutineePath);
  if (!scrutineeResult.ok) return scrutineeResult;

  let scrutineeType = scrutineeResult.value;
  let resultType: Type | undefined;

  for (let i = 0; i < branches.length; i++) {
    const branchExpr = branches[i]!;
    const branchElements = listToArray(branchExpr);
    if (branchElements.length !== 2) {
      return err({ kind: 'invalid-type-annotation', message: 'match branch must be (pattern body)' });
    }

    const patternExpr = branchElements[0]!;
    const bodyExpr = branchElements[1]!;

    // Parse and type check pattern
    const patternResult = parsePattern(patternExpr, ctx.env);
    if (!patternResult.ok) return patternResult;

    const patternCheck = inferPatternWithMap(ctx, patternResult.value, applySubst(ctx.subst, scrutineeType));
    if (!patternCheck.ok) return patternCheck;

    const patternUnify = unifyWithContext(
      applySubst(ctx.subst, scrutineeType),
      patternCheck.value.type,
      'match pattern'
    );
    if (!patternUnify.ok) return patternUnify;

    ctx.subst = composeSubst(patternUnify.value, composeSubst(patternCheck.value.subst, ctx.subst));
    scrutineeType = applySubst(ctx.subst, scrutineeType);

    // Extend environment with pattern bindings
    let branchEnv = applySubstEnv(ctx.subst, ctx.env);
    for (const [name, type] of patternCheck.value.bindings) {
      const scheme = { quantifiedVars: [], qualType: { constraints: [], type: applySubst(ctx.subst, type) } };
      branchEnv = extendEnv(branchEnv, name, scheme);
    }

    // Infer body
    const bodyCtx: InferContext = {
      env: branchEnv,
      subst: ctx.subst,
      types: ctx.types,
      constraints: [...ctx.constraints],
    };

    const bodyPath = [...path, 2 + i, 1];
    const bodyResult = inferAndRecord(bodyCtx, bodyExpr, bodyPath);
    if (!bodyResult.ok) return bodyResult;

    ctx.subst = bodyCtx.subst;
    ctx.constraints = bodyCtx.constraints;

    const branchType = applySubst(ctx.subst, bodyResult.value);

    if (resultType === undefined) {
      resultType = branchType;
    } else {
      const branchUnify = unifyWithContext(resultType, branchType, 'match branches');
      if (!branchUnify.ok) return branchUnify;
      ctx.subst = composeSubst(branchUnify.value, ctx.subst);
      resultType = applySubst(ctx.subst, resultType);
    }
  }

  return ok(resultType || tUnit);
};

// Infer pattern type and get bindings
const inferPatternWithMap = (
  ctx: InferContext,
  pattern: import('./type-ast').Pattern,
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
      const ctorInfo = lookupConstructor(ctx.env, pattern.name);
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

      // Instantiate with fresh type vars
      const typeVarMap = new Map<string, Type>();
      for (const tv of dataType.typeParams) {
        typeVarMap.set(tv, freshTypeVar());
      }

      const instantiatedResult = dataType.typeParams.length === 0
        ? typeConst(dataType.name)
        : typeApp(
            typeConst(dataType.name),
            dataType.typeParams.map((tv) => typeVarMap.get(tv)!)
          );

      return flatMap(unify(expectedType, instantiatedResult), (s1) => {
        let subst = s1;
        const allBindings = new Map<string, Type>();

        for (let i = 0; i < pattern.args.length; i++) {
          let paramType = constructor.params[i]!;
          for (const [tv, replacement] of typeVarMap) {
            paramType = applySubst(new Map([[tv, replacement]]), paramType);
          }
          paramType = applySubst(subst, paramType);

          const argResult = inferPatternWithMap(ctx, pattern.args[i]!, paramType);
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

// FFI variadic functions
const FFI_VARIADIC_FUNCTIONS = new Set(['ffi-call', 'struct-new', 'struct-get', 'struct-set', 'list']);

// Infer variadic arithmetic operators (+, -, *, /)
const inferVariadicArithApplicationWithMap = (
  ctx: InferContext,
  opName: string,
  argExprs: Expr[],
  path: NodePath
): TypeResult<Type> => {
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
    return ok(tInt);
  }

  const argTypes: Type[] = [];

  // Infer all argument types with path tracking
  for (let i = 0; i < argExprs.length; i++) {
    const argExpr = argExprs[i]!;
    const argPath = [...path, 1 + i];
    const argResult = inferAndRecord(ctx, argExpr, argPath);
    if (!argResult.ok) return argResult;
    argTypes.push(argResult.value);
  }

  // Unify all argument types together
  let unifiedType = applySubst(ctx.subst, argTypes[0]!);
  for (let i = 1; i < argTypes.length; i++) {
    const argType = applySubst(ctx.subst, argTypes[i]!);
    const unifyResult = unify(unifiedType, argType);
    if (!unifyResult.ok) return unifyResult;
    ctx.subst = composeSubst(unifyResult.value, ctx.subst);
    unifiedType = applySubst(ctx.subst, unifiedType);
  }

  // Add Num constraint for the unified type
  ctx.constraints.push({ class: 'Num', typeArg: unifiedType });

  return ok(unifiedType);
};

// Infer application
const inferApplicationWithMap = (
  ctx: InferContext,
  elements: Expr[],
  path: NodePath
): TypeResult<Type> => {
  const fnExpr = elements[0]!;
  const argExprs = elements.slice(1);

  const fnName = getSymbol(fnExpr);
  if (fnName && FFI_VARIADIC_FUNCTIONS.has(fnName)) {
    return inferFFIApplicationWithMap(ctx, fnName, argExprs, path);
  }

  if (fnName && VARIADIC_ARITH_OPS.has(fnName)) {
    return inferVariadicArithApplicationWithMap(ctx, fnName, argExprs, path);
  }

  // Infer function type
  const fnPath = [...path, 0];
  const fnResult = inferAndRecord(ctx, fnExpr, fnPath);
  if (!fnResult.ok) return fnResult;

  const argTypes: Type[] = [];

  // Infer argument types
  for (let i = 0; i < argExprs.length; i++) {
    const argExpr = argExprs[i]!;
    const argPath = [...path, 1 + i];
    const argResult = inferAndRecord(ctx, argExpr, argPath);
    if (!argResult.ok) return argResult;
    argTypes.push(argResult.value);
  }

  // Create result type variable
  const resultType = freshTypeVar();

  // Build expected function type
  const expectedFnType = typeArrow(argTypes.map((t) => applySubst(ctx.subst, t)), resultType);

  // Unify
  const fnType = applySubst(ctx.subst, fnResult.value);
  const appUnify = unifyWithContext(fnType, expectedFnType, 'function application');
  if (!appUnify.ok) return appUnify;

  ctx.subst = composeSubst(appUnify.value, ctx.subst);
  return ok(applySubst(ctx.subst, resultType));
};

// Infer FFI application
const inferFFIApplicationWithMap = (
  ctx: InferContext,
  fnName: string,
  argExprs: Expr[],
  path: NodePath
): TypeResult<Type> => {
  // Type check all arguments
  for (let i = 0; i < argExprs.length; i++) {
    const argExpr = argExprs[i]!;
    const argPath = [...path, 1 + i];
    const argResult = inferAndRecord(ctx, argExpr, argPath);
    if (!argResult.ok) return argResult;
  }

  // Return appropriate type
  switch (fnName) {
    case 'ffi-call':
      return ok(freshTypeVar());
    case 'struct-new':
      return ok(tFFIStruct);
    case 'struct-get':
      return ok(freshTypeVar());
    case 'struct-set':
      return ok(tUnit);
    case 'list':
      return ok(typeApp(typeConst('List'), [freshTypeVar()]));
    default:
      return ok(freshTypeVar());
  }
};

// Infer data definition
const inferDataDefWithMap = (
  ctx: InferContext,
  elements: Expr[],
  path: NodePath
): TypeResult<Type> => {
  if (elements.length < 3) {
    return err({ kind: 'arity-mismatch', expected: 2, got: elements.length - 1, context: 'data' });
  }

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
        const paramTypeResult = parseTypeExpr(ctorElements[i]!, ctx.env);
        if (!paramTypeResult.ok) return paramTypeResult;
        params.push(paramTypeResult.value);
      }

      constructors.push({ name: ctorName, params, resultType });
    }
  }

  // Define data type
  const dataType = { name: typeName, typeParams, constructors };
  ctx.env = defineDataType(ctx.env, dataType);

  // Add constructor schemes
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

    ctx.env = extendEnv(ctx.env, ctor.name, scheme);
  }

  return ok(tUnit);
};

// Main entry point: build type map for a program
export const buildTypeMap = (exprs: Expr[]): TypeMapResult => {
  resetTypeVarCounter();

  const ctx: InferContext = {
    env: createBuiltinEnv(),
    subst: emptySubst(),
    types: new Map(),
    constraints: [],
  };

  const topLevelTypes: Type[] = [];

  for (let i = 0; i < exprs.length; i++) {
    const expr = exprs[i]!;
    const path = [i];

    const result = inferAndRecord(ctx, expr, path);
    if (!result.ok) {
      finalizeTypes(ctx);
      return {
        ok: false,
        error: result.error,
        errorPath: path,
        types: ctx.types,
      };
    }

    topLevelTypes.push(result.value);

    // Resolve constraints
    const resolved = resolveConstraints(ctx.constraints);
    for (const c of resolved) {
      if (c.typeArg.kind !== 'type-var') {
        finalizeTypes(ctx);
        return {
          ok: false,
          error: { kind: 'no-instance', constraint: c },
          errorPath: path,
          types: ctx.types,
        };
      }
    }
  }

  // Finalize all types with final substitution
  finalizeTypes(ctx);

  return {
    ok: true,
    types: ctx.types,
    topLevelTypes: topLevelTypes.map((t) => applySubst(ctx.subst, t)),
  };
};
