# Marlang Development Guide

## Running

```bash
bun index.ts run <file.mar>              # Run with type checking (tree-walker)
bun index.ts run <file.mar> --vm         # Run with bytecode VM (faster)
bun index.ts run <file.mar> --no-typecheck  # Skip type checking
bun index.ts repl                        # Interactive REPL
bun index.ts typecheck <file.mar>        # Type check only
bun index.ts typecheck <file.mar> --verbose  # Type check with inferred types
bun index.ts disasm <file.mar>           # Disassemble to bytecode
bun bench/run.ts                         # Run benchmarks
```

## Testing

```bash
bun test                           # Run all tests
bun test spec/type-system.test.ts  # Run specific test file
```

Tests are in `spec/` directory using bun:test.

## Rust Port

The Rust port lives in `port/` with a Cargo workspace. Phases 1-7c complete (lexer, parser, type system, bytecode compiler, VM execution, modules, FFI, CLI, single-threaded scheduler, multi-threaded scheduler, async IO).

### Rust CLI

```bash
# Build the CLI (creates 'mar' binary)
cargo build --release -p marlang-cli

# Or run directly via cargo
cargo run -p marlang-cli -- run file.mar           # Run a file
cargo run -p marlang-cli -- run file.mar --no-typecheck  # Skip type checking
cargo run -p marlang-cli -- typecheck file.mar     # Type check only
cargo run -p marlang-cli -- typecheck file.mar --verbose  # Show inferred types
cargo run -p marlang-cli -- disasm file.mar        # Disassemble bytecode
cargo run -p marlang-cli -- repl                   # Interactive REPL
```

### Rust Tests

```bash
cargo test --workspace                # Run all Rust tests
cargo test -p marlang-core            # Core: lexer + parser (75 tests)
cargo test -p marlang-types           # Type system (81 tests)
cargo test -p marlang-vm              # VM: compiler + execution + modules + FFI + scheduler (271 tests)
```

### Rust Port Structure (`port/crates/`)
- **`core/`** (`marlang-core`) — `span.rs`, `intern.rs`, `ast.rs`, `lexer.rs`, `parser.rs`
- **`types/`** (`marlang-types`) — `ast.rs`, `env.rs`, `unify.rs`, `infer.rs`, `builtins.rs`, `checker.rs`, `errors.rs`
- **`vm/`** (`marlang-vm`) — `bytecode.rs`, `value.rs`, `object.rs`, `compiler.rs`, `vm.rs`, `builtins.rs`, `debug.rs`, `errors.rs`, `modules.rs`, `ffi.rs`, `scheduler.rs`, `runtime.rs`
- **`cli/`** (`marlang-cli`) — `main.rs` (clap-based CLI with run, repl, typecheck, disasm)

## Project Structure

```
src/
  core/       - Core pipeline (lexer, parser, tree-walker interpreter)
  vm/         - Bytecode VM (compiler, VM, builtins, debug)
  types/      - Type system
  modules/    - Module system
lsp/          - Language server
spec/         - Tests
stdlib/       - Standard library
bench/        - Benchmarks (tree-walker vs VM)
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

### Bytecode VM (`src/vm/`)
- `bytecode.ts` - Opcodes, instructions, chunks, VM value types (VMValue, VMClosure, VMContinuation, FFI types)
- `compiler.ts` - AST → bytecode compiler with flat closures and upvalue capture
- `vm.ts` - Stack-based VM with algebraic effect system (handler frames, continuations, default IO handlers)
- `builtins.ts` - Built-in globals for the VM (arithmetic, comparison, list, show, ADT constructors, FFI)
- `debug.ts` - Bytecode disassembler (formatInstruction, getAnnotation)
- `index.ts` - Public API exports (compileProgram, createVM, interpret, etc.)

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
Defined in `interpreter.ts` builtins array (tree-walker) and `builtins.ts` (VM):
- Arithmetic: `+`, `-`, `*`, `/`
- Comparison: `<`, `>`, `=`, `<=`, `>=`
- Lists: `list`
- FFI: `ffi-open`, `ffi-call`, `ffi-close` (both tree-walker and VM)
- Structs: `ffi-struct`, `struct-new`, `struct-get`, `struct-set` (both tree-walker and VM)
- Environment: `env-bindings`, `env-get`, `env-parent` (tree-walker only)

### Default IO Effect Handlers
Both tree-walker and VM provide built-in handlers for IO effects (no `handle` block needed):
- `print` — write string to stdout (no newline)
- `println` — write string to stdout with newline
- `read-line` — read a line from stdin
- `read-file` — read file contents as string
- `write-file` — write string to file

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
