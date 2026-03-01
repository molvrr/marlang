# Marlang Development Guide

## Running

```bash
# Build the CLI (creates 'mar' binary)
cargo build --release -p marlang-cli

# Or run directly via cargo
cargo run -p marlang-cli -- run file.mar           # Run a file
cargo run -p marlang-cli -- typecheck file.mar     # Type check only
cargo run -p marlang-cli -- typecheck file.mar --verbose  # Show inferred types
cargo run -p marlang-cli -- disasm file.mar        # Disassemble bytecode
cargo run -p marlang-cli -- repl                   # Interactive REPL
```

**Important:** Type checking is mandatory — there is no `--no-typecheck` flag. Programs must pass the type checker before they can run.

## Testing

```bash
cargo test --workspace                # Run all tests
cargo test -p marlang-core            # Core: lexer + parser
cargo test -p marlang-types           # Type system
cargo test -p marlang-vm              # VM: compiler + execution + modules + FFI + scheduler + macros + TCP + Exception + Variadic + CPS handler dispatch
```

## Project Structure

```
port/crates/
  core/       - marlang-core: lexer, parser, string interning, spans
  types/      - marlang-types: HM type inference, exhaustiveness
  vm/         - marlang-vm: compiler, NaN-boxed VM, modules, FFI, schedulers, macros, TCP
  cli/        - marlang-cli: clap-based CLI (run, repl, typecheck, disasm)
stdlib/       - Standard library (.mar files, shared)
examples/     - Example programs
bench/        - Benchmark .mar programs
web/          - Web playground (currently broken — needs WASM port, see web/TODO.md)
ts-archive/   - Archived TypeScript implementation (historical reference)
```

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

### Effects
Effect operations use arrow type syntax. The type checker supports row-polymorphic effect annotations using `{...}`:
```lisp
(effect IO
  (print (-> String Unit))
  (read-line (-> Unit String))
  (write-file (-> String String Unit)))

;; Pure function (no effects)
(define (f : (-> String {} String)) (lambda (s) s))

;; Function with IO effect
(define (g : (-> String {IO} Unit)) (lambda (s) (perform println s)))

;; Open effect row (row-polymorphic): IO + any other effects
(define (h : (-> String {IO | e} Unit)) (lambda (s) (perform println s)))
```

Effect row rules:
- Pure lambdas get `{}` (closed empty row); effectful lambdas get effects collected: `{IO}`, `{Exception}`, etc.
- `handle` subtracts handled effects from the row
- Omitting the effect row (e.g. `(-> String String)`) means pure — same as `{}`
- Open rows `{IO | e}` allow additional effects via the row variable `e`

The VM handles IO effects (`print`, `println`, `read-line`, `read-file`, `write-file`) without needing explicit `handle` blocks.

### Built-in Functions
Defined in `vm/builtins.rs`. Categories: Int arithmetic (`+`, `-`, `*`, `/`, `rem`, `abs`, `negate`), Float arithmetic (`+.`, `-.`, `*.`, `/.`, `rem.`, `abs.`, `negate.`), Int comparison (`<`, `>`, `=`, `<=`, `>=`), Float comparison (`<.`, `>.`, `=.`, `<=.`, `>=.`), string comparison (`string=`, `string<`, `string>`, `string<=`, `string>=`), polymorphic (`equal?`, `show`, `int->float`), math (`sqrt`, `pow`, `floor`, `ceil`, `round`, `truncate`, `exp`, `log`, `sin`, `cos`, `tan`, etc.), lists (`list`), strings (`string-length`, `string-append`, `string-split`, `string-join`, etc.), FFI (`ffi-open`, `ffi-call`, `ffi-close`), structs (`struct-new`, `struct-get`, `struct-set`), macros (`gensym`, `symbol->string`, `string->symbol`), type predicates (`symbol?`, `pair?`, `nil?`, `number?`, `string?`, `list?`). See `vm/builtins.rs` for the full list.

### Failable Operations Convention
Operations that can fail follow a consistent pattern:
- **Raw effect**: returns `(Result String a)` or `(Option a)` — caller matches on result
- **Throwing wrapper** (name ends with `!`): unwraps the Result and does `(perform throw msg)` on error
- Examples: `chan-send` / `chan-send!`, `read-file` / `read-file!`, `write-file` / `write-file!`

### Exception Effect
Declared in the prelude with a polymorphic return type so `throw` unifies with any expected type:
```lisp
(effect Exception
  (throw (-> String a)))

(handle (string->number "abc")
  ((throw msg k) -> (perform println (string-append "caught: " msg))))
```
Without a handler, `throw` crashes the fiber. Note: `string->number` (defined in prelude) wraps the `__try-string->number` builtin and throws on parse failure.

## Known Issues

1. **Module globals workaround:** Qualified imports (`import "mod" as m`) install both prefixed (`m/foo`) and unqualified (`foo`) names into parent globals. First-imported module wins for name collisions. Long-term fix: module-scoped globals.

2. **Recursive `define` + TCP effects hangs:** Using `(define f (lambda () ... (f)))` inside `fiber-spawn` where the body does TCP effects causes hangs. Workaround: unroll recursion or avoid combining recursive `define` with scheduler-suspending effects.

3. **`match` branches require single expressions:** Multi-expression match bodies need explicit `(begin ...)` wrapping.

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
