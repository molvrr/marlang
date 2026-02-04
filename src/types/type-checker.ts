// Main type checker entry point

import type { Expr } from '../core/types';
import type { Type } from './type-ast';
import { formatType, formatScheme } from './type-ast';
import type { TypeEnv } from './type-env';
import { resetTypeVarCounter } from './type-env';
import { createBuiltinEnv } from './type-builtins';
import { inferProgram } from './type-infer';
import type { TypeError } from './type-errors';
import { formatError } from './type-errors';

// Type checking result
export type TypeCheckResult =
  | { ok: true; env: TypeEnv; types: Type[] }
  | { ok: false; error: TypeError };

// Type check a program
export const typeCheck = (exprs: Expr[], filePath?: string): TypeCheckResult => {
  // Reset type variable counter for fresh inference
  resetTypeVarCounter();

  // Create initial environment with built-in types
  const env = createBuiltinEnv();

  // Run type inference
  const result = inferProgram(env, exprs, filePath);

  if (!result.ok) {
    return { ok: false, error: result.error };
  }

  return { ok: true, env: result.value.env, types: result.value.types };
};

// Format the inferred types for display
export const formatTypes = (result: TypeCheckResult): string[] => {
  if (!result.ok) return [];
  return result.types.map(formatType);
};

// Export for use in other modules
export { formatError, formatType, formatScheme };
