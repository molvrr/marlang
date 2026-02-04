// Type error definitions and formatting

import type { Type, Constraint, Pattern } from './type-ast';
import { formatType, formatConstraint, formatPattern } from './type-ast';

export type TypeError =
  | { kind: 'unification-error'; t1: Type; t2: Type; context?: string }
  | { kind: 'occurs-check'; varName: string; type: Type }
  | { kind: 'undefined-variable'; name: string }
  | { kind: 'undefined-type'; name: string }
  | { kind: 'undefined-constructor'; name: string }
  | { kind: 'undefined-class'; name: string }
  | { kind: 'no-instance'; constraint: Constraint }
  | { kind: 'duplicate-definition'; name: string; what: string }
  | { kind: 'arity-mismatch'; expected: number; got: number; context: string }
  | { kind: 'not-a-function'; type: Type }
  | { kind: 'pattern-type-mismatch'; pattern: Pattern; expected: Type; got: Type }
  | { kind: 'non-exhaustive-patterns'; missing: string[] }
  | { kind: 'overlapping-instances'; className: string; type: Type }
  | { kind: 'missing-type-annotation'; name: string }
  | { kind: 'invalid-type-annotation'; message: string }
  | { kind: 'ambiguous-type'; varName: string; constraints: Constraint[] }
  | { kind: 'cyclic-type-alias'; name: string }
  | { kind: 'kind-error'; message: string }
  | { kind: 'internal-error'; message: string };

export const formatError = (err: TypeError): string => {
  switch (err.kind) {
    case 'unification-error': {
      let msg = `Type mismatch: cannot unify ${formatType(err.t1)} with ${formatType(err.t2)}`;
      if (err.context) {
        msg += `\n  in ${err.context}`;
      }
      return msg;
    }
    case 'occurs-check':
      return `Infinite type: ${err.varName} occurs in ${formatType(err.type)}`;
    case 'undefined-variable':
      return `Undefined variable: ${err.name}`;
    case 'undefined-type':
      return `Undefined type: ${err.name}`;
    case 'undefined-constructor':
      return `Undefined constructor: ${err.name}`;
    case 'undefined-class':
      return `Undefined typeclass: ${err.name}`;
    case 'no-instance':
      return `No instance for ${formatConstraint(err.constraint)}`;
    case 'duplicate-definition':
      return `Duplicate ${err.what}: ${err.name}`;
    case 'arity-mismatch':
      return `${err.context}: expected ${err.expected} arguments, got ${err.got}`;
    case 'not-a-function':
      return `Cannot apply non-function type: ${formatType(err.type)}`;
    case 'pattern-type-mismatch':
      return `Pattern ${formatPattern(err.pattern)} has type ${formatType(err.got)}, expected ${formatType(err.expected)}`;
    case 'non-exhaustive-patterns':
      return `Non-exhaustive patterns. Missing: ${err.missing.join(', ')}`;
    case 'overlapping-instances':
      return `Overlapping instances for ${err.className} ${formatType(err.type)}`;
    case 'missing-type-annotation':
      return `Missing type annotation for top-level definition: ${err.name}`;
    case 'invalid-type-annotation':
      return `Invalid type annotation: ${err.message}`;
    case 'ambiguous-type':
      return `Ambiguous type variable ${err.varName} with constraints: ${err.constraints.map(formatConstraint).join(', ')}`;
    case 'cyclic-type-alias':
      return `Cyclic type alias: ${err.name}`;
    case 'kind-error':
      return `Kind error: ${err.message}`;
    case 'internal-error':
      return `Internal type error: ${err.message}`;
  }
};

// Type checking result
export type TypeResult<T> =
  | { ok: true; value: T }
  | { ok: false; error: TypeError };

export const ok = <T>(value: T): TypeResult<T> => ({ ok: true, value });
export const err = <T>(error: TypeError): TypeResult<T> => ({ ok: false, error });

// Monadic helpers for TypeResult
export const map = <A, B>(result: TypeResult<A>, f: (a: A) => B): TypeResult<B> => {
  if (!result.ok) return result;
  return ok(f(result.value));
};

export const flatMap = <A, B>(
  result: TypeResult<A>,
  f: (a: A) => TypeResult<B>
): TypeResult<B> => {
  if (!result.ok) return result;
  return f(result.value);
};

export const mapError = <T>(
  result: TypeResult<T>,
  f: (e: TypeError) => TypeError
): TypeResult<T> => {
  if (result.ok) return result;
  return err(f(result.error));
};

// Combine multiple results
export const sequence = <T>(results: TypeResult<T>[]): TypeResult<T[]> => {
  const values: T[] = [];
  for (const r of results) {
    if (!r.ok) return r;
    values.push(r.value);
  }
  return ok(values);
};

// Try multiple alternatives, return first success
export const tryAll = <T>(
  alternatives: (() => TypeResult<T>)[],
  errorIfAllFail: TypeError
): TypeResult<T> => {
  for (const alt of alternatives) {
    const result = alt();
    if (result.ok) return result;
  }
  return err(errorIfAllFail);
};
