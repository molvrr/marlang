# Marlang Development Guide

## Running

```bash
# Build the CLI (creates 'mar' binary)
cargo build --release -p marlang-cli

# Or run directly via cargo
cargo run -p marlang-cli -- run file.mar           # Run a file
cargo run -p marlang-cli -- typecheck file.mar     # Type check (prints last expression's type)
cargo run -p marlang-cli -- disasm file.mar        # Disassemble bytecode
cargo run -p marlang-cli -- expand file.mar        # Show macro-expanded source
cargo run -p marlang-cli -- expand file.mar --clean # Expanded source without hygiene marks
cargo run -p marlang-cli -- repl                   # Interactive REPL
```

**Important:** Type checking is mandatory — there is no `--no-typecheck` flag. Programs must pass the type checker before they can run.

## Testing

```bash
cargo test --workspace                # Run all tests
cargo test -p marlang-core            # Core: lexer + parser
cargo test -p marlang-types           # Type system
cargo test -p marlang-vm              # VM: compiler + execution + modules + FFI + scheduler + macros + TCP + Exception + Variadic + CPS handler dispatch + Bytes + HTTP
```

## Project Structure

```
port/crates/
  core/       - marlang-core: lexer, parser, string interning, spans
  types/      - marlang-types: HM type inference, exhaustiveness
  vm/         - marlang-vm: compiler, VM, modules, FFI, schedulers, macros, TCP
  cli/        - marlang-cli: clap-based CLI (run, repl, typecheck, disasm, expand)
stdlib/       - Standard library (.mar files, shared)
examples/     - Example programs
bench/        - Benchmark .mar programs
web/          - Web playground (currently broken — needs WASM port, see web/TODO.md)
ts-archive/   - Archived TypeScript implementation (historical reference)
```

### Standard Library
Located in `stdlib/`:
- `prelude.mar` - Auto-loaded prelude: data types (Option, List, Pair, Result), Exception effect, `when` macro, utility functions (not, cons, car, cdr, append, string->int, string->float). `(declare ...)` forms for all builtins live here.
- `option.mar` - Option type utilities (map, flat-map, unwrap-or, etc.)
- `result.mar` - Result type and utilities (Ok, Err, map, flat-map, etc.)
- `io.mar` - IO effect and operations (print, println, read-line, read-file, write-file)
- `list.mar` - List utilities (foldl, foldr, map, filter, zip, etc.)
- `string.mar` - String utilities: basic ops (concat, length, is-empty, reverse, repeat), case (to-upper, to-lower), trim, search (starts-with, ends-with, contains, index-of), substrings (substring, char-at, take, drop), split/join (split, join, lines, unlines, words, unwords), replace, padding (pad-start, pad-end, center), comparison (eq, lt, gt, le, ge), conversion (from-number), char ops (to-char-code, from-char-code, to-chars, from-chars, char-to-string)
- `math.mar` - Math utilities: constants (pi, e, tau), basic ops (min-of, max-of, clamp — Int and Float versions), powers (square, cube), int division (div, div., even, odd), logarithms (log-base), angles (degrees, radians), number theory (gcd, lcm, factorial). Trig/exp/log builtins (sqrt, pow, sin, cos, etc.) are available globally without import.
- `concurrent.mar` - Concurrency wrappers (fibers, channels)
- `actor.mar` - Actor model (mailboxes, supervision)
- `codec.mar` - Bidirectional parser/printer: Iso type, Codec (parse/print/round-trip), combinators (seq, alt, many, between, sep-by, choice), character codecs (satisfy, digit, letter, space), numeric codecs (nat, integer), isomorphisms (iso-chars, iso-int, iso-float, iso-cons, iso-some), convenience (token, symbol, parens, braces, brackets), parse-only compat (chainl1, chainr1, or-else)
- `net.mar` - TCP networking wrappers: safe (connect, listen, send return Result) and throwing (connect!, listen!, send! raise on error), plus accept, recv, close, close-listener
- `http.mar` - HTTP client and server: HttpMethod ADT (GET, POST, PUT, DELETE, PATCH, HEAD, OPTIONS), HttpResponse ADT (status, headers, body as Bytes), HttpRequest ADT (method, path, headers, body, connection handle). Client: safe (get, post, request return Result) and throwing (get!, post!, request! raise on error) variants. Server: serve/serve! (bind port), accept (returns Option HttpRequest), respond/respond! (send response via connection handle), close-server (shutdown). Backed by hyper in the multi-threaded scheduler. Both HTTP and HTTPS supported for client (TLS via rustls with native OS certificates).
- `ws.mar` - WebSocket module: WsMessage ADT (WsText, WsBinary), WebSocket effect (ws-upgrade, ws-send, ws-send-bin, ws-recv, ws-close), upgrade? predicate, safe and throwing wrappers.
- `web.mar` - Web framework: Response ADT (HtmlResponse, RedirectResponse, TextResponse), HTML DSL (el, void-el, named elements, text escaping), routing (GET/POST/PUT/DELETE/PATCH with :param extraction), Ctx effect (param, query, form-val, header, method, path), state cells (cell-new, cell-get, cell-set, cell-update), error pages (500, 404), static file serving (serve-file, mime-type), server runner (run!, dispatch, accept-loop).
- `json.mar` - RFC 8259 compliant JSON codec: Json ADT (JsonNull, JsonBool, JsonInt, JsonFloat, JsonString, JsonArray, JsonObject), encode/decode, accessors (json-get, json-nth, json-string, json-int, etc.).
- `crypto.mar` - Hex encoding utility. MD5, SHA-256, and HMAC-SHA256 are builtins (ring + md-5 crates): `(md5 bytes)`, `(sha256 bytes)`, `(hmac-sha256 key data)`. Also: `(pbkdf2-sha256 password salt iterations)`, `(bytes-xor a b)`, `(base64-encode bytes)`, `(base64-decode string)`, `(random-bytes n)`.
- `pg.mar` - PostgreSQL client via wire protocol over TCP. Pure Marlang, no Rust dependencies. Functions: connect/connect!, query/query!, execute/execute!, close, col/col!/col-int/col-float/col-bool, each, map-rows, scalar/scalar!. Supports trust, cleartext, MD5, and SCRAM-SHA-256 authentication. Parameterized queries with $1, $2, etc. Connection pooling: pool-new, pool-query, pool-execute, pool-checkout, pool-checkin, pool-close. LISTEN/NOTIFY: listener-new, listener-subscribe, listener-unsubscribe, listener-stop, listen, unlisten, notify, wait-notification.

Note: ADT constructors (None, Some, Nil, Cons, MkPair, Ok, Err) and utility functions (not, cons, car, cdr, append, string->int, string->float) are defined in `stdlib/prelude.mar`, not as Rust builtins. The prelude is auto-loaded before every program.

## Language Conventions

### Primitive Types
The language has these primitive types: `Int`, `Float`, `Bool`, `String`, `Char`, `Unit`, `Bytes`. `Char` is a Unicode codepoint stored inline (no heap allocation). Character literals are displayed as `'A'` by `show`. Use `char-from-code` to create chars and `char-code` to extract the codepoint. `Bytes` is an immutable byte array for raw binary data (used by HTTP, file I/O, etc.).

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
Defined in `vm/builtins.rs`. Categories: Int arithmetic (`+`, `-`, `*`, `/`, `rem`, `abs`, `negate`, `sign`), Float arithmetic (`+.`, `-.`, `*.`, `/.`, `rem.`, `abs.`, `negate.`, `sign.`), Int comparison (`<`, `>`, `=`, `<=`, `>=`), Float comparison (`<.`, `>.`, `=.`, `<=.`, `>=.`), string comparison (`string=`, `string<`, `string>`, `string<=`, `string>=`), polymorphic (`equal?`, `show`, `int->float`), math (`sqrt`, `pow`, `exp`, `log`, `log10`, `log2`, `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan2`, `sinh`, `cosh`, `tanh`), conversions (`floor`, `ceil`, `round`, `truncate`), float checks (`is-nan`, `is-infinite`, `is-finite`), lists (`list`), strings (`string-length`, `string-append`, `string-is-empty`, `string-reverse`, `string-repeat`, `string-to-upper`, `string-to-lower`, `string-trim`, `string-trim-start`, `string-trim-end`, `string-starts-with`, `string-ends-with`, `string-contains`, `string-index-of`, `string-substring`, `string-char-at`, `string-split`, `string-join`, `string-replace`, `string-replace-all`, `string-pad-start`, `string-pad-end`, `number->string`), chars (`char-code`, `char-from-code`, `char->string`, `chars->string`, `string->chars`), bytes (`bytes-new`, `bytes-length`, `bytes-get`, `bytes-set`, `bytes-slice`, `bytes-concat`, `bytes-from-string`, `bytes-to-string`, `bytes-from-list`, `bytes-to-list`, `bytes-xor`), crypto (`md5`, `sha256`, `hmac-sha256`, `pbkdf2-sha256`, `base64-encode`, `base64-decode`, `random-bytes`), FFI (`ffi-open`, `ffi-call`, `ffi-close`), structs (`struct-new`, `struct-get`, `struct-set`), macros (`gensym`, `symbol->string`, `string->symbol`), type predicates (`symbol?`, `pair?`, `nil?`, `number?`, `string?`, `list?`). See `vm/builtins.rs` for the full list.

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

(handle (string->int "abc")
  ((throw msg k) -> (perform println (string-append "caught: " msg))))
```
Without a handler, `throw` crashes the fiber. Note: `string->int` and `string->float` (defined in prelude) wrap the `__try-string->int` and `__try-string->float` builtins and throw on parse failure.

## Rust Style

- Prefer `crate::` over `super::` for imports. Always use absolute crate paths (`crate::module::Item`) instead of relative `super::` paths.
- Keep all `use` statements in a single group with no blank lines between them. Do not separate std, external, and crate imports into separate groups.
- Declare `mod` items only in `lib.rs` (or `mod.rs` for submodules), never in `main.rs`. Binary crates should have a `lib.rs` that owns the module tree; `main.rs` imports from the lib crate.

## General Principles

- Never take hacky approaches. Always implement the proper, clean solution, even if that means things are momentarily broken.
- Never hardcode system-specific paths (e.g. `/usr/lib/system/libcommonCrypto.dylib`). Use Rust dependencies instead of FFI for core functionality that must be portable.
- Never ignore pre-existing bugs. If you discover a bug while working on something else, fix it before moving on.

## Workflow

### After writing Rust code
Always run clippy and fmt after writing or modifying Rust code:
```bash
cargo fmt --all --manifest-path port/Cargo.toml
cargo clippy --workspace --manifest-path port/Cargo.toml
```
Fix all warnings before considering the task done. The codebase must have zero clippy warnings.

### Verifying macro-using code
When writing or modifying Marlang code that uses macros (e.g. `when`, `unless`, or any `defmacro`), always run `expand` to inspect the expanded output:
```bash
cargo run -p marlang-cli -- expand file.mar --clean
```
Check that the expansion looks correct before running or type-checking. This catches macro bugs early — the expanded form is exactly what the compiler and type checker will see.

### After changes to the type checker or parser
Run the fuzz targets briefly to check for crashes:
```bash
cd port/fuzz
cargo fuzz run fuzz_source corpus/source/ -- -max_total_time=30 -dict=marlang.dict
cargo fuzz run fuzz_ast -- -max_total_time=30
cargo fuzz run fuzz_unify -- -max_total_time=30
```
Any crash is a bug — the type checker and unifier should never panic, only return `Ok` or `Err`. Investigate and fix crashes before considering the task done.

### After any changes
Run the benchmarks to make sure there is no performance regression:
```bash
./bench/run.sh
```
Compare the results against the baseline in `bench/README.md`. Investigate any significant regressions before considering the task done.

## Adding Features

### New Special Form
1. Add compilation in `compiler.rs` (`compile_expr` match arm)
2. Add VM execution in `vm.rs` if needed (new opcode)
3. Add type inference in `types/infer.rs` (`infer_expr`)
4. Add tests via `cargo test`

### New Built-in Function
1. Add to `vm/builtins.rs` (`register_builtins`)
2. Add `(declare ...)` form in `stdlib/prelude.mar`

### New Stdlib Module
1. Create `stdlib/module-name.mar`
2. Add tests in `port/tests/` or as Rust unit tests
3. Users import via `(import "std:module-name" as alias)`
