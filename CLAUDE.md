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

### Core Pipeline
- `lexer.ts` - Tokenizer (handles comments, strings, numbers, symbols)
- `parser.ts` - S-expression parser (builds cons-list AST)
- `types.ts` - AST types (Expr, LispValue) and runtime values (RuntimeValue)
- `interpreter.ts` - Tree-walking interpreter with TCO

### Type System
- `type-ast.ts` - Type representation (Type, TypeScheme, Constraint, Pattern)
- `type-env.ts` - Type environment, substitution, generalization
- `type-unify.ts` - Robinson's unification algorithm
- `type-infer.ts` - Constraint generation, inference, and exhaustiveness checking for match
- `type-builtins.ts` - Built-in types (Int, Float, Bool, String) and typeclasses (Num, Eq, Ord)
- `type-checker.ts` - Main entry point for type checking
- `type-errors.ts` - Error types and formatting

### Special Forms
Handled in `interpreter.ts`:
- `define`, `lambda`, `if`, `let`, `cond`, `begin`
- `data` - ADT definitions
- `match` - Pattern matching
- `class`, `instance`, `type` - Type system (no-op at runtime)
- `current-env` - Environment introspection

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
1. Add handling in `evalExprTail()` in `interpreter.ts`
2. Add type inference in `inferExpr()` in `type-infer.ts`
3. Add tests in `spec/`

### New Built-in Function
1. Add to `builtins` array in `interpreter.ts`
2. Add type scheme in `createBuiltinEnv()` in `type-builtins.ts`

### New Type
1. Add to `type-ast.ts`
2. Update `type-builtins.ts` if it's a built-in type
