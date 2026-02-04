// Module system types
// Separate file to avoid circular dependencies between types.ts and type-env.ts

import type { RuntimeValue } from '../core/types';
import type { TypeEnv } from '../types/type-env';

/**
 * Exports from a loaded module.
 */
export type ModuleExports = {
  /** Runtime bindings exported by the module */
  bindings: Map<string, RuntimeValue>;
  /** Type environment with type definitions */
  typeEnv: TypeEnv;
};

/**
 * State of a module in the cache.
 * 'loading' is used to detect circular dependencies.
 */
export type ModuleState =
  | { status: 'loading' }
  | { status: 'loaded'; exports: ModuleExports };

/**
 * Cache of loaded modules, keyed by absolute file path.
 */
export type ModuleCache = Map<string, ModuleState>;

/**
 * Context for module resolution during evaluation.
 */
export type ModuleContext = {
  /** Absolute path of the current file, or null for REPL/eval */
  filePath: string | null;
  /** Shared cache of loaded modules */
  cache: ModuleCache;
};

/**
 * Create an empty module context for REPL or programmatic evaluation.
 */
export const emptyModuleContext = (): ModuleContext => ({
  filePath: null,
  cache: new Map(),
});

/**
 * Create a module context for a specific file.
 */
export const fileModuleContext = (filePath: string): ModuleContext => ({
  filePath,
  cache: new Map(),
});
