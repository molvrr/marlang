# Marlang

A Lisp-like interpreter written in TypeScript, running on Bun. Features include closures, recursion, and a Foreign Function Interface (FFI) for calling native C libraries.

## Features

- Lexer, parser, and tree-walking interpreter
- First-class functions with lexical closures
- Recursive function support
- FFI for calling native libraries (via Bun's FFI)
- Interactive REPL

## Installation

```bash
bun install
```

## Running

Run the test suite:

```bash
bun run index.ts
```

Start the interactive REPL:

```bash
bun run repl.ts
```

## Language Reference

### Data Types

**Numbers** - integers and floating-point:

```lisp
42
3.14
-17
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

## Architecture

The interpreter follows a classic pipeline:

1. **Lexer** (`lexer.ts`) - tokenizes source code into tokens (parens, numbers, strings, identifiers)
2. **Parser** (`parser.ts`) - builds an AST of S-expressions from tokens
3. **Interpreter** (`interpreter.ts`) - evaluates the AST using a tree-walking approach

### Environment Model

Variables are stored in environments with lexical scoping:

- Each environment has a map of bindings and an optional parent
- Variable lookup walks up the parent chain
- Closures capture their defining environment
- `define` mutates the current environment (enabling recursion)
- `let` creates a child environment for local bindings
