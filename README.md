# Marlang

A Lisp-like interpreter written in TypeScript, running on Bun. Features include a Haskell-style static type system, algebraic data types, pattern matching, closures, tail call optimization, and a Foreign Function Interface (FFI) for calling native C libraries.

## Features

- **Type System**: Static type checking with local inference, parametric polymorphism, and typeclasses
- **Algebraic Data Types**: Define sum types with constructors
- **Pattern Matching**: Destructure values with `match` expressions
- **Tail Call Optimization**: Efficient recursion without stack overflow
- **LSP Server**: Language Server Protocol support for IDE integration
- First-class functions with lexical closures
- First-class environments for introspection
- FFI for calling native libraries (via Bun's FFI)
- FFI struct support for C struct interop
- Interactive REPL

## Installation

```bash
bun install
```

## Running

Run a Marlang file:

```bash
bun index.ts run <filename.mar>
```

Run without type checking:

```bash
bun index.ts run <filename.mar> --no-typecheck
```

Type check without running:

```bash
bun index.ts typecheck <filename.mar>
bun index.ts typecheck <filename.mar> --verbose  # Show inferred types
```

Start the interactive REPL:

```bash
bun index.ts repl
```

Start the LSP server:

```bash
bun index.ts lsp
```

Run the test suite:

```bash
bun test
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

**Environments** - captured with `current-env`:

```lisp
(current-env)   ; => #<environment [+, -, *, /, ...] +parent>
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

### Type Inference

Types are inferred when annotations are omitted:

```lisp
(define id (lambda (x) x))  ; Inferred: forall a. a -> a
(id 42)                      ; OK: Int
(id "hello")                 ; OK: String
```

### Typeclasses

Built-in typeclasses provide polymorphic operations:

| Class | Methods | Instances |
|-------|---------|-----------|
| `Num` | `+`, `-`, `*`, `/` | Int, Float |
| `Eq` | `=` | Int, Float, Bool, String |
| `Ord` | `<`, `>`, `<=`, `>=` | Int, Float, String |
| `Show` | `show` | Int, Float, Bool, String |

```lisp
; Works with any Num type
(define double (lambda (x) (+ x x)))
(double 5)      ; => 10 : Int
(double 2.5)    ; => 5.0 : Float
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

(fact-tail 50000 1)  ; Works! (returns Infinity due to number overflow)

; Mutual recursion also optimized
(define is-even (lambda (n)
  (if (= n 0) #t (is-odd (- n 1)))))
(define is-odd (lambda (n)
  (if (= n 0) #f (is-even (- n 1)))))

(is-even 50000)  ; => #t (no stack overflow)
```

## FFI (Foreign Function Interface)

Marlang can call functions from native C libraries using Bun's FFI.

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

### Library Paths

`ffi-open` accepts library names in several formats:

- **Short name**: `"m"` → automatically becomes `libm.dylib` (macOS) or `libm.so` (Linux)
- **Absolute path**: `"/usr/local/lib/libcustom.dylib"` → used as-is
- **Relative path with extension**: `"./mylib.so"` → used as-is

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

## Environment Introspection

Marlang provides first-class access to lexical environments for metaprogramming and debugging.

### Functions

- **`current-env`** - returns the current lexical environment as a value
- **`env-bindings`** - returns a list of `(name value)` pairs for immediate scope
- **`env-get`** - looks up a binding by name string (returns `nil` if not found)
- **`env-parent`** - returns the parent environment or `nil`

### Examples

```lisp
; Capture current environment
(define x 10)
(define my-env (current-env))
(env-get my-env "x")           ; => 10

; Inspect bindings in a let scope
(let ((a 1) (b 2))
  (env-bindings (current-env))) ; => (("a" 1) ("b" 2))

; Navigate the scope chain
(let ((outer 100))
  (let ((inner 200))
    (env-get (env-parent (current-env)) "outer"))) ; => 100
```

## LSP Server

Marlang includes a Language Server Protocol (LSP) server for IDE integration.

### Features

- **Diagnostics**: Type errors are reported with source locations
- **Hover**: Shows type information for symbols on hover
- **Go to Definition**: Navigate to symbol definitions
- **Completion**: Suggests keywords, built-in functions, and user-defined symbols

### Usage

Start the LSP server:

```bash
bun index.ts lsp
```

The server communicates over stdin/stdout using the standard LSP JSON-RPC protocol.

### Editor Integration

To use the LSP with your editor, configure it to run `bun index.ts lsp` as the language server for `.mar` files. For example:

**VS Code**: Create an extension or use a generic LSP client extension, configuring it to run the Marlang LSP for `.mar` files.

**Neovim** (with nvim-lspconfig):

```lua
local lspconfig = require('lspconfig')
local configs = require('lspconfig.configs')

configs.marlang = {
  default_config = {
    cmd = { 'bun', 'index.ts', 'lsp' },
    filetypes = { 'marlang' },
    root_dir = lspconfig.util.find_git_ancestor,
  },
}

lspconfig.marlang.setup({})
```

## Architecture

The interpreter follows a classic pipeline:

1. **Lexer** (`lexer.ts`) - tokenizes source code into tokens
2. **Parser** (`parser.ts`) - builds an AST of S-expressions
3. **Type Checker** (`type-checker.ts`) - validates types statically
4. **Interpreter** (`interpreter.ts`) - evaluates the AST with tail call optimization

### Type System Architecture

- **`type-ast.ts`** - Type representation (Type, TypeScheme, Constraint, Pattern)
- **`type-env.ts`** - Type environment and substitution operations
- **`type-unify.ts`** - Unification algorithm
- **`type-infer.ts`** - Constraint generation and inference
- **`type-builtins.ts`** - Built-in types and typeclass definitions
- **`type-errors.ts`** - Error types and formatting

### Project Structure

```
marlang/
├── index.ts           # CLI entry point
├── lexer.ts           # Tokenizer with position tracking
├── parser.ts          # S-expression parser
├── types.ts           # AST and runtime value types
├── interpreter.ts     # Tree-walking interpreter with TCO
├── type-*.ts          # Type system modules
├── lsp/               # Language Server Protocol
│   ├── server.ts      # LSP server entry point
│   ├── protocol.ts    # JSON-RPC message handling
│   ├── document.ts    # Open document management
│   ├── analyzer.ts    # Analysis pipeline
│   └── handlers/      # LSP request handlers
│       ├── diagnostics.ts
│       ├── hover.ts
│       ├── definition.ts
│       └── completion.ts
├── spec/              # Test suite
│   ├── lexer.test.ts
│   ├── parser.test.ts
│   ├── interpreter.test.ts
│   ├── tco.test.ts
│   ├── type-system.test.ts
│   └── lsp.test.ts
└── examples/
    ├── types.mar      # Type system demo
    └── raylib.mar     # Raylib graphics bindings
```
