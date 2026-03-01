# Marlang Development Guide

## Running

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

## Testing

```bash
cargo test --workspace                # Run all tests
cargo test -p marlang-core            # Core: lexer + parser
cargo test -p marlang-types           # Type system
cargo test -p marlang-vm              # VM: compiler + execution + modules + FFI + scheduler + macros + TCP + Exception + Variadic + CPS handler dispatch
```

### Running .mar Programs

When running `.mar` files to verify changes, **always test both with and without type checking**:

```bash
cargo run -p marlang-cli -- run file.mar                # With type checking
cargo run -p marlang-cli -- run file.mar --no-typecheck  # Without type checking
```

The type checker and the VM have separate code paths (macro expansion fallbacks, variable/constructor resolution, etc.), so a program can succeed in one mode but fail in the other.

## Project Structure

```
port/crates/
  core/       - marlang-core: lexer, parser, string interning, spans
  types/      - marlang-types: HM type inference, typeclasses, exhaustiveness
  vm/         - marlang-vm: compiler, NaN-boxed VM, modules, FFI, schedulers, macros, TCP
  cli/        - marlang-cli: clap-based CLI (run, repl, typecheck, disasm)
stdlib/       - Standard library (.mar files, shared)
examples/     - Example programs
bench/        - Benchmark .mar programs
web/          - Web playground (currently broken — needs WASM port, see web/TODO.md)
ts-archive/   - Archived TypeScript implementation (historical reference)
```

### Crate Structure (`port/crates/`)
- **`core/`** (`marlang-core`) — `span.rs`, `intern.rs`, `ast.rs`, `lexer.rs`, `parser.rs`
- **`types/`** (`marlang-types`) — `ast.rs`, `env.rs`, `unify.rs`, `infer.rs`, `builtins.rs`, `checker.rs`, `errors.rs`
- **`vm/`** (`marlang-vm`) — `bytecode.rs`, `value.rs`, `object.rs`, `compiler.rs`, `vm.rs`, `builtins.rs`, `debug.rs`, `errors.rs`, `modules.rs`, `ffi.rs`, `scheduler.rs`, `runtime.rs`, `expand.rs`
- **`cli/`** (`marlang-cli`) — `main.rs` (clap-based CLI with run, repl, typecheck, disasm)

### Standard Library
Located in `stdlib/`:
- `prelude.mar` - Auto-loaded prelude: data types (Option, List, Pair, Result), `when` macro, utility functions (not, cons, car, cdr, append, string->number)
- `option.mar` - Option type utilities (map, flat-map, unwrap-or, etc.)
- `result.mar` - Result type and utilities (Ok, Err, map, flat-map, etc.)
- `io.mar` - IO effect and operations (print, println, read-line, read-file, write-file)
- `list.mar` - List utilities (foldl, foldr, map, filter, zip, etc.)
- `string.mar` - String utilities
- `math.mar` - Math utilities
- `concurrent.mar` - Concurrency wrappers (fibers, channels)
- `actor.mar` - Actor model (mailboxes, supervision)

Note: ADT constructors (None, Some, Nil, Cons, MkPair, Ok, Err) and utility functions (not, cons, car, cdr, append, string->number) are defined in `stdlib/prelude.mar`, not as Rust builtins. The prelude is auto-loaded before every program.

## Language Conventions

### Function Type Convention
Every function takes at least one argument. Zero-argument thunks (`(lambda () body)`) are typed as taking an implicit `Unit` parameter:
```lisp
;; Thunk: (lambda () ...) has type (-> Unit ReturnType)
(define (f : (-> Unit Unit)) (lambda () (perform println "hi")))

;; (-> X) is invalid — arrow types always need at least param + result
;; (-> {IO} Unit) is also invalid — the effect row is not a parameter
```

At the type level, `(lambda () body)` infers as `(-> Unit body_type)`. Calling a thunk with zero args `(f)` is accepted when the function expects `Unit`. Effect operations and FFI declarations follow the same rule — zero-arg ops use `(-> Unit ResultType)`.

### Effect Syntax
Effect operations use arrow type syntax:
```lisp
(effect IO
  (print (-> String Unit))
  (read-line (-> Unit String))
  (write-file (-> String String Unit)))  ; Multi-argument
```

### Effect Row Annotations
The type checker supports row-polymorphic effect annotations on arrow types using `{...}` syntax:
```lisp
;; Pure function (no effects)
(define (f : (-> String {} String)) (lambda (s) s))

;; Function with IO effect
(define (g : (-> String {IO} Unit)) (lambda (s) (perform println s)))

;; Open effect row (row-polymorphic): IO + any other effects
(define (h : (-> String {IO | e} Unit)) (lambda (s) (perform println s)))
```

Effect rows are enforced during type checking:
- Pure lambdas get `{}` (closed empty row)
- Effectful lambdas get their effects collected: `{IO}`, `{Exception}`, etc.
- `handle` subtracts handled effects from the row
- Omitting the effect row in a type annotation (e.g. `(-> String String)`) means pure — same as `{}`
- A `{}` annotation rejects effectful bodies
- Open rows `{IO | e}` allow additional effects via the row variable `e`

### Built-in Functions
Defined in `vm/builtins.rs`:
- Arithmetic: `+`, `-`, `*`, `/`, `negate`, `rem`, `abs`, `floor`, `ceil`, `round`, `sqrt`, `pow`, `log`, `sin`, `cos`, `tan`, `atan2`, `exp`, `float->int`, `int->float`
- Comparison: `<`, `>`, `=`, `<=`, `>=`
- Lists: `list`
- Strings: `show`, `string-length`, `string-append`, `number->string`, `__try-string->number`, `string-concat`, `string-is-empty`, `string-reverse`, `string-repeat`, `string-to-upper`, `string-to-lower`, `string-trim`, `string-starts-with`, `string-ends-with`, `string-contains`, `string-index-of`, `string-substring`, `string-char-at`, `string-split`, `string-join`, `string-replace`, `string-replace-all`, `string-pad-start`, `string-pad-end`, `char-code`, `char-from-code`, `string-trim-start`, `string-trim-end`
- FFI: `ffi-open`, `ffi-call`, `ffi-close`
- Structs: `ffi-struct`, `struct-new`, `struct-get`, `struct-set`
- Macros: `make-symbol`, `gensym`, `symbol->string`, `string->symbol`
- Type predicates: `symbol?`, `pair?`, `nil?`, `number?`, `string?`, `list?`

### Default IO Effect Handlers
The VM provides built-in handlers for IO effects (no `handle` block needed):
- `print` — write string to stdout (no newline)
- `println` — write string to stdout with newline
- `read-line` — read a line from stdin
- `read-file` — returns `(Result String String)` — `(Ok content)` or `(Err msg)` on IO error
- `write-file` — returns `(Result String Unit)` — `(Ok nil)` or `(Err msg)` on IO error

### Failable Operations Convention
Operations that can fail at runtime follow a consistent pattern:
- **Raw effect**: returns `(Result String a)` or `(Option a)` — caller matches on result
- **Throwing wrapper** (name ends with `!`): unwraps the Result and does `(perform throw msg)` on error via the Exception effect
- Examples: `chan-send` / `chan-send!`, `read-file` / `read-file!`, `write-file` / `write-file!`, TCP `try-connect` / `connect`

### Exception Effect
The `Exception` effect is declared in the prelude with a polymorphic return type:
```lisp
(effect Exception
  (throw (-> String a)))
```
The `a` return type allows `(perform throw msg)` to unify with any expected type (since it never actually returns). Without a handler, `throw` crashes the fiber. Use `handle` to catch exceptions:
```lisp
(handle (string->number "abc")
  ((throw msg k) -> (perform println (string-append "caught: " msg))))
```

### string->number
`string->number` is defined in the prelude as a wrapper around the `__try-string->number` builtin. On parse failure, it does `(perform throw ...)`. The raw builtin `__try-string->number` returns `(Option Float)` — `(Some n)` on success, `None` on failure.

## Known Issues

1. **Module globals workaround:** Qualified imports (`import "mod" as m`) install both prefixed (`m/foo`) and unqualified (`foo`) names into parent globals. This is needed because module closures reference siblings by unprefixed names via `PUSH_GLOBAL`. However, first-imported module wins for name collisions. Long-term fix: module-scoped globals.

2. **Recursive `define` + TCP effects hangs:** Using `(define f (lambda () ... (f)))` inside `fiber-spawn` where the body does TCP effects causes hangs. The `define` sets a global, but after TCP suspend/resume the `PUSH_GLOBAL` may not resolve. Workaround: unroll recursion or avoid combining recursive `define` with scheduler-suspending effects.

3. **`match` branches require single expressions:** Multi-expression match bodies need explicit `(begin ...)` wrapping. The compiler enforces `(pattern body)` with exactly 2 elements per branch.

## Adding Features

### New Special Form
1. Add compilation in `compiler.rs` (`compile_expr` match arm)
2. Add VM execution in `vm.rs` if needed (new opcode)
3. Add type inference in `types/infer.rs` (`infer_expr`)
4. Add tests via `cargo test`

### New Built-in Function
1. Add to `vm/builtins.rs` (`register_builtins`)
2. Add type scheme in `types/builtins.rs` (`builtin_env`)

### New Stdlib Module
1. Create `stdlib/module-name.mar`
2. Add tests in `port/tests/` or as Rust unit tests
3. Users import via `(import "std:module-name" as alias)`

## Archived TypeScript Implementation

The original TypeScript implementation (which ran on Bun) is preserved in `ts-archive/` for historical reference. It includes the tree-walking interpreter, bytecode VM, type system, LSP server, and test suite.
