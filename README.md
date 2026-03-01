# Marlang

A Lisp-like language written in Rust. Features include a bytecode VM with NaN-boxed values, a Haskell-style static type system, algebraic data types, pattern matching, algebraic effects with delimited continuations, closures, tail call optimization, green threads with channels, an actor model with supervision, and a Foreign Function Interface (FFI) via libffi.

## Features

- **Bytecode VM**: Stack-based bytecode compiler and virtual machine with NaN-boxed values
- **Type System**: Static type checking with local inference, parametric polymorphism, row-polymorphic effects, and OCaml-style operator split (Int/Float)
- **Algebraic Data Types**: Define sum types with constructors
- **Pattern Matching**: Destructure values with `match` expressions, exhaustiveness checking
- **Algebraic Effects**: Delimited continuations with `effect`, `perform`, and `handle`
- **Concurrency**: Green threads (fibers), CSP-style channels, actor model with mailboxes and supervision
- **Module System**: Import code with `std:` prefix for stdlib or relative paths
- **Standard Library**: Auto-loaded prelude with Option, List, Pair, Result types; stdlib utilities out of the box
- **Tail Call Optimization**: Efficient recursion without stack overflow
- **Macros**: Hygienic macro system with `defmacro`, `gensym`, and quasiquoting
- First-class functions with lexical closures
- FFI for calling native libraries (via libffi)
- FFI struct support for C struct interop
- Async IO (TCP networking)
- Interactive REPL

## Building

```bash
# Build the CLI (creates 'mar' binary)
cargo build --release -p marlang-cli

# Or run directly via cargo
cargo run -p marlang-cli -- --help
```

## Running

Run a Marlang file:

```bash
cargo run -p marlang-cli -- run <filename.mar>
```

Disassemble a file to view bytecode:

```bash
cargo run -p marlang-cli -- disasm <filename.mar>
```

Type check without running:

```bash
cargo run -p marlang-cli -- typecheck <filename.mar>
cargo run -p marlang-cli -- typecheck <filename.mar> --verbose  # Show inferred types
```

Start the interactive REPL:

```bash
cargo run -p marlang-cli -- repl
```

Run the test suite:

```bash
cargo test --workspace
```

## Language Reference

### Data Types

**Comments** - semicolon to end of line:

```lisp
; This is a comment
(+ 1 2) ; inline comment
```

**Numbers** - integers and floating-point:

```lisp
42        ; Int
3.14      ; Float
-17       ; Int
```

**Strings** - double-quoted with escape sequences:

```lisp
"hello world"
"line1\nline2"
"tab\there"
```

**Booleans** - `#t` for true, `#f` for false (returned by comparison operators)

**Lists** - created with the `list` function:

```lisp
(list 1 2 3)
(list "a" "b" "c")
```

**Functions** - created with `lambda`:

```lisp
(lambda (x) (* x x))
```

## Type System

Marlang features a Haskell-style static type system with local type inference.

### Type Annotations

Use `:` to annotate types:

```lisp
; Annotated variable
(define (x : Int) 42)
(define (y : Float) 3.14)
(define (name : String) "marlang")

; Annotated function
(define (add : (-> Int Int Int))
  (lambda (a b) (+ a b)))

; Annotated lambda parameters
(lambda ((x : Int) (y : Int)) (+ x y))

; Annotated let bindings
(let (((x : Int) 42)) x)
```

### Function Types

Function types use the `->` syntax:

```lisp
(-> Int Int)           ; Int -> Int
(-> Int Int Int)       ; Int -> Int -> Int
(-> a a)               ; Polymorphic: a -> a
```

### Effect Row Annotations

Arrow types support row-polymorphic effect annotations using `{...}` syntax:

```lisp
;; Pure function (no effects)
(define (f : (-> String {} String)) (lambda (s) s))

;; Function with IO effect
(define (g : (-> String {IO} Unit)) (lambda (s) (perform println s)))

;; Open effect row (row-polymorphic): IO + any other effects
(define (h : (-> String {IO | e} Unit)) (lambda (s) (perform println s)))
```

### Type Inference

Types are inferred when annotations are omitted:

```lisp
(define id (lambda (x) x))  ; Inferred: forall a. a -> a
(id 42)                      ; OK: Int
(id "hello")                 ; OK: String
```

### OCaml-Style Operator Split

Arithmetic and comparison operators are split by type — Int operators are plain, Float operators use a dot suffix:

| Int | Float | Description |
|-----|-------|-------------|
| `+` `-` `*` `/` | `+.` `-.` `*.` `/.` | Arithmetic |
| `=` `<` `>` `<=` `>=` | `=.` `<.` `>.` `<=.` `>=.` | Comparison |
| `rem` `abs` `negate` | `rem.` `abs.` `negate.` | Other numeric |

String comparison uses named functions: `string=`, `string<`, `string>`, `string<=`, `string>=`.

Polymorphic operations (no split needed):
- `equal?` — structural equality on any type
- `show` — convert any value to String
- `print` / `println` — output any value

```lisp
; Int arithmetic
(+ 1 2)              ; => 3
(< 1 2)              ; => #t

; Float arithmetic (dot suffix)
(+. 1.5 2.5)         ; => 4.0
(<. 1.0 2.0)         ; => #t

; Conversion
(+. (int->float 42) 0.5)  ; => 42.5

; Polymorphic
(equal? (Some 1) (Some 1))  ; => #t
(show 42)                    ; => "42"
```

## Algebraic Data Types

Define sum types with the `data` form:

```lisp
; Option type
(data (Option a)
  None
  (Some a))

; List type
(data (List a)
  Nil
  (Cons a (List a)))

; Pair type
(data (Pair a b)
  (MkPair a b))
```

Constructors become values or functions:

```lisp
None                    ; => None
(Some 42)               ; => (Some 42)
(Cons 1 (Cons 2 Nil))   ; => (Cons 1 (Cons 2 Nil))
```

## Pattern Matching

Use `match` to destructure values:

```lisp
; Match on Option
(match (Some 42)
  (None 0)
  ((Some v) v))          ; => 42

; Match on List
(define length
  (lambda (lst)
    (match lst
      (Nil 0)
      ((Cons h t) (+ 1 (length t))))))

(length (Cons 1 (Cons 2 (Cons 3 Nil))))  ; => 3

; Wildcard pattern
(match x
  (_ "default"))

; Nested patterns
(match (MkPair (Some 1) (Some 2))
  ((MkPair (Some x) (Some y)) (+ x y))
  (_ 0))                 ; => 3
```

### Exhaustiveness Checking

The type checker verifies that `match` expressions cover all possible constructors:

```lisp
; ERROR: Non-exhaustive patterns. Missing: None
(match (Some 42)
  ((Some v) v))

; OK: All constructors covered
(match (Some 42)
  (None 0)
  ((Some v) v))

; OK: Wildcard covers all cases
(match (Some 42)
  (_ 0))
```

### Arithmetic Operations

```lisp
(+ 1 2)         ; => 3
(+ 1 2 3 4 5)   ; => 15 (variadic)
(- 10 3)        ; => 7
(- 5)           ; => -5 (negation)
(* 4 5)         ; => 20
(/ 20 4)        ; => 5
```

### Comparison Operations

All comparisons return `#t` or `#f`:

```lisp
(< 1 2)         ; => #t
(> 1 2)         ; => #f
(<= 3 3)        ; => #t
(>= 5 3)        ; => #t
(= 4 4)         ; => #t
```

### Variable Bindings

**`define`** - create global bindings:

```lisp
(define x 10)
(define pi 3.14159)
(+ x 5)         ; => 15
```

**`let`** - create local bindings:

```lisp
(let ((a 1) (b 2))
  (+ a b))      ; => 3
```

### Functions

**`lambda`** - create anonymous functions:

```lisp
(define square (lambda (n) (* n n)))
(square 4)      ; => 16
```

**Closures** - functions capture their lexical environment:

```lisp
(define make-adder (lambda (n) (lambda (x) (+ n x))))
(define add5 (make-adder 5))
(add5 10)       ; => 15
```

**Recursion** - functions can call themselves:

```lisp
(define fact (lambda (n)
  (if (= n 0)
    1
    (* n (fact (- n 1))))))
(fact 5)        ; => 120
```

### Control Flow

**`if`** - conditional expression:

```lisp
(if (< 1 2) "yes" "no")   ; => "yes"
(if (> 1 2) "yes" "no")   ; => "no"
```

**`cond`** - multi-way conditional:

```lisp
(define grade (lambda (score)
  (cond
    ((>= score 90) "A")
    ((>= score 80) "B")
    ((>= score 70) "C")
    (else "F"))))

(grade 95)      ; => "A"
(grade 85)      ; => "B"
(grade 65)      ; => "F"
```

**`begin`** - evaluate a sequence of expressions, returning the last value:

```lisp
(begin
  (define x 10)
  (define y 20)
  (+ x y))          ; => 30
```

## Tail Call Optimization

Marlang implements proper tail call optimization, allowing efficient recursive algorithms:

```lisp
; Tail-recursive factorial - won't overflow
(define fact-tail (lambda (n acc)
  (if (= n 0)
    acc
    (fact-tail (- n 1) (* n acc)))))

(fact-tail 50000 1)  ; Works!

; Mutual recursion also optimized
(define is-even (lambda (n)
  (if (= n 0) #t (is-odd (- n 1)))))
(define is-odd (lambda (n)
  (if (= n 0) #f (is-even (- n 1)))))

(is-even 50000)  ; => #t (no stack overflow)
```

## Algebraic Effects

Marlang supports algebraic effects with delimited continuations:

```lisp
; Declare an effect
(effect State
  (get (-> Unit Int))
  (put (-> Int Unit)))

; Use effects
(define counter (lambda ()
  (let ((n (perform get)))
    (perform put (+ n 1))
    (perform get))))

; Handle effects
(handle (counter)
  ((get _ k) -> (resume k 0))
  ((put n k) -> (resume k nil)))
```

### Exception Effect

```lisp
(effect Exception
  (throw (-> String a)))

; Catch exceptions
(handle (perform throw "something went wrong")
  ((throw msg k) -> (perform println (string-append "caught: " msg))))
```

## Concurrency

Marlang provides green threads (fibers), CSP-style channels, and an Erlang-inspired actor model:

```lisp
; Spawn fibers
(run-fibers (lambda ()
  (perform fiber-spawn (lambda ()
    (perform println "child")))
  (perform println "parent")))

; Channels
(run-fibers (lambda ()
  (let ((ch (perform chan-new 0)))
    (perform fiber-spawn (lambda ()
      (perform chan-send ch 42)))
    (perform println (show (perform chan-recv ch))))))

; Actors with mailboxes
(run-fibers (lambda ()
  (let ((me (perform fiber-self)))
    (perform fiber-spawn (lambda ()
      (perform actor-send me "hello")))
    (perform println (perform actor-receive)))))
```

## FFI (Foreign Function Interface)

Marlang can call functions from native C libraries using libffi.

### Functions

- **`ffi-open`** - load a native library with function signatures
- **`ffi-call`** - call a function from the library
- **`ffi-close`** - close the library handle

### Supported Types

| Type     | Description              |
|----------|--------------------------|
| `i8`     | 8-bit signed integer     |
| `i16`    | 16-bit signed integer    |
| `i32`    | 32-bit signed integer    |
| `i64`    | 64-bit signed integer    |
| `u8`     | 8-bit unsigned integer   |
| `u16`    | 16-bit unsigned integer  |
| `u32`    | 32-bit unsigned integer  |
| `u64`    | 64-bit unsigned integer  |
| `f32`    | 32-bit float             |
| `f64`    | 64-bit float (double)    |
| `ptr`    | Pointer                  |
| `cstring`| C string (null-terminated)|
| `bool`   | Boolean                  |
| `void`   | Void (for return types)  |

### Example: Using libm

```lisp
; Open libm with function signatures
(define math (ffi-open "m"
  (list
    (list "sin" (list "f64") "f64")
    (list "cos" (list "f64") "f64")
    (list "pow" (list "f64" "f64") "f64"))))

; Call math functions
(ffi-call math "sin" 0.0)              ; => 0
(ffi-call math "sin" 1.5707963267948966) ; => 1 (pi/2)
(ffi-call math "cos" 0.0)              ; => 1
(ffi-call math "pow" 2.0 10.0)         ; => 1024

; Close when done
(ffi-close math)
```

## FFI Structs

Marlang supports C struct definitions for passing structured data to native libraries.

### Struct Functions

- **`ffi-struct`** - define a struct layout with field types
- **`struct-new`** - create a struct instance with field values
- **`struct-get`** - read a field value from a struct
- **`struct-set`** - write a value to a struct field

### Defining Structs

```lisp
; Define a Color struct (4 bytes: r, g, b, a)
(define Color (ffi-struct "Color" (list
  (list "r" "u8")
  (list "g" "u8")
  (list "b" "u8")
  (list "a" "u8"))))

; Define a Vector2 struct (8 bytes: x, y)
(define Vector2 (ffi-struct "Vector2" (list
  (list "x" "f32")
  (list "y" "f32"))))
```

### Creating and Using Structs

```lisp
; Create a red color
(define red (struct-new Color 255 0 0 255))

; Read fields
(struct-get red "r")    ; => 255
(struct-get red "a")    ; => 255

; Modify fields
(struct-set red "g" 128)
(struct-get red "g")    ; => 128
```

## Module System

Marlang supports importing code from other files using the `import` form.

### Import Syntax

```lisp
; Import all bindings
(import "path/to/module.mar")

; Import with qualifier (recommended)
(import "path/to/module.mar" as m)
(m/function-name arg)  ; Use with prefix

; Import specific bindings
(import "path/to/module.mar" (foo bar baz))
```

### Standard Library

Use the `std:` prefix to import from the standard library:

```lisp
; Option utilities (map, flat-map, unwrap-or, filter, etc.)
(import "std:option" as opt)

; Result type and utilities (map, flat-map, unwrap-or, ok, err, etc.)
(import "std:result" as res)
```

### Example: Using stdlib

```lisp
(import "std:option" as opt)
(import "std:result" as res)

; Option utilities
(opt/map (lambda (x) (+ x 1)) (Some 5))  ; => (Some 6)
(opt/unwrap-or 0 None)                    ; => 0
(opt/filter (lambda (x) (> x 10)) (Some 5)) ; => None

; Result type for error handling
(define safe-div
  (lambda (a b)
    (if (= b 0)
        (res/Err "division by zero")
        (res/Ok (/ a b)))))

(safe-div 10 2)   ; => (Ok 5)
(safe-div 10 0)   ; => (Err "division by zero")

; IO operations (built-in handlers work automatically)
(perform println "Hello, world!")
```

### Prelude (Auto-Loaded)

Every Marlang program automatically has access to the prelude (`stdlib/prelude.mar`), which defines:
- **Data types**: `Option` (None, Some), `List` (Nil, Cons), `Pair` (MkPair), `Result` (Ok, Err)
- **Macros**: `when`
- **Functions**: `not`, `cons`, `car`, `cdr`, `append`, `string->number`

No import is needed for these -- they are available everywhere.

### Available stdlib modules

| Module | Description |
|--------|-------------|
| `std:option` | Utilities for the `Option` type: `map`, `flat-map`, `unwrap-or`, `unwrap-or-else`, `is-some`, `is-none`, `filter`, `and-then`, `or-else` |
| `std:result` | Utilities for the `Result` type: `map`, `map-err`, `flat-map`, `unwrap-or`, `unwrap-or-else`, `is-ok`, `is-err`, `ok`, `err`, `and-then`, `or-else` |
| `std:list` | List utilities: `foldl`, `foldr`, `length`, `reverse`, `concat`, `map`, `filter`, `any`, `all`, `find`, `take`, `drop`, `zip`, `zip-with` |
| `std:io` | IO effect with **built-in handlers**: `print`, `println`, `read-line`, `read-file`, `write-file`. Handlers work automatically without explicit `handle` blocks. |
| `std:string` | String utilities: `length`, `concat`, `reverse`, `to-upper`, `to-lower`, `trim`, `split`, `join`, `replace`, `substring`, `char-at` |
| `std:math` | Math utilities: `pi`, `e`, `min-of`/`min-of.`, `max-of`/`max-of.`, `clamp`/`clamp.`, `square`, `cube`, `div`/`div.`, `even`, `odd`, `gcd`, `lcm`, `factorial`, `log-base`, `degrees`, `radians` |
| `std:concurrent` | Concurrency wrappers: `spawn`, `yield`, `self`, `chan-new`, `chan-send`, `chan-recv`, `chan-close`, `run` |
| `std:actor` | Actor model: `send`, `receive`, `receive-match`, `monitor`, `link`, `unlink`, `trap-exit` |

## Architecture

```
marlang/
├── port/                 # Rust implementation (Cargo workspace)
│   └── crates/
│       ├── core/         # marlang-core: lexer, parser, string interning, spans
│       ├── types/        # marlang-types: HM type inference, exhaustiveness
│       ├── vm/           # marlang-vm: compiler, NaN-boxed VM, modules, FFI, schedulers, macros, TCP
│       └── cli/          # marlang-cli: clap-based CLI (run, repl, typecheck, disasm)
├── stdlib/               # Standard library (.mar files)
│   ├── prelude.mar       # Auto-loaded: data types, macros, core functions
│   ├── option.mar        # Option type utilities
│   ├── result.mar        # Result type and utilities
│   ├── list.mar          # List utilities
│   ├── io.mar            # IO effect and operations
│   ├── string.mar        # String utilities
│   ├── math.mar          # Math utilities
│   ├── concurrent.mar    # Concurrency wrappers
│   └── actor.mar         # Actor model
├── examples/             # Example programs
├── bench/                # Benchmark .mar programs
├── web/                  # Web playground (needs WASM port — see web/TODO.md)
├── ts-archive/           # Archived TypeScript implementation (historical reference)
│   ├── src/              # Original TS source (core, vm, types, modules)
│   ├── spec/             # Original TS test suite
│   ├── lsp/              # Original TS LSP server
│   └── ...
└── specs/                # Language specification notes
```

### Crate Structure

- **`marlang-core`** -- Lexer, parser, string interning (`intern.rs`), spans. Tokenizes source into tokens, parses into S-expression AST.
- **`marlang-types`** -- Hindley-Milner type inference, row-polymorphic effect types, exhaustiveness checking for `match`, FFI struct type validation.
- **`marlang-vm`** -- Bytecode compiler, NaN-boxed value representation, stack-based VM with algebraic effect system (handler frames, continuations), module loader, FFI via libffi, single-threaded and multi-threaded fiber schedulers, async IO, hygienic macro expansion, TCP networking.
- **`marlang-cli`** -- Clap-based CLI with `run`, `repl`, `typecheck`, `disasm` subcommands.

## Archived TypeScript Implementation

The original TypeScript implementation (which ran on Bun) is preserved in `ts-archive/` for historical reference. It includes the tree-walking interpreter, bytecode VM, type system, LSP server, and test suite. The Rust port is the active implementation.
