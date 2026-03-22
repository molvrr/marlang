# Macro System Vision — Parser & Type Checker Access

## Current State

Macros operate on raw S-expressions (lists, symbols, strings). They can:
- Walk the expression tree
- Construct new expressions
- Call string functions, arithmetic, etc.
- Call `emit-js` to compile to JavaScript

They CANNOT:
- Parse Marlang source from strings
- Access type information
- Know which symbols are ADT constructors vs functions
- Introspect effect declarations

## What Exposing Parser & Type Checker Enables

### 1. `define-client` auto-generates server dispatch

Currently the user manually writes a `handle-effects` function that matches
on effect name strings. With parser access, `define-client` could:

```scheme
(js/define-client app (x)
  (let ((count (perform load-count))
        (ref (perform atom-new (parse-int count))))
    ...))
```

The macro walks the body, finds `perform` calls, identifies which effects
are Atom (local) vs Server (proxied), and auto-generates:

```scheme
; Auto-generated server dispatch
(define __dispatch_app (lambda (name args)
  (match name
    ("load_count" (perform load-count))  ; ← re-performs on server in a handle block
    (_ (perform throw ...)))))
```

### 2. Smarter JS compilation

With type info, `emit-js` could:
- Know whether a name is an ADT constructor (no heuristic needed)
- Emit proper types for TypeScript declarations
- Optimize based on known types (int arithmetic vs float)

### 3. Code generation from types

```scheme
; Auto-derive JSON serialization from ADT definition
(derive-json User)  ; generates encode/decode from the User data type
```

### 4. Compile-time validation

Macros could typecheck generated code before emitting it,
catching errors in macro output rather than confusing the user.

## Implementation Plan

### New builtins (in vm/builtins.rs)

```
(mar-parse source-string)
  → Returns AST as Marlang data (nested lists/symbols)
  → Example: (mar-parse "(+ 1 2)") → (list '+ 1 2)
  → Already essentially what macros receive, but from strings

(mar-typecheck expr)
  → Type-checks an expression in the current environment
  → Returns type as a string: "(-> Int {Atom Server} String)"
  → Or error: (Err "Type mismatch: ...")

(mar-effects expr)
  → Returns the effect row of an expression
  → Example: (mar-effects '(perform load-count)) → (list "Server")

(mar-constructors type-name)
  → Returns ADT constructor info
  → Example: (mar-constructors "Option") → ((Some 1) (None 0))

(mar-effect-ops effect-name)
  → Returns effect operation signatures
  → Example: (mar-effect-ops "Server")
    → ((load-count (-> Unit String)) (save-count (-> String String)))
```

### Rust implementation estimate

1. **mar-parse** (~50 lines): Call existing parser, convert AST to Value
2. **mar-effect-ops** (~80 lines): Look up effect in type environment,
   return operations as list of (name, type-string) pairs
3. **mar-constructors** (~60 lines): Look up ADT, return constructor info
4. **mar-typecheck** (~150 lines): Run type inference on expression,
   return type string or error
5. **mar-effects** (~100 lines): Extract effect row from inferred type

Total: ~440 lines of Rust, mostly conversion between Rust types and Marlang Values.

### What changes in the macro system

Nothing — macros already run arbitrary Marlang code. These are just new
builtin functions available during expansion. The key insight: macro bodies
already execute at expansion time, so calling `(mar-effect-ops "Server")`
inside a `defmacro` body just works.

### Example: auto-dispatch

```scheme
(defmacro define-client-with-dispatch (name params body)
  ; Walk body to find all perform calls
  (let ((effects (find-performs body))
        ; Filter out Atom effects (handled locally)
        (server-effects (filter-non-atom effects)))
    ; Generate the dispatch function
    (let ((dispatch (generate-dispatch name server-effects)))
      (list 'begin
        ; Type-check body
        (list 'let (list (list '__tc (list 'lambda params body))) 'nil)
        ; Mount function
        (list 'define name ...)
        ; JS registration
        (list 'js/register-client-js ...)
        ; Auto-generated dispatch
        dispatch))))
```
