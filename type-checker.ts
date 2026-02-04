// Main type checker entry point

import type { Expr } from './types';
import type { Type, TypeScheme } from './type-ast';
import { formatType, formatScheme } from './type-ast';
import type { TypeEnv } from './type-env';
import { resetTypeVarCounter, lookupVar, applySubst } from './type-env';
import { createBuiltinEnv } from './type-builtins';
import { inferProgram } from './type-infer';
import type { TypeError } from './type-errors';
import { formatError } from './type-errors';

// Type checking result
export type TypeCheckResult =
  | { ok: true; env: TypeEnv; types: Type[] }
  | { ok: false; error: TypeError };

// Type check a program
export const typeCheck = (exprs: Expr[]): TypeCheckResult => {
  // Reset type variable counter for fresh inference
  resetTypeVarCounter();

  // Create initial environment with built-in types
  const env = createBuiltinEnv();

  // Run type inference
  const result = inferProgram(env, exprs);

  if (!result.ok) {
    return { ok: false, error: result.error };
  }

  return { ok: true, env: result.value.env, types: result.value.types };
};

// Type check and return formatted error message if failed
export const typeCheckWithMessage = (exprs: Expr[]): { ok: boolean; message?: string } => {
  const result = typeCheck(exprs);

  if (!result.ok) {
    return { ok: false, message: formatError(result.error) };
  }

  return { ok: true };
};

// Get the type of a variable in the result environment
export const getTypeOf = (result: TypeCheckResult, name: string): TypeScheme | undefined => {
  if (!result.ok) return undefined;
  return lookupVar(result.env, name);
};

// Format the inferred types for display
export const formatTypes = (result: TypeCheckResult): string[] => {
  if (!result.ok) return [];
  return result.types.map(formatType);
};

// Type check and print results (for REPL)
export const typeCheckAndPrint = (exprs: Expr[]): void => {
  const result = typeCheck(exprs);

  if (!result.ok) {
    console.error(`Type error: ${formatError(result.error)}`);
    return;
  }

  // Print the type of the last expression if not Unit
  const lastType = result.types[result.types.length - 1];
  if (lastType && !(lastType.kind === 'type-const' && lastType.name === 'Unit')) {
    console.log(`  : ${formatType(lastType)}`);
  }
};

// Check if expressions have a specific expected type
export const checkType = (
  exprs: Expr[],
  expectedType: Type
): TypeCheckResult => {
  const result = typeCheck(exprs);

  if (!result.ok) {
    return result;
  }

  // Get the type of the last expression
  const lastType = result.types[result.types.length - 1];
  if (!lastType) {
    return result;
  }

  // TODO: Could add unification check here to verify against expected type

  return result;
};

// Export for use in other modules
export { formatError, formatType, formatScheme };
