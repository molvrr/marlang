# Marlang Development Guide

## Running

```bash
bun index.ts run <file.mar>              # Run with type checking
bun index.ts run <file.mar> --no-typecheck  # Skip type checking
bun index.ts repl                        # Interactive REPL
bun index.ts typecheck <file.mar>        # Type check only
bun index.ts typecheck <file.mar> --verbose  # Type check with inferred types
```

## Testing

```bash
bun test                           # Run all tests
bun test spec/type-system.test.ts  # Run specific test file
```

Tests are in `spec/` directory using bun:test.

## Project Structure

```
src/
  core/       - Core pipeline
  types/      - Type system
  modules/    - Module system
lsp/          - Language server
spec/         - Tests
stdlib/       - Standard library
web/          - Web playground
```

### Core Pipeline (`src/core/`)
- `lexer.ts` - Tokenizer (handles comments, strings, numbers, symbols)
- `parser.ts` - S-expression parser (builds cons-list AST)
- `types.ts` - AST types (Expr, LispValue) and runtime values (RuntimeValue)
- `interpreter.ts` - Tree-walking interpreter with TCO and effect system
- `io-handlers.ts` - Native IO effect handlers (auto-loaded in CLI)

### Type System (`src/types/`)
- `type-ast.ts` - Type representation (Type, TypeScheme, Constraint, Pattern)
- `type-env.ts` - Type environment, substitution, generalization
- `type-unify.ts` - Robinson's unification algorithm
- `type-infer.ts` - Constraint generation, inference, and exhaustiveness checking for match
- `type-builtins.ts` - Built-in types (Int, Float, Bool, String) and typeclasses (Num, Eq, Ord)
- `type-checker.ts` - Main entry point for type checking
- `type-errors.ts` - Error types and formatting

### Module System (`src/modules/`)
- `module-loader.ts` - Path resolution and file loading
- `module-types.ts` - Module context and cache types
- Uses `std:` prefix for stdlib imports (e.g., `(import "std:option" as opt)`)
- Custom prefixes can be registered via `registerPathPrefix()`

### Standard Library
Located in `stdlib/`:
- `option.mar` - Option type utilities (map, flat-map, unwrap-or, etc.)
- `result.mar` - Result type and utilities (Ok, Err, map, flat-map, etc.)
- `io.mar` - IO effect and operations (print, println, read-line, read-file, write-file)

### Special Forms
Handled in `interpreter.ts`:
- `define`, `lambda`, `if`, `let`, `cond`, `begin`
- `data` - ADT definitions
- `match` - Pattern matching
- `import` - Module imports
- `effect`, `perform`, `handle` - Algebraic effects
- `class`, `instance`, `type` - Type system (no-op at runtime)
- `current-env` - Environment introspection

### Effect Syntax
Effect operations use arrow type syntax:
```lisp
(effect IO
  (print (-> String Unit))
  (read-line (-> Unit String))
  (write-file (-> String String Unit)))  ; Multi-argument
```

### Built-in Functions
Defined in `interpreter.ts` builtins array:
- Arithmetic: `+`, `-`, `*`, `/`
- Comparison: `<`, `>`, `=`, `<=`, `>=`
- Lists: `list`
- FFI: `ffi-open`, `ffi-call`, `ffi-close`
- Structs: `ffi-struct`, `struct-new`, `struct-get`, `struct-set`
- Environment: `env-bindings`, `env-get`, `env-parent`

## Code Conventions

- Use Bun APIs (not Node.js)
- Immutable data structures where practical
- `kind` field discriminator for union types
- Namespaces for related functions (e.g., `Environment.lookup`)

## Adding Features

### New Special Form
1. Add handling in `evalExprTail()` in `src/core/interpreter.ts`
2. Add type inference in `inferExpr()` in `src/types/type-infer.ts`
3. Add tests in `spec/`

### New Built-in Function
1. Add to `builtins` array in `src/core/interpreter.ts`
2. Add type scheme in `createBuiltinEnv()` in `src/types/type-builtins.ts`

### New Type
1. Add to `src/types/type-ast.ts`
2. Update `src/types/type-builtins.ts` if it's a built-in type

### New Stdlib Module
1. Create `stdlib/module-name.mar`
2. Add tests in `spec/stdlib.test.ts`
3. Users import via `(import "std:module-name" as alias)`

### New Import Prefix
1. In TypeScript: `registerPathPrefix('prefix', (name, fromFile) => resolvedPath)`
2. Users import via `(import "prefix:module" as alias)`
