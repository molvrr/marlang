<<<<<<< Conflict 1 of 1
+++++++ Contents of side #1
%%%%%%% Changes from base to side #2
 // Module loader - path resolution and file loading for imports
 
 import * as path from 'path';
 import * as fs from 'fs';
 import * as Lexer from '../core/lexer';
 import * as Parser from '../core/parser';
 import type { Expr } from '../core/types';
 
 // ===== Virtual Module System =====
 // Allows registering module source code directly, for browser support
 
 /**
  * Registry of virtual modules.
  * Key is the resolved absolute path, value is the source code.
  */
 const virtualModules = new Map<string, string>();
 
 /**
  * Register a virtual module with its source code.
  * Virtual modules take precedence over file system modules.
  *
  * @param absolutePath - The absolute path this module should resolve to
  * @param source - The source code of the module
  */
 export const registerVirtualModule = (absolutePath: string, source: string): void => {
   virtualModules.set(absolutePath, source);
 };
 
 /**
  * Unregister a virtual module.
  */
 export const unregisterVirtualModule = (absolutePath: string): boolean => {
   return virtualModules.delete(absolutePath);
 };
 
 /**
  * Check if a virtual module exists.
  */
 export const hasVirtualModule = (absolutePath: string): boolean => {
   return virtualModules.has(absolutePath);
 };
 
 /**
  * Get source from a virtual module.
  */
 export const getVirtualModuleSource = (absolutePath: string): string | undefined => {
   return virtualModules.get(absolutePath);
 };
 
 // ===== Path Prefix Resolvers =====
 
 /**
  * A path resolver takes a module name (after the prefix) and returns an absolute path.
  * Return null if the resolver cannot handle this path.
  */
 export type PathResolver = (moduleName: string, fromFile: string) => string | null;
 
 /**
  * Registry of path prefix resolvers.
  * Key is the prefix (e.g., "std"), value is the resolver function.
  *
  * To add a new prefix, use `registerPathPrefix`:
  * ```typescript
  * registerPathPrefix('mylib', (name) => path.join('/path/to/mylib', name));
  * ```
  */
 const pathPrefixResolvers = new Map<string, PathResolver>();
 
 /**
  * Register a new path prefix resolver.
  *
  * @param prefix - The prefix without the colon (e.g., "std" for "std:option")
  * @param resolver - Function that resolves module name to absolute path
  *
  * @example
  * // Register a custom library prefix
  * registerPathPrefix('mylib', (name) => path.join('/path/to/mylib', name));
  *
  * // Now you can use: (import "mylib:utils" as u)
  */
 export const registerPathPrefix = (prefix: string, resolver: PathResolver): void => {
   pathPrefixResolvers.set(prefix, resolver);
 };
 
 /**
  * Unregister a path prefix resolver.
  */
 export const unregisterPathPrefix = (prefix: string): boolean => {
   return pathPrefixResolvers.delete(prefix);
 };
 
 /**
  * Get all registered path prefixes.
  */
 export const getRegisteredPrefixes = (): string[] => {
   return Array.from(pathPrefixResolvers.keys());
 };
 
 // ===== Built-in Resolvers =====
 
-// Get the Marlang root directory (project root, two levels up from src/modules/)
+// Get the Marlang root directory
+// Priority: MARLANG_HOME env var > ~/.marlang > source directory (dev mode)
 const getMarlangRoot = (): string => {
+  // 1. Check for explicit env var
+  const envHome = process.env.MARLANG_HOME;
+  if (envHome) {
+    return path.resolve(envHome);
+  }
+
+  // 2. Check ~/.marlang (standard installation location)
+  const homeDir = process.env.HOME || process.env.USERPROFILE || '';
+  if (homeDir) {
+    const userMarlang = path.join(homeDir, '.marlang');
+    const userStdlib = path.join(userMarlang, 'stdlib');
+    if (fs.existsSync(userStdlib)) {
+      return userMarlang;
+    }
+  }
+
+  // 3. Fall back to source directory (dev mode)
   return path.resolve(import.meta.dir, '../..');
 };
 
 // Standard library resolver
 const stdResolver: PathResolver = (moduleName: string, _fromFile: string): string => {
   return path.join(getMarlangRoot(), 'stdlib', moduleName);
 };
 
 // Register the built-in "std" prefix
 registerPathPrefix('std', stdResolver);
 
 // ===== Main Path Resolution =====
 
 /**
  * Resolve a module import path to an absolute file path.
  *
  * Supports the following import path formats:
  * - "prefix:module" - uses registered prefix resolver (e.g., "std:option")
  * - "./relative/path" - relative to the importing file
  * - "/absolute/path" - absolute path
  *
  * @param importPath - The path from the import statement
  * @param fromFile - The absolute path of the file containing the import
  * @returns The resolved absolute path
  */
 // Browser-compatible path helpers
 const isAbsolutePath = (p: string): boolean => {
   return p.startsWith('/') || /^[A-Za-z]:[\\/]/.test(p);
 };
 
 const getDirectory = (p: string): string => {
   const lastSlash = p.lastIndexOf('/');
   if (lastSlash === -1) return '.';
   if (lastSlash === 0) return '/';
   return p.substring(0, lastSlash);
 };
 
 const joinPaths = (base: string, relative: string): string => {
   if (base.endsWith('/')) {
     return base + relative;
   }
   return base + '/' + relative;
 };
 
 const normalizePath = (p: string): string => {
   // Simple normalization: remove double slashes and resolve . and ..
   const parts = p.split('/').filter(part => part !== '' && part !== '.');
   const result: string[] = [];
   for (const part of parts) {
     if (part === '..') {
       result.pop();
     } else {
       result.push(part);
     }
   }
   return (p.startsWith('/') ? '/' : '') + result.join('/');
 };
 
 export const resolveModulePath = (importPath: string, fromFile: string): string => {
   let resolved: string;
 
   // Check for prefix patterns (e.g., "std:option", "mylib:utils")
   const prefixMatch = importPath.match(/^([a-zA-Z][a-zA-Z0-9_-]*):(.+)$/);
   if (prefixMatch) {
     const [, prefix, moduleName] = prefixMatch;
     const resolver = pathPrefixResolvers.get(prefix!);
     if (resolver) {
       const resolvedPath = resolver(moduleName!, fromFile);
       if (resolvedPath) {
         resolved = resolvedPath;
       } else {
         throw new Error(`Prefix resolver '${prefix}' could not resolve: ${moduleName}`);
       }
     } else {
       throw new Error(`Unknown import prefix '${prefix}'. Registered prefixes: ${getRegisteredPrefixes().join(', ') || '(none)'}`);
     }
   } else if (isAbsolutePath(importPath)) {
     // Absolute path - use as-is
     resolved = importPath;
   } else {
     // Relative path - resolve from importing file's directory
     const fromDir = getDirectory(fromFile);
     resolved = joinPaths(fromDir, importPath);
   }
 
   // Normalize the path
   resolved = normalizePath(resolved);
 
   // Auto-append .mar extension if missing
   if (!resolved.endsWith('.mar')) {
     resolved = resolved + '.mar';
   }
 
   return resolved;
 };
 
 /**
  * Load module source code from an absolute file path.
  * Checks virtual modules first, then falls back to file system.
  *
  * @param absolutePath - The absolute path to the module file
  * @returns The source code as a string
  * @throws Error if file cannot be read
  */
 export const loadModuleSource = (absolutePath: string): string => {
   // Check virtual modules first (for browser support)
   const virtualSource = getVirtualModuleSource(absolutePath);
   if (virtualSource !== undefined) {
     return virtualSource;
   }
 
   // Fall back to file system
   try {
     return fs.readFileSync(absolutePath, 'utf-8');
   } catch (e) {
     if (e instanceof Error && 'code' in e && e.code === 'ENOENT') {
       throw new Error(`Module not found: ${absolutePath}`);
     }
     throw new Error(`Failed to load module ${absolutePath}: ${e instanceof Error ? e.message : String(e)}`);
   }
 };
 
 /**
  * Parse module source code into expressions.
  *
  * @param source - The source code string
  * @returns Array of parsed expressions
  */
 export const parseModule = (source: string): Expr[] => {
   const tokens = Lexer.lex(Lexer.empty(source));
   const parsed = Parser.parse(Parser.empty(tokens));
   return parsed.exprs;
 };
 
 /**
  * Load and parse a module from an absolute file path.
  *
  * @param absolutePath - The absolute path to the module file
  * @returns Array of parsed expressions
  */
 export const loadAndParseModule = (absolutePath: string): Expr[] => {
   const source = loadModuleSource(absolutePath);
   return parseModule(source);
 };
>>>>>>> Conflict 1 of 1 ends
