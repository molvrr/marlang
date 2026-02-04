// Type error definitions and formatting

import type { Type, Constraint, Pattern } from './type-ast';
import { formatType, formatConstraint, formatPattern } from './type-ast';
import type { SourceLocation } from './types';

// Base type for errors with optional location
type WithLocation = { loc?: SourceLocation };

export type TypeError =
  | { kind: 'unification-error'; t1: Type; t2: Type; context?: string } & WithLocation
  | { kind: 'occurs-check'; varName: string; type: Type } & WithLocation
  | { kind: 'undefined-variable'; name: string } & WithLocation
  | { kind: 'undefined-type'; name: string } & WithLocation
  | { kind: 'undefined-constructor'; name: string } & WithLocation
  | { kind: 'undefined-class'; name: string } & WithLocation
  | { kind: 'no-instance'; constraint: Constraint } & WithLocation
  | { kind: 'duplicate-definition'; name: string; what: string } & WithLocation
  | { kind: 'arity-mismatch'; expected: number; got: number; context: string } & WithLocation
  | { kind: 'not-a-function'; type: Type } & WithLocation
  | { kind: 'pattern-type-mismatch'; pattern: Pattern; expected: Type; got: Type } & WithLocation
  | { kind: 'non-exhaustive-patterns'; missing: string[] } & WithLocation
  | { kind: 'overlapping-instances'; className: string; type: Type } & WithLocation
  | { kind: 'missing-type-annotation'; name: string } & WithLocation
  | { kind: 'invalid-type-annotation'; message: string } & WithLocation
  | { kind: 'ambiguous-type'; varName: string; constraints: Constraint[] } & WithLocation
  | { kind: 'cyclic-type-alias'; name: string } & WithLocation
  | { kind: 'kind-error'; message: string } & WithLocation
  | { kind: 'internal-error'; message: string } & WithLocation;

// Format location for display
export const formatLocation = (loc: SourceLocation): string => {
  return `${loc.start.line}:${loc.start.column}`;
};

export const formatError = (err: TypeError): string => {
  const locPrefix = err.loc ? `${formatLocation(err.loc)}: ` : '';

  switch (err.kind) {
    case 'unification-error': {
      let msg = `${locPrefix}Type mismatch: cannot unify ${formatType(err.t1)} with ${formatType(err.t2)}`;
      if (err.context) {
        msg += `\n  in ${err.context}`;
      }
      return msg;
    }
    case 'occurs-check':
      return `${locPrefix}Infinite type: ${err.varName} occurs in ${formatType(err.type)}`;
    case 'undefined-variable':
      return `${locPrefix}Undefined variable: ${err.name}`;
    case 'undefined-type':
      return `${locPrefix}Undefined type: ${err.name}`;
    case 'undefined-constructor':
      return `${locPrefix}Undefined constructor: ${err.name}`;
    case 'undefined-class':
      return `${locPrefix}Undefined typeclass: ${err.name}`;
    case 'no-instance':
      return `${locPrefix}No instance for ${formatConstraint(err.constraint)}`;
    case 'duplicate-definition':
      return `${locPrefix}Duplicate ${err.what}: ${err.name}`;
    case 'arity-mismatch':
      return `${locPrefix}${err.context}: expected ${err.expected} arguments, got ${err.got}`;
    case 'not-a-function':
      return `${locPrefix}Cannot apply non-function type: ${formatType(err.type)}`;
    case 'pattern-type-mismatch':
      return `${locPrefix}Pattern ${formatPattern(err.pattern)} has type ${formatType(err.got)}, expected ${formatType(err.expected)}`;
    case 'non-exhaustive-patterns':
      return `${locPrefix}Non-exhaustive patterns. Missing: ${err.missing.join(', ')}`;
    case 'overlapping-instances':
      return `${locPrefix}Overlapping instances for ${err.className} ${formatType(err.type)}`;
    case 'missing-type-annotation':
      return `${locPrefix}Missing type annotation for top-level definition: ${err.name}`;
    case 'invalid-type-annotation':
      return `${locPrefix}Invalid type annotation: ${err.message}`;
    case 'ambiguous-type':
      return `${locPrefix}Ambiguous type variable ${err.varName} with constraints: ${err.constraints.map(formatConstraint).join(', ')}`;
    case 'cyclic-type-alias':
      return `${locPrefix}Cyclic type alias: ${err.name}`;
    case 'kind-error':
      return `${locPrefix}Kind error: ${err.message}`;
    case 'internal-error':
      return `${locPrefix}Internal type error: ${err.message}`;
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
