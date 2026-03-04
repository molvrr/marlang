# Language Reference

## Data Types

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

**Characters** - Unicode codepoints, created via `char-from-code`:

```lisp
(char-from-code 65)      ; => 'A'
(char-code (char-from-code 65))  ; => 65
(string->chars "hi")     ; => (Cons 'h' (Cons 'i' Nil))
(char->string (char-from-code 65))  ; => "A"
```

Characters are NaN-boxed inline (no heap allocation). Type: `Char`.

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
(import "std:io")

;; Pure function (no effects)
(define (f : (-> String {} String)) (lambda (s) s))

;; Function with IO effect
(define (g : (-> String {IO} Unit)) (lambda (s) (println s)))

;; Open effect row (row-polymorphic): IO + any other effects
(define (h : (-> String {IO | e} Unit)) (lambda (s) (println s)))
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
| `rem` `abs` `negate` `sign` | `rem.` `abs.` `negate.` `sign.` | Other numeric |

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
(match (Some 1)
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

; Mutual recursion also optimized (wrap in a function body for letrec semantics)
(define check-parity (lambda (n)
  (define is-even (lambda (n)
    (if (= n 0) #t (is-odd (- n 1)))))
  (define is-odd (lambda (n)
    (if (= n 0) #f (is-even (- n 1)))))
  (is-even n)))

(check-parity 50000)  ; => #t (no stack overflow)
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

The `Exception` effect is declared in the prelude and available everywhere:

```lisp
(import "std:io")

; Catch exceptions
(handle (perform throw "something went wrong")
  ((throw msg k) -> (println (string-append "caught: " msg))))
```

## Concurrency

Marlang provides green threads (fibers), CSP-style channels, and an Erlang-inspired actor model:

```lisp
(import "std:concurrent" as conc)
(import "std:actor" as act)
(import "std:io" as io)

; Spawn fibers
(run-fibers (lambda ()
  (conc/spawn (lambda ()
    (io/println "child")))
  (io/println "parent")))

; Channels
(run-fibers (lambda ()
  (let ((ch (conc/chan-new 0)))
    (conc/spawn (lambda ()
      (conc/chan-send! ch 42)))
    (io/println (show (conc/chan-recv ch))))))

; Actors with mailboxes
(run-fibers (lambda ()
  (let ((me (conc/self)))
    (conc/spawn (lambda ()
      (act/send me "hello")))
    (io/println (act/receive)))))
```

## FFI (Foreign Function Interface)

Marlang can call functions from native C libraries using libffi.

### Functions

- **`foreign-lib`** - declare a foreign library with typed function signatures
- **`ffi-open`** - load a native library (low-level, used by `foreign-lib` internally)
- **`ffi-call`** - call a function from a library opened with `ffi-open`
- **`ffi-close`** - close a library handle

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
; Declare a foreign library with typed function signatures
(foreign-lib math "m"
  (sin (-> Float Float))
  (cos (-> Float Float))
  (pow (-> Float Float Float)))

; Call math functions (qualified with library name)
(math/sin 0.0)                ; => 0.0
(math/sin 1.5707963267948966) ; => 1.0 (pi/2)
(math/cos 0.0)                ; => 1.0
(math/pow 2.0 10.0)           ; => 1024.0
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
(import "std:io")

; Option utilities
(opt/map (lambda (x) (+ x 1)) (Some 5))  ; => (Some 6)
(opt/unwrap-or 0 None)                    ; => 0
(opt/filter (lambda (x) (> x 10)) (Some 5)) ; => None

; Result type for error handling (Ok and Err constructors come from the prelude)
(define safe-div
  (lambda (a b)
    (if (= b 0)
        (Err "division by zero")
        (Ok (/ a b)))))

(safe-div 10 2)   ; => (Ok 5)
(safe-div 10 0)   ; => (Err "division by zero")

; IO operations
(println "Hello, world!")
```

### Prelude (Auto-Loaded)

Every Marlang program automatically has access to the prelude (`stdlib/prelude.mar`), which defines:
- **Data types**: `Option` (None, Some), `List` (Nil, Cons), `Pair` (MkPair), `Result` (Ok, Err)
- **Effects**: `Exception` (throw)
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
| `std:string` | String utilities: basic ops (`concat`, `length`, `is-empty`, `reverse`, `repeat`), case (`to-upper`, `to-lower`), trim (`trim`, `trim-start`, `trim-end`), search (`starts-with`, `ends-with`, `contains`, `index-of`), substrings (`substring`, `char-at`, `take`, `drop`), split/join (`split`, `join`, `lines`, `unlines`, `words`, `unwords`), replace (`replace`, `replace-all`), padding (`pad-start`, `pad-end`, `center`), comparison (`eq`, `lt`, `gt`, `le`, `ge`), conversion (`from-number`), char ops (`to-char-code`, `from-char-code`, `to-chars`, `from-chars`, `char-to-string`) |
| `std:math` | Math utilities: constants (`pi`, `e`, `tau`), basic ops (`min-of`/`min-of.`, `max-of`/`max-of.`, `clamp`/`clamp.`), powers (`square`, `cube`), int division (`div`, `div.`, `even`, `odd`), logarithms (`log-base`), angles (`degrees`, `radians`), number theory (`gcd`, `lcm`, `factorial`). Trig/exp/log builtins (`sqrt`, `pow`, `sin`, `cos`, etc.) are available globally without import. |
| `std:concurrent` | Concurrency wrappers: `spawn`, `yield`, `self`, `chan-new`, `chan-send`, `chan-recv`, `chan-close`, `run` |
| `std:actor` | Actor model: `send`, `receive`, `receive-match`, `monitor`, `link`, `unlink`, `trap-exit` |
| `std:net` | TCP networking: `listen`, `accept`, `connect`, `send`, `recv`, `close`, `close-listener` |
