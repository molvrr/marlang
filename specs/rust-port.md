# Porting Marlang to Rust

**Type:** Architecture Specification
**Scope:** Full port — lexer, parser, type system, bytecode VM, scheduler, modules, LSP, CLI
**Goals:** Performance (5× fib40, <10 ms startup, 3× less memory) + standalone binary

---

## Table of Contents

1. [Overview & Goals](#1-overview--goals)
2. [Crate Structure](#2-crate-structure)
3. [Core Data Structures](#3-core-data-structures)
4. [Lexer & Parser](#4-lexer--parser-) ✅
5. [Type System](#5-type-system-) ✅
6. [Bytecode Compiler](#6-bytecode-compiler-) ✅
7. [VM Execution Engine](#7-vm-execution-engine-) ✅
8. [Concurrency](#8-concurrency-) ✅ (single-threaded + multi-threaded + async IO)
9. [FFI](#9-ffi-) ✅
10. [Module System](#10-module-system-) ✅
11. [LSP](#11-lsp)
12. [CLI](#12-cli-) ✅
13. [Testing Strategy](#13-testing-strategy)
14. [Migration Milestones](#14-migration-milestones)
15. [WASM & Web Editor](#15-wasm--web-editor)

---

## 1. Overview & Goals

### Why Rust

The TypeScript implementation (~14k LOC) runs on Bun and delivers good developer velocity but hits fundamental limits:

- **Startup**: Bun's ~30 ms JIT warm-up before any Marlang code runs
- **Memory**: Every VMValue is a JS object (even with NaN-unboxing of numbers, tagged unions carry overhead)
- **Distribution**: Requires Bun runtime installed; no single-binary story
- **GC pauses**: Bun's GC is opaque; latency-sensitive scheduler work (fibers, channels) suffers

Rust eliminates all four. The language is a natural fit: Marlang's VM is already a stack machine with explicit frames, its type system is pure-functional (no mutation), and the scheduler is a cooperative trampoline — all patterns that map directly to Rust ownership.

### Performance Targets

| Metric | TypeScript (Bun) | Rust Target | Improvement |
|--------|------------------|-------------|-------------|
| `fib(40)` wall-clock | ~1.8 s | < 0.35 s | ≥ 5× |
| Startup (empty file) | ~35 ms | < 10 ms | ≥ 3× |
| Peak RSS (`fib(40)`) | ~45 MB | < 15 MB | ≥ 3× |
| Binary size | N/A (needs Bun) | < 10 MB | standalone |

### Concurrency Target

| Metric | TypeScript (Bun) | Rust Target | Improvement |
|--------|------------------|-------------|-------------|
| `fib(40)` parallel × N cores | N/A (single-threaded) | Near-linear scaling | multi-core |
| Fiber spawn overhead | ~2 μs | < 0.5 μs | ≥ 4× |
| Cross-thread message latency | N/A | < 1 μs | native threads |

### Non-Goals (for initial port)

- JIT compilation — the bytecode interpreter is sufficient

---

## 2. Crate Structure

```
marlang/
├── Cargo.toml              # Workspace root
├── crates/
│   ├── core/               # marlang-core
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── lexer.rs
│   │       ├── parser.rs
│   │       ├── ast.rs      # Expr, LispValue, SourceLocation
│   │       ├── span.rs     # Span, line map
│   │       └── intern.rs   # String interning
│   ├── types/              # marlang-types
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── ast.rs      # Type, TypeScheme, Constraint, Pattern
│   │       ├── env.rs      # TypeEnv, substitution
│   │       ├── unify.rs    # Robinson's unification
│   │       ├── infer.rs    # HM inference, constraint generation
│   │       ├── builtins.rs # Built-in types, typeclasses
│   │       ├── checker.rs  # Top-level entry point
│   │       └── errors.rs   # TypeError, formatting
│   ├── vm/                 # marlang-vm
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── value.rs    # NaN-boxed Value (u64)
│   │       ├── object.rs   # VMObject enum (heap-allocated)
│   │       ├── bytecode.rs # Op, Instruction, Chunk, FunctionPrototype
│   │       ├── compiler.rs # AST → bytecode
│   │       ├── vm.rs       # VM struct, run(), effect handling
│   │       ├── builtins.rs # Built-in functions
│   │       ├── modules.rs  # Module loader, run_program, import handling
│   │       ├── ffi.rs      # FFI types, dlopen via libloading (behind "ffi" feature)
│   │       ├── scheduler.rs # Single-threaded fiber scheduler, channels, actors
│   │       ├── runtime.rs  # Multi-threaded scheduler with tokio async IO
│   │       └── debug.rs    # Disassembler
│   │       # Module system and FFI are in the vm crate:
│   │       # ├── modules.rs  — Module loader, run_program, import handling
│   │       # └── ffi.rs      — FFI types, dlopen via libloading (feature-gated)
│   ├── lsp/                # marlang-lsp
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── server.rs   # tower-lsp backend
│   │       ├── analyzer.rs # Full pipeline: lex → parse → typecheck
│   │       └── handlers.rs # hover, definition, completion, diagnostics
│   ├── cli/                # marlang-cli (binary crate)
│   │   └── src/
│   │       └── main.rs     # clap-based CLI: run, repl, typecheck, disasm, lsp
│   └── wasm/               # marlang-wasm (cdylib crate)
│       └── src/
│           └── lib.rs      # wasm-bindgen API: parse, typecheck, run, formatValue
├── web/                    # SolidJS web editor (imports WASM module)
│   ├── src/
│   │   ├── App.tsx
│   │   ├── ASTRenderer.tsx
│   │   ├── editor-state.ts
│   │   ├── editor-ops.ts
│   │   └── ReferencePanel.tsx
│   └── vite.config.ts
└── stdlib/                 # .mar files bundled at compile time
    ├── option.mar
    ├── result.mar
    ├── io.mar
    ├── net.mar
    └── http.mar
```

### Dependency Graph

```
cli ──→ lsp ──→ types ──→ core
  │                        ↑
  └──→ vm ────→ types ────┘
        ↑
wasm ───┘──→ types ──→ core
```

Every crate depends on `core` (for AST and spans). The `vm` crate depends on `core` only (not `types` — type annotations are handled syntactically by the compiler). The module system and FFI live inside the `vm` crate rather than as separate crates, since they need tight integration with the VM for expression-at-a-time compilation/execution and dirty globals tracking. The `lsp` crate depends on `types`. The `cli` crate depends on everything. The `wasm` crate depends on `vm`, `types`, and `core` — but **not** `lsp` or `cli`.

---

## 3. Core Data Structures

### 3.1 Spans and Source Locations

The TypeScript codebase stores `SourceLocation` as `{start: {line, column, offset}, end: {line, column, offset}}` on every AST node. This is 6 numbers (48 bytes). In Rust, use a compact span that resolves lazily:

```rust
/// Compact source span — 8 bytes total.
/// Byte offsets into the source string; line/column computed on demand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub const DUMMY: Span = Span { start: 0, end: 0 };
}

/// Line map built once per source file.
/// Binary-searches newline offsets to resolve (line, column).
pub struct LineMap {
    newline_offsets: Vec<u32>,
}

impl LineMap {
    pub fn new(source: &str) -> Self { /* ... */ }

    pub fn resolve(&self, offset: u32) -> (u32, u32) {
        let line = self.newline_offsets.partition_point(|&nl| nl < offset) as u32;
        let col = offset - if line > 0 { self.newline_offsets[line as usize - 1] + 1 } else { 0 };
        (line + 1, col + 1) // 1-indexed
    }
}
```

### 3.2 String Interning

Symbols, identifiers, and global names appear repeatedly. Intern them once. The interner is **thread-safe** — shared across all scheduler threads via `Arc`:

```rust
/// Interned string handle — 4 bytes, Copy, Eq, Hash.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Symbol(u32);

/// Thread-safe global interner — shared across all scheduler threads.
/// Read-heavy, write-rare: RwLock allows concurrent resolution with
/// infrequent contention during compilation/loading.
/// Strings are leaked to 'static — safe because interning is append-only
/// and the interner lives for the process lifetime.
pub struct Interner {
    map: RwLock<HashMap<&'static str, Symbol>>,
    strings: RwLock<Vec<&'static str>>,
}

impl Interner {
    /// Intern a string. Takes write lock only if the string is new.
    pub fn intern(&self, s: &str) -> Symbol {
        // Fast path: read lock
        if let Some(&sym) = self.map.read().unwrap().get(s) {
            return sym;
        }
        // Slow path: write lock, double-check
        let mut map = self.map.write().unwrap();
        if let Some(&sym) = map.get(s) {
            return sym;
        }
        let mut strings = self.strings.write().unwrap();
        let leaked: &'static str = Box::leak(s.to_string().into_boxed_str());
        let sym = Symbol(strings.len() as u32);
        strings.push(leaked);
        map.insert(leaked, sym);
        sym
    }

    /// Resolve a symbol to its string. Lock-free on the common path via
    /// indexing into the append-only strings vector.
    pub fn resolve(&self, sym: Symbol) -> &str {
        self.strings.read().unwrap()[sym.0 as usize]
    }
}
```

> **Note:** The interner is wrapped in `Arc<Interner>` and shared across all scheduler threads. Since symbol resolution dominates (during VM execution) and interning is rare (only during compilation/loading), the `RwLock` contention is negligible. An alternative is `DashMap`, but the read-heavy pattern favors `RwLock`.

### 3.3 AST (Expr)

Mirrors the TypeScript `Expr` type — a cons-list with atoms and nil:

```rust
#[derive(Debug, Clone)]
pub enum Expr {
    Atom(LispValue, Span),
    List {
        head: Box<Expr>,
        tail: Box<Expr>,
        span: Span,
    },
    Nil(Span),
}

#[derive(Debug, Clone)]
pub enum LispValue {
    Symbol(Symbol),           // Interned
    Number(f64, bool),        // (value, is_float)
    String(Arc<str>),
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Atom(_, s) | Expr::List { span: s, .. } | Expr::Nil(s) => *s,
        }
    }
}

/// Convert a cons-list Expr to a Vec<Expr>.
pub fn list_to_array(expr: &Expr) -> Vec<&Expr> {
    let mut result = Vec::new();
    let mut current = expr;
    while let Expr::List { head, tail, .. } = current {
        result.push(head.as_ref());
        current = tail.as_ref();
    }
    result
}
```

### 3.4 Token

```rust
#[derive(Debug, Clone)]
pub enum Token {
    LParen(Span),
    RParen(Span),
    Identifier(Symbol, Span),
    Number(f64, bool, Span),   // (value, is_float, span)
    String(Arc<str>, Span),
    Quote(Span),               // ' shorthand
    Eof(Span),
}
```

### 3.5 Type System Types

> **Implementation note:** The spec originally used `Symbol` for type variable names, effect names, class names, etc. The actual implementation uses `String` instead — type variables are generated fresh (`t0`, `t1`, ...) and never interned, so coupling them to the `Interner` would add unnecessary complexity. This keeps the type system decoupled from the core crate's interner. The `Interner` is only needed in `infer.rs` to resolve symbols in the AST when dispatching special forms.

```rust
/// Core type representation for HM inference.
#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    Var(String),                                      // type-var
    Const(String),                                    // type-const (Int, Bool, String, ...)
    App { constructor: Box<Type>, args: Vec<Type> },  // type-app (List Int), (Option a)
    Arrow {
        params: Vec<Type>,
        effects: Option<EffectRow>,
        result: Box<Type>,
    },
    Rec { var: String, body: Box<Type> },             // recursive types
}

#[derive(Debug, Clone, PartialEq)]
pub enum EffectRow {
    Closed(Vec<String>),                     // {IO, State}
    Open { effects: Vec<String>, row_var: String }, // {IO | r}
}

#[derive(Debug, Clone)]
pub struct Constraint {
    pub class: String,    // Num, Eq, Ord, Show
    pub type_arg: Type,
}

#[derive(Debug, Clone)]
pub struct QualType {
    pub constraints: Vec<Constraint>,
    pub ty: Type,
}

#[derive(Debug, Clone)]
pub struct TypeScheme {
    pub quantified_vars: Vec<String>,
    pub qual_type: QualType,
}

/// Patterns for match expressions and exhaustiveness checking.
#[derive(Debug, Clone)]
pub enum Pattern {
    Wildcard,
    Var(String),
    Literal(LiteralPattern),
    Constructor { name: String, args: Vec<Pattern> },
}

#[derive(Debug, Clone)]
pub enum LiteralPattern {
    Int(i64),
    Float(f64),
    String(Arc<str>),
    Bool(bool),
}
```

### 3.6 NaN-Boxed Value

The TypeScript VM uses `VMValue = number | VMObject` (unboxed JS numbers). In Rust, use NaN-boxing for a true 8-byte tagged value with zero-allocation arithmetic:

```rust
/// NaN-boxed value — 8 bytes.
///
/// Layout (64-bit IEEE 754):
///   Normal f64          → float value (including +0, -0, Inf, NaN)
///   QNAN | tag | payload → tagged pointer or immediate
///
/// Tags (low bits within QNAN space):
///   QNAN | 1 → nil
///   QNAN | 2 → true
///   QNAN | 3 → false
///   SIGN_BIT | QNAN | 48-bit ptr → object (Arc<VMObject>)
#[repr(transparent)]
pub struct Value(u64);  // Manual Clone/Drop for Arc refcounting

const QNAN: u64          = 0x7FFC_0000_0000_0000;  // Quiet NaN prefix (extra 0x4 bit)
const SIGN_BIT: u64      = 0x8000_0000_0000_0000;
const TAG_NIL: u64       = 1;
const TAG_TRUE: u64      = 2;
const TAG_FALSE: u64     = 3;

impl Value {
    #[inline]
    pub fn number(n: f64) -> Value {
        Value(n.to_bits())
    }

    #[inline]
    pub fn nil() -> Value { Value(QNAN | TAG_NIL) }
    pub fn boolean(b: bool) -> Value { Value(QNAN | if b { TAG_TRUE } else { TAG_FALSE }) }

    pub fn object(obj: VMObject) -> Value {
        let arc = Arc::new(obj);
        let ptr = Arc::into_raw(arc) as u64;
        debug_assert!(ptr & !(0x0000_FFFF_FFFF_FFFF) == 0, "pointer exceeds 48 bits");
        Value(SIGN_BIT | QNAN | ptr)
    }

    #[inline]
    pub fn is_number(&self) -> bool {
        (self.0 & QNAN) != QNAN
    }

    #[inline]
    pub fn is_object(&self) -> bool {
        (self.0 & (SIGN_BIT | QNAN)) == (SIGN_BIT | QNAN)
    }

    #[inline]
    pub fn as_number(&self) -> Option<f64> {
        if self.is_number() { Some(f64::from_bits(self.0)) } else { None }
    }

    pub fn as_object(&self) -> Option<&VMObject> {
        if self.is_object() {
            let ptr = (self.0 & 0x0000_FFFF_FFFF_FFFF) as *const VMObject;
            unsafe { Some(&*ptr) }
        } else {
            None
        }
    }

    #[inline]
    pub fn is_truthy(&self) -> bool {
        // Only nil and #f are falsy (matches TS behavior)
        !self.is_nil() && !self.is_false()
    }
}

impl Drop for Value {
    fn drop(&mut self) {
        if self.is_object() {
            let ptr = (self.0 & 0x0000_FFFF_FFFF_FFFF) as *const VMObject;
            unsafe { Arc::decrement_strong_count(ptr); }
        }
    }
}

impl Clone for Value {
    fn clone(&self) -> Self {
        if self.is_object() {
            let ptr = (self.0 & 0x0000_FFFF_FFFF_FFFF) as *const VMObject;
            unsafe { Arc::increment_strong_count(ptr); }
        }
        Value(self.0)
    }
}
```

> **Platform assumptions:** NaN-boxing relies on 48-bit virtual addresses (x86-64 canonical addresses, ARM64 default). For WASM (32-bit pointers), a tagged enum fallback is used — see [Section 15: WASM & Web Editor](#15-wasm--web-editor) for details.
>
> **Implementation note (Phase 3):** The NaN-boxing uses `QNAN = 0x7FFC_0000_0000_0000` (not `0x7FF8` from the original spec). The extra `0x4` bit ensures all tagged values sit firmly in the quiet NaN space. Objects use the sign bit + QNAN + 48-bit pointer. The `Clone`/`Drop` impls use `Arc::increment_strong_count` / `Arc::decrement_strong_count` (cleaner than the original `from_raw` + `forget` pattern). `as_object()` returns `&VMObject` (borrow, no refcount change) instead of `Arc<VMObject>` (clone) — more efficient for read access.

### 3.7 VMObject

Heap-allocated objects behind `Arc<VMObject>`:

```rust
/// Heap-allocated Marlang values.
/// Arc provides reference counting — replaces JS garbage collection.
pub enum VMObject {
    String(Arc<str>),
    List(Vec<Value>),
    AdtValue {
        tag: Symbol,
        fields: Box<[Value]>,
    },
    AdtConstructor {
        tag: Symbol,
        arity: u8,
    },
    Closure(Box<Closure>),
    Continuation(Box<VMContinuation>),
    Builtin {
        name: Symbol,
        func: BuiltinFn,
    },

    // Concurrency & IO (BEAM model: Pid for processes, Port for all IO resources)
    Pid(u64),
    Port(u64),        // Channels, TCP sockets, listeners, HTTP servers, etc.

    // FFI (see Section 9.1)
    FfiLibrary(Box<FfiLibrary>),
    FfiPointer(*mut u8),
    FfiStructDef(Arc<FfiStructDef>),
    FfiStruct(Box<FfiStruct>),
}

// Phase 3 (compiler only): simple signature without VM access
pub type BuiltinFn = fn(&[Value]) -> Value;
// Phase 4 (execution engine) will extend to:
// pub type BuiltinFn = fn(&mut VM, &[Value]) -> Result<Value, VMError>;
```

### 3.8 Bytecode

```rust
/// Opcodes — mirrors the TypeScript Op enum exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Op {
    PushConst = 0,     // [idx]
    PushLocal = 1,     // [slot]
    SetLocal = 2,      // [slot]
    PushUpvalue = 3,   // [idx]
    PushGlobal = 4,    // [name_idx]
    SetGlobal = 5,     // [name_idx]
    Pop = 6,
    Dup = 7,

    Add = 10,
    Sub = 11,
    Mul = 12,
    Div = 13,
    Neg = 14,

    Eq = 20,
    Lt = 21,
    Gt = 22,
    Le = 23,
    Ge = 24,
    Not = 25,

    Jump = 30,         // [target]
    JumpIfFalse = 31,  // [target]
    Return = 33,

    Call = 40,         // [arity]
    TailCall = 41,     // [arity]
    Closure = 42,      // [proto_idx]

    Construct = 50,    // [name_idx, arity]
    GetField = 51,     // [field_idx]
    MatchTag = 52,     // [name_idx, jump_target]

    PushHandler = 60,  // [handler_info_idx]
    PopHandler = 61,
    Perform = 62,      // [name_idx, arity]
    Resume = 63,

    Halt = 99,
}

/// Single bytecode instruction with optional operands.
#[derive(Debug, Clone, Copy)]
pub struct Instruction {
    pub op: Op,
    pub operand: u32,   // Primary operand (0 if unused)
    pub operand2: u32,  // Secondary operand (MatchTag, Construct)
    pub line: u32,      // Source line for error reporting
}

/// Constant pool entry — pre-converted to Value at load time (Phase 4).
/// During compilation (Phase 3), uses ConstantValue enum for clean separation.
pub struct Chunk {
    pub code: Vec<Instruction>,
    pub constants: Vec<ConstantValue>,  // Compile-time; preconvert_constants() for Phase 4
    pub local_count: u32,
    pub source_file: Option<String>,
}

/// A compiled function template — instantiated into closures at runtime.
pub struct FunctionPrototype {
    pub name: Symbol,
    pub arity: u8,
    pub upvalues: Vec<UpvalueDescriptor>,
    pub chunk: Chunk,
}

#[derive(Debug, Clone, Copy)]
pub struct UpvalueDescriptor {
    pub is_local: bool,  // Captured from enclosing local (true) or upvalue (false)
    pub index: u16,
}
```

### 3.9 Call Frames and Closures

```rust
/// A closure: function prototype + captured upvalues.
pub struct Closure {
    pub proto: Arc<FunctionPrototype>,
    pub upvalues: Box<[Value]>,
}

/// A single activation record on the call stack.
pub struct CallFrame {
    pub closure: Arc<Closure>,
    pub ip: usize,
    pub locals: Vec<Value>,
}

/// Effect handler frame — installed by PUSH_HANDLER, removed by POP_HANDLER.
pub struct VMHandlerFrame {
    pub handlers: HashMap<Symbol, HandlerEntry>,
    pub return_handler: Option<Arc<Closure>>,
    // Snapshot for unwinding on PERFORM
    pub unwind_frame_count: usize,
    pub unwind_stack_top: usize,
    // Mutable capture window (updated by RESUME)
    pub capture_frame_start: usize,
    pub capture_stack_start: usize,
}

pub struct HandlerEntry {
    pub closure: Arc<Closure>,
    pub arity: u8,
}
```

### 3.10 Continuations

```rust
/// One-shot delimited continuation — captured by PERFORM, invoked by RESUME.
pub struct VMContinuation {
    pub frames: Vec<CallFrame>,
    pub stack: Vec<Value>,
    pub handler_stack: Vec<VMHandlerFrame>,
    pub resumed: bool,
    pub frame_base: usize,
    pub stack_base: usize,
}
```

---

## 4. Lexer & Parser ✅

### Lexer

A mutable-cursor lexer, matching the TypeScript `Lexer.lex()` function:

```rust
pub struct Lexer<'src> {
    source: &'src str,
    bytes: &'src [u8],
    pos: usize,
    interner: &'src mut Interner,
}

impl<'src> Lexer<'src> {
    pub fn new(source: &'src str, interner: &'src mut Interner) -> Self { /* ... */ }

    pub fn lex_all(&mut self) -> Result<Vec<Token>, LexError> { /* ... */ }

    fn next_token(&mut self) -> Result<Token, LexError> {
        self.skip_whitespace_and_comments();
        match self.peek() {
            None => Ok(Token::Eof(self.span_here())),
            Some(b'(') => { self.advance(); Ok(Token::LParen(self.span_prev())) }
            Some(b')') => { self.advance(); Ok(Token::RParen(self.span_prev())) }
            Some(b'"') => self.lex_string(),
            Some(b) if b.is_ascii_digit() || (*b == b'-' && self.peek_next_is_digit()) => {
                self.lex_number()
            }
            _ => self.lex_identifier(),
        }
    }
}
```

Key behaviors to preserve:
- `;` starts a line comment (to newline)
- `#t` / `#f` are boolean literals (lexed as identifiers, handled by parser)
- Strings support `\"` and `\\` escapes
- Numbers: `42` (int), `3.14` (float), `-7` (negative)
- Identifiers: any non-whitespace, non-paren, non-quote sequence

### Parser

Recursive descent, building the same cons-list AST:

```rust
pub struct Parser<'t> {
    tokens: &'t [Token],
    pos: usize,
}

impl<'t> Parser<'t> {
    pub fn parse_all(&mut self) -> Result<Vec<Expr>, ParseError> {
        let mut exprs = Vec::new();
        while !self.at_eof() {
            exprs.push(self.parse_expr()?);
        }
        Ok(exprs)
    }

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        match self.peek() {
            Token::LParen(_) => self.parse_list(),
            Token::Identifier(sym, span) => {
                self.advance();
                Ok(Expr::Atom(LispValue::Symbol(*sym), *span))
            }
            Token::Number(n, is_float, span) => {
                self.advance();
                Ok(Expr::Atom(LispValue::Number(*n, *is_float), *span))
            }
            Token::String(s, span) => {
                self.advance();
                Ok(Expr::Atom(LispValue::String(s.clone()), *span))
            }
            _ => Err(ParseError::Unexpected(self.peek().span())),
        }
    }

    fn parse_list(&mut self) -> Result<Expr, ParseError> {
        let start = self.expect_lparen()?;
        let mut elements = Vec::new();
        while !self.check_rparen() {
            elements.push(self.parse_expr()?);
        }
        let end = self.expect_rparen()?;
        let span = Span { start: start.start, end: end.end };

        // Build cons-list from elements (right fold)
        let mut result = Expr::Nil(span);
        for elem in elements.into_iter().rev() {
            result = Expr::List {
                head: Box::new(elem),
                tail: Box::new(result),
                span,
            };
        }
        Ok(result)
    }
}
```

---

## 5. Type System ✅

The type system is a Hindley-Milner inference engine with typeclasses, algebraic effects, and exhaustiveness checking. It's entirely pure-functional (no mutation of the inference state) making it a clean fit for Rust.

### Architecture

```
types/
├── ast.rs       — Type, TypeScheme, Constraint, Pattern, EffectRow, fresh var generation
├── env.rs       — TypeEnv (immutable bindings map), Substitution, generalize, instantiate
├── unify.rs     — Robinson's unification + effect row unification
├── infer.rs     — HM inference, all special forms, patterns, exhaustiveness
├── builtins.rs  — Built-in types (Int, Float, Bool, String), classes (Num, Eq, Ord, Show), 70+ builtins
├── checker.rs   — Top-level type_check() entry point
└── errors.rs    — TypeError enum (19 variants), format_error()
```

### Type Environment

```rust
/// Immutable type environment. Clone-on-extend for lexical scoping.
#[derive(Clone)]
pub struct TypeEnv {
    pub bindings: HashMap<String, TypeScheme>,
    pub data_types: HashMap<String, DataType>,
    pub constructors: HashMap<String, (DataType, Constructor)>,
    pub classes: HashMap<String, TypeClass>,
    pub instances: HashMap<String, Vec<ClassInstance>>,
    pub aliases: HashMap<String, TypeAlias>,
    pub effects: HashMap<String, EffectDeclaration>,
    pub parent: Option<Box<TypeEnv>>,
}

/// Substitution: mapping from type variables to types.
pub type Substitution = HashMap<String, Type>;

pub fn apply_subst(subst: &Substitution, ty: &Type) -> Type { /* ... */ }
pub fn compose_subst(s1: &Substitution, s2: &Substitution) -> Substitution { /* ... */ }
```

### Unification

```rust
/// Robinson's unification with cycle detection.
/// Creates mu-types on occurs check instead of failing (supports recursive data structures).
pub fn unify(t1: &Type, t2: &Type) -> Result<Substitution, TypeError> { /* ... */ }
pub fn unify_with_context(t1: &Type, t2: &Type, context: &str) -> Result<Substitution, TypeError> { /* ... */ }
pub fn unify_many(pairs: &[(Type, Type)]) -> Result<Substitution, TypeError> { /* ... */ }
pub fn match_types(pattern: &Type, target: &Type) -> Result<Substitution, TypeError> { /* ... */ }
pub fn unify_effect_rows(r1: Option<&EffectRow>, r2: Option<&EffectRow>) -> Result<EffectRowSubstitution, TypeError> { /* ... */ }
```

### Inference

```rust
pub struct InferResult {
    pub ty: Type,
    pub subst: Substitution,
    pub constraints: Vec<Constraint>,
    pub effects: EffectRow,
}

pub fn infer_expr(env: &TypeEnv, expr: &Expr, interner: &Interner) -> Result<InferResult, TypeError> { /* ... */ }

pub fn infer_program(
    env: &TypeEnv,
    exprs: &[Expr],
    interner: &Interner,
) -> Result<(TypeEnv, Vec<Type>), TypeError> { /* ... */ }
```

### Exhaustiveness Checking

Pattern matching exhaustiveness follows the same algorithm as TypeScript: for each `match` expression, compute the set of uncovered constructors after all arms and report an error if non-empty. The `Pattern` type maps directly (Section 3.5).

### Effect Row Enforcement

Effect rows are enforced during type checking. Every `Arrow` type carries an `Option<EffectRow>` where:

- `Some(Closed([]))` — pure function (no effects)
- `Some(Closed(["IO", "Exception"]))` — function that performs exactly those effects
- `Some(Open { effects: ["IO"], row_var: "ε0" })` — performs IO plus any other effects (row-polymorphic)
- `None` — treated as pure (same as `Closed([])`) during unification

**Inference:** `infer_lambda` always produces `Some(EffectRow)` — pure bodies get `Closed([])`, effectful bodies get their collected effects. `infer_application` constructs the expected function type with `Some(Open([], fresh_ε))` so the application site's effect expectations unify properly with the callee's actual effects.

**Substitution:** Effect row variables (`ε0`, `ε1`, ...) are stored in the regular `Substitution` as entries mapping to `Type::EffectRowRef(EffectRow)`. `apply_subst` recurses into effect rows in Arrow types, chasing substitution chains and splicing rows together. `instantiate` replaces quantified effect row vars (detected by `ε` prefix) with fresh open rows.

**Unification:** Arrow unification (`unify_internal`) unifies effect rows after unifying params and result. `unify_effect_rows_into_subst` handles all row combinations (closed-closed, open-closed, closed-open, open-open) and produces `Substitution` entries using `Type::EffectRowRef`.

**Parser syntax:** The lexer supports `{` and `}` tokens and `|` as a symbol char. The parser (`parse_with_interner`) converts `{IO Exception | e}` into an `(__effect_row IO Exception __row_var:e)` AST form. `parse_type_expr` in `infer.rs` recognizes this form and produces an `EffectRow`. In arrow types: `(-> String {IO} Unit)`.

**Semantics:**
- `(-> String String)` with no effect annotation means **pure** (rejects effectful bodies)
- `(-> String {} String)` — explicit pure (same behavior)
- `(-> String {IO} Unit)` — must perform exactly IO
- `(-> String {IO | e} Unit)` — must perform IO, may perform others

Builtins that accept effectful callbacks (`fiber-spawn`, `run-fibers`, `run-fibers-parallel`, `time-it`) are typed with open effect row variables so they accept both pure and effectful functions.

### Error Types

```rust
pub enum TypeError {
    UnificationError { t1: Type, t2: Type, context: Option<String>, span: Option<Span> },
    OccursCheck { var_name: String, ty: Type, span: Option<Span> },
    UndefinedVariable { name: String, span: Option<Span> },
    UndefinedType { name: String, span: Option<Span> },
    UndefinedConstructor { name: String, span: Option<Span> },
    UndefinedClass { name: String, span: Option<Span> },
    NoInstance { constraint: Constraint, span: Option<Span> },
    DuplicateDefinition { name: String, what: String, span: Option<Span> },
    ArityMismatch { expected: usize, got: usize, context: String, span: Option<Span> },
    NotAFunction { ty: Type, span: Option<Span> },
    PatternTypeMismatch { pattern: Pattern, expected: Type, got: Type, span: Option<Span> },
    NonExhaustivePatterns { missing: Vec<String>, span: Option<Span> },
    OverlappingInstances { class_name: String, ty: Type, span: Option<Span> },
    MissingTypeAnnotation { name: String, span: Option<Span> },
    InvalidTypeAnnotation { message: String, span: Option<Span> },
    AmbiguousType { var_name: String, constraints: Vec<Constraint>, span: Option<Span> },
    CyclicTypeAlias { name: String, span: Option<Span> },
    KindError { message: String, span: Option<Span> },
    InternalError { message: String, span: Option<Span> },
}

pub fn format_error(err: &TypeError) -> String { /* ... */ }
```

---

## 6. Bytecode Compiler ✅

The compiler transforms `Vec<Expr>` into a `Chunk`. Architecture matches TypeScript exactly:

### Compiler State

```rust
pub struct Compiler<'a> {
    /// Current chunk being compiled into.
    chunk: Chunk,
    /// Local variable slots for the current scope.
    locals: Vec<Local>,
    local_count: u32,
    /// Upvalue descriptors for the current function.
    upvalues: Vec<UpvalueDescriptor>,
    /// Snapshot of parent scope for upvalue resolution.
    enclosing: Option<Box<ParentScope>>,
    /// String interner (shared, immutable reference).
    interner: &'a Interner,
    /// Pre-interned known symbols for fast dispatch.
    known_symbols: &'a KnownSymbols,
    /// Scope depth for variable resolution.
    scope_depth: u32,
    /// Known data constructors (for pattern matching).
    known_constructors: HashSet<Symbol>,
    /// Known effect operations (opName → effectName).
    known_effect_ops: HashMap<Symbol, Symbol>,
}

struct Local {
    name: Symbol,
    depth: u32,
    slot: u32,
}

/// Snapshot of parent's locals+upvalues for child upvalue resolution.
/// Avoids Rust's borrow-checker issues with mutable enclosing references.
struct ParentScope {
    locals: Vec<Local>,
    upvalues: Vec<UpvalueDescriptor>,
    enclosing: Option<Box<ParentScope>>,
}
```

### Key Compilation Rules

```rust
impl Compiler<'_> {
    pub fn compile_program(exprs: &[Expr], interner: &mut Interner) -> Chunk {
        let mut compiler = Compiler::new(interner);
        for expr in exprs {
            compiler.compile_expr(expr);
        }
        compiler.emit(Op::Halt, 0, 0);
        compiler.chunk
    }

    fn compile_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Atom(LispValue::Number(n, _), _) => {
                let idx = self.add_constant(Value::from_f64(*n));
                self.emit(Op::PushConst, idx, 0);
            }
            Expr::Atom(LispValue::Symbol(sym), _) => {
                self.compile_variable_lookup(*sym);
            }
            Expr::List { .. } => {
                let elements = list_to_array(expr);
                self.compile_form(&elements);
            }
            _ => { /* nil literal */ }
        }
    }

    fn compile_form(&mut self, elements: &[&Expr]) {
        // Dispatch on first element (special forms)
        match get_symbol_name(elements[0]) {
            Some("define") => self.compile_define(elements),
            Some("lambda") => self.compile_lambda(elements),
            Some("if")     => self.compile_if(elements),
            Some("let")    => self.compile_let(elements),
            Some("begin")  => self.compile_begin(elements),
            Some("cond")   => self.compile_cond(elements),
            Some("match")  => self.compile_match(elements),
            Some("data")   => self.compile_data(elements),
            Some("handle") => self.compile_handle(elements),
            Some("perform")=> self.compile_perform(elements),
            Some("import") => self.compile_import(elements),
            _              => self.compile_call(elements),
        }
    }
}
```

### Binary Operator Optimization

Exactly as in TypeScript — when a known operator is applied to exactly 2 arguments, emit a direct opcode instead of a CALL:

```rust
const BINARY_OPS: &[(&str, Op)] = &[
    ("+", Op::Add), ("-", Op::Sub), ("*", Op::Mul), ("/", Op::Div),
    ("<", Op::Lt), (">", Op::Gt), ("=", Op::Eq), ("<=", Op::Le), (">=", Op::Ge),
];

fn compile_call(&mut self, elements: &[&Expr]) {
    if let Some(name) = get_symbol_name(elements[0]) {
        if elements.len() == 3 {
            if let Some(&(_, op)) = BINARY_OPS.iter().find(|(n, _)| *n == name) {
                self.compile_expr(elements[1]);
                self.compile_expr(elements[2]);
                self.emit(op, 0, 0);
                return;
            }
        }
    }
    // General case: compile callee, args, CALL
    for elem in elements {
        self.compile_expr(elem);
    }
    self.emit(Op::Call, (elements.len() - 1) as u32, 0);
}
```

### Pattern Matching

Pattern compilation returns fail-patches for the caller to wire up:

```rust
pub struct FailPatch {
    pub idx: usize,
    pub kind: FailPatchKind,
}

pub enum FailPatchKind {
    Jump,     // Patch operand of a JUMP instruction
    MatchTag, // Patch operand2 of a MATCH_TAG instruction
}

fn compile_pattern(&mut self, pattern: &Expr) -> Vec<FailPatch> {
    // Emit MATCH_TAG for constructor patterns, GET_FIELD for nested fields
    // Return patches to be filled in by compile_match() with the next-arm address
    /* ... */
}
```

---

## 7. VM Execution Engine ✅

### VM State

Each scheduler thread owns its own `VM` instance. The VM is **not** `Send` or `Sync` — it is thread-local. Shared state (interner, globals, process registry) is accessed through `Arc`-wrapped structures:

```rust
/// Per-thread VM state. Each scheduler thread owns one.
/// NOT Send/Sync — exclusively owned by its scheduler thread.
pub struct VM {
    pub stack: Vec<Value>,
    pub stack_top: usize,
    pub frames: Vec<CallFrame>,
    pub frame_count: usize,
    pub handler_stack: Vec<VMHandlerFrame>,

    /// Thread-local copy of globals. Populated at startup from SharedState.
    /// Fibers running on this thread see this snapshot. New defines are
    /// broadcast to other schedulers via the shared globals (eventual consistency).
    pub globals: HashMap<Symbol, Value>,

    /// Shared state — Arc-cloned from the runtime.
    pub shared: Arc<SharedState>,
}

/// State shared across all scheduler threads (read-heavy, write-rare).
pub struct SharedState {
    pub interner: Interner,
    pub globals: RwLock<HashMap<Symbol, Value>>,
    pub process_registry: ProcessRegistry,
    pub channel_registry: ChannelRegistry,
}
```

### Effect Handling: RunResult Instead of Exceptions

The TypeScript VM uses `throw new EffectSignal(...)` caught by `runWithHandler()`. Rust has no exceptions. Instead, `run()` returns a `RunResult` enum:

```rust
pub enum RunResult {
    /// Normal completion — value on top of stack.
    Ok(Value),
    /// An effect was performed — handler must process it.
    Effect(EffectSignal),
}

pub struct EffectSignal {
    pub op_name: Symbol,
    pub args: Vec<Value>,
    pub continuation: VMContinuation,
    pub target_handler: usize, // Index into handler_stack
}
```

### Main Execution Loop

```rust
impl VM {
    /// Execute bytecode starting from the current frame.
    /// Returns when RETURN pops below `base_frame_count` or an unhandled effect fires.
    pub fn run(&mut self, base_frame_count: usize) -> RunResult {
        loop {
            let frame = &mut self.frames[self.frame_count - 1];
            let instr = frame.closure.proto.chunk.code[frame.ip];
            frame.ip += 1;

            match instr.op {
                Op::PushConst => {
                    let val = frame.closure.proto.chunk.constants[instr.operand as usize];
                    self.push(val);
                }
                Op::Add => {
                    let b = self.pop();
                    let a = self.pop();
                    // Fast path: both are f64
                    if let (Some(a), Some(b)) = (a.as_f64(), b.as_f64()) {
                        self.push(Value::from_f64(a + b));
                    } else {
                        // String concatenation or error
                        self.push(self.add_values(a, b)?);
                    }
                }
                Op::Call => {
                    let arity = instr.operand as usize;
                    self.call(arity)?;
                }
                Op::TailCall => {
                    let arity = instr.operand as usize;
                    self.tail_call(arity)?;
                }
                Op::Return => {
                    let result = self.pop();
                    self.frame_count -= 1;
                    if self.frame_count <= base_frame_count {
                        return RunResult::Ok(result);
                    }
                    self.stack_top = self.frames[self.frame_count].closure.proto.chunk.local_count as usize;
                    self.push(result);
                }
                Op::Perform => {
                    return self.perform_effect(instr);
                }
                Op::PushHandler => {
                    // Parse handler descriptor, install handler frame
                    // Then call run_with_handler() which loops on RunResult::Effect
                    return self.push_handler(instr, base_frame_count);
                }
                // ... all other opcodes
                Op::Halt => return RunResult::Ok(self.pop()),
                _ => unreachable!("unknown opcode: {:?}", instr.op),
            }
        }
    }
}
```

### Handler Execution

```rust
impl VM {
    /// Install a handler and run the body. Catches EffectSignal results
    /// and dispatches to the appropriate handler closure.
    fn run_with_handler(&mut self, base_frame_count: usize) -> RunResult {
        loop {
            match self.run(base_frame_count) {
                RunResult::Ok(value) => {
                    // Body completed normally — apply return handler if present
                    let handler_frame = self.handler_stack.pop().unwrap();
                    if let Some(return_fn) = handler_frame.return_handler {
                        return self.call_closure(&return_fn, &[value]);
                    }
                    return RunResult::Ok(value);
                }
                RunResult::Effect(signal) => {
                    // Check if this handler handles the effect
                    if let Some(entry) = self.handler_stack.last()
                        .and_then(|h| h.handlers.get(&signal.op_name))
                    {
                        // Build args: [continuation, ...effect_args]
                        let cont_val = Value::from_object(Arc::new(
                            VMObject::Continuation(Box::new(signal.continuation))
                        ));
                        let mut args = vec![cont_val];
                        args.extend_from_slice(&signal.args);

                        let closure = entry.closure.clone();
                        // Deep handler: recursively run_with_handler for the handler body
                        return self.call_closure(&closure, &args);
                    } else {
                        // Not our effect — propagate upward
                        return RunResult::Effect(signal);
                    }
                }
            }
        }
    }

    /// Call a closure with given arguments, push frame, run.
    pub fn call_closure(&mut self, closure: &Arc<Closure>, args: &[Value]) -> RunResult {
        let frame = CallFrame {
            closure: closure.clone(),
            ip: 0,
            locals: {
                let mut locals = vec![Value::NIL; closure.proto.chunk.local_count as usize];
                for (i, arg) in args.iter().enumerate() {
                    locals[i] = *arg;
                }
                locals
            },
        };
        self.frames.push(frame);
        self.frame_count += 1;
        self.run(self.frame_count - 1)
    }

    /// Resume a one-shot continuation with a value.
    pub fn resume_continuation(&mut self, cont: &mut VMContinuation, value: Value) -> RunResult {
        assert!(!cont.resumed, "continuation already resumed (one-shot)");
        cont.resumed = true;

        // Restore captured frames, stack, and handler stack
        let base = self.frame_count;
        self.frames.extend(cont.frames.drain(..));
        self.frame_count = self.frames.len();
        self.stack.extend(cont.stack.drain(..));
        self.stack_top = self.stack.len();
        self.handler_stack.extend(cont.handler_stack.drain(..));

        // Push the resume value
        self.push(value);

        self.run(base)
    }
}
```

### Default IO Handlers

Built-in IO effects that work without an explicit `handle` block (matching `tryDefaultHandler()` in TS):

```rust
fn try_default_handler(vm: &mut VM, op_name: Symbol, args: &[Value]) -> Option<Value> {
    let name = vm.shared.interner.resolve(op_name);
    match name {
        "print"      => { print!("{}", format_value(args[0])); Some(Value::NIL) }
        "println"    => { println!("{}", format_value(args[0])); Some(Value::NIL) }
        "read-line"  => { /* read from stdin */ }
        "read-file"  => { /* std::fs::read_to_string */ }
        "write-file" => { /* std::fs::write */ }
        _ => None,
    }
}
```

---

## 8. Concurrency

### Architecture: BEAM-Inspired Multi-Scheduler

The TypeScript scheduler (`src/vm/scheduler.ts`) is a single-threaded async trampoline. The Rust port goes further: a **multi-scheduler architecture** inspired by Erlang/BEAM, where N scheduler threads each own a VM instance and cooperatively execute fibers across all available cores.

```
┌──────────────────────────────────────────────────────────┐
│                    SharedState (Arc)                       │
│  ┌───────────┐ ┌─────────────────┐ ┌──────────────────┐ │
│  │ Interner  │ │ ProcessRegistry │ │ ChannelRegistry  │ │
│  │ (RwLock)  │ │   (DashMap)     │ │   (DashMap)      │ │
│  └───────────┘ └─────────────────┘ └──────────────────┘ │
│  ┌────────────────────┐ ┌──────────────────────────────┐ │
│  │ Globals (RwLock)   │ │ Global PID Counter (AtomicU64)│ │
│  └────────────────────┘ └──────────────────────────────┘ │
└──────────────────────────────────────────────────────────┘
        │              │              │              │
  ┌─────┴─────┐  ┌─────┴─────┐  ┌─────┴─────┐     │
  │ Sched #0  │  │ Sched #1  │  │ Sched #N  │  (one per core)
  │ ┌───────┐ │  │ ┌───────┐ │  │ ┌───────┐ │
  │ │ VM #0 │ │  │ │ VM #1 │ │  │ │ VM #N │ │
  │ └───────┘ │  │ └───────┘ │  │ └───────┘ │
  │ ┌───────┐ │  │ ┌───────┐ │  │ ┌───────┐ │
  │ │ Ready │ │  │ │ Ready │ │  │ │ Ready │ │
  │ │ Queue │ │  │ │ Queue │ │  │ │ Queue │ │
  │ └───────┘ │  │ └───────┘ │  │ └───────┘ │
  │ ┌───────┐ │  │ ┌───────┐ │  │ ┌───────┐ │
  │ │ Steal │ │  │ │ Steal │ │  │ │ Steal │ │
  │ │ Deque │ │  │ │ Deque │ │  │ │ Deque │ │
  │ └───────┘ │  │ └───────┘ │  │ └───────┘ │
  │  inbox ◄──┼──┼───────────┼──┼───────────│
  └───────────┘  └───────────┘  └───────────┘
```

**Key design principles:**

1. **Shared-nothing VM execution** — each scheduler thread owns a `VM` exclusively (`&mut self`). No locks during bytecode execution.
2. **Message passing between threads** — actor sends and channel operations crossing thread boundaries go through lock-free MPSC queues.
3. **Work stealing** — idle schedulers steal fibers from busy ones via `crossbeam-deque`.
4. **Cooperative within each thread** — fibers yield at effect boundaries (same as TypeScript). No preemption.
5. **Values are `Send`** — `Arc<VMObject>` is `Send + Sync`, so values can safely cross thread boundaries in messages.

### Scheduler Thread

```rust
/// One scheduler thread — owns a VM and runs fibers cooperatively.
/// N of these run in parallel (one per core).
pub struct Scheduler {
    id: usize,
    vm: VM,

    /// Local ready queue — LIFO for cache locality.
    ready_queue: VecDeque<Fiber>,
    /// Work-stealing deque — other schedulers steal from here.
    steal_deque: crossbeam_deque::Worker<Fiber>,
    /// Stealers for all other schedulers (read-only after init).
    other_stealers: Vec<crossbeam_deque::Stealer<Fiber>>,

    /// Inbox for cross-thread messages (fibers migrated from other threads,
    /// actor messages, channel wakeups).
    inbox: crossbeam_channel::Receiver<SchedulerMessage>,
    /// Sender cloned and given to SharedState for routing.
    inbox_sender: crossbeam_channel::Sender<SchedulerMessage>,

    /// Pending async IO for this scheduler's fibers.
    pending_io: HashMap<u64, PendingIO>,
    active_servers: usize,

    /// Parked fibers waiting on actor receive, channel recv, TCP accept, etc.
    parked: HashMap<u64, ParkedFiber>,

    /// Local actor state (mailboxes for PIDs owned by this scheduler).
    mailboxes: HashMap<u64, VecDeque<Value>>,
    monitors: HashMap<u64, Vec<u64>>,
    links: HashMap<u64, HashSet<u64>>,
    trap_exit: HashSet<u64>,

    /// Result of the main fiber (scheduler 0 only).
    result: Option<Value>,
}

/// A fiber — must be `Send` so it can be stolen by another scheduler thread.
/// This is guaranteed because:
///   - `Arc<Closure>` is Send (Arc<T> is Send when T: Send + Sync)
///   - `VMContinuation` contains Vec<CallFrame>, Vec<Value>, Vec<VMHandlerFrame> — all Send
///   - `Value` (u64 NaN-boxed) is Send; object pointers are Arc<VMObject> which is Send
pub enum Fiber {
    New { closure: Arc<Closure>, pid: u64 },
    Suspended { continuation: VMContinuation, resume_value: Value, pid: u64 },
}

impl Fiber {
    pub fn pid(&self) -> u64 {
        match self {
            Fiber::New { pid, .. } | Fiber::Suspended { pid, .. } => *pid,
        }
    }
}

/// Messages sent between scheduler threads.
pub enum SchedulerMessage {
    /// A fiber migrated to this scheduler (from work stealing or spawn).
    Fiber(Fiber),
    /// An actor message delivered to a PID on this scheduler.
    ActorMessage { target_pid: u64, message: Value },
    /// A channel send to a channel with a parked receiver on this scheduler.
    ChannelWake { channel_id: u64, value: Value },
    /// A fiber's IO completed — wake it with the result.
    IoComplete { fiber_pid: u64, result: Value },
    /// Shutdown signal.
    Shutdown,
}

pub enum PendingIO {
    TcpConnect(tokio::task::JoinHandle<Result<TcpStream, io::Error>>),
    HttpFetch(tokio::task::JoinHandle<Result<HttpResponse, reqwest::Error>>),
    // ...
}
```

### Shared Registries

```rust
/// Maps PID → scheduler ID. Used to route actor messages and monitor notifications.
pub struct ProcessRegistry {
    /// PID → (scheduler_id, is_alive)
    pids: DashMap<u64, ProcessEntry>,
    next_pid: AtomicU64,
}

struct ProcessEntry {
    scheduler_id: usize,
    alive: bool,
}

impl ProcessRegistry {
    /// Register a new process on a scheduler. Returns the PID.
    pub fn spawn(&self, scheduler_id: usize) -> u64 {
        let pid = self.next_pid.fetch_add(1, Ordering::Relaxed);
        self.pids.insert(pid, ProcessEntry { scheduler_id, alive: true });
        pid
    }

    /// Look up which scheduler owns a PID.
    pub fn lookup(&self, pid: u64) -> Option<usize> {
        self.pids.get(&pid).map(|e| e.scheduler_id)
    }

    /// Mark a process as dead (for monitors/links).
    pub fn mark_dead(&self, pid: u64) {
        if let Some(mut entry) = self.pids.get_mut(&pid) {
            entry.alive = false;
        }
    }
}

/// Global channel state — channels may span scheduler threads.
pub struct ChannelRegistry {
    channels: DashMap<u64, ChannelState>,
    next_id: AtomicU64,
}

pub struct ChannelState {
    buffer: VecDeque<Value>,
    /// Scheduler ID of a parked receiver (if any).
    parked_receiver: Option<(usize, u64)>, // (scheduler_id, fiber_pid)
    /// Scheduler ID of a parked sender (if any, for bounded channels).
    parked_sender: Option<(usize, u64)>,
    closed: bool,
}
```

### Main Scheduler Loop

Each scheduler thread runs this loop independently:

```rust
impl Scheduler {
    pub async fn run(mut self) {
        loop {
            // 1. DRAIN INBOX: process cross-thread messages
            self.drain_inbox();

            // 2. SYNC PHASE: run all ready fibers (cooperative, no preemption)
            while let Some(fiber) = self.ready_queue.pop_front() {
                self.run_fiber(fiber);
                // After each fiber, check inbox for urgent messages
                self.drain_inbox();
            }

            // 3. CHECK TERMINATION
            if self.is_idle() {
                // Notify runtime we're idle (for global termination detection)
                if self.try_steal() {
                    continue; // Stole work — keep going
                }
                // No work anywhere — wait for inbox or IO
            }

            // 4. DRAIN PHASE: await IO completion or inbox messages
            if !self.pending_io.is_empty() || self.active_servers > 0 {
                self.drain_pending_io().await;
            } else {
                // Park until inbox has a message
                self.wait_for_inbox().await;
            }
        }
    }

    fn drain_inbox(&mut self) {
        while let Ok(msg) = self.inbox.try_recv() {
            match msg {
                SchedulerMessage::Fiber(fiber) => {
                    self.ready_queue.push_back(fiber);
                }
                SchedulerMessage::ActorMessage { target_pid, message } => {
                    self.deliver_actor_message(target_pid, message);
                }
                SchedulerMessage::ChannelWake { channel_id, value } => {
                    self.wake_channel_receiver(channel_id, value);
                }
                SchedulerMessage::IoComplete { fiber_pid, result } => {
                    self.wake_fiber(fiber_pid, result);
                }
                SchedulerMessage::Shutdown => return,
            }
        }
    }

    fn run_fiber(&mut self, fiber: Fiber) {
        let pid = fiber.pid();
        // Save/restore VM state for fiber isolation
        let saved_frame_count = self.vm.frame_count;
        let saved_stack_top = self.vm.stack_top;

        let result = match fiber {
            Fiber::New { closure, .. } => self.vm.call_closure(&closure, &[]),
            Fiber::Suspended { mut continuation, resume_value, .. } => {
                self.vm.resume_continuation(&mut continuation, resume_value)
            }
        };

        match result {
            RunResult::Ok(value) => {
                self.fiber_completed(pid, value);
            }
            RunResult::Effect(signal) => {
                self.handle_effect(pid, signal);
            }
        }

        // Restore VM state
        self.vm.frame_count = saved_frame_count;
        self.vm.stack_top = saved_stack_top;
    }
}
```

### Work Stealing

When a scheduler's ready queue is empty, it tries to steal fibers from other schedulers:

```rust
impl Scheduler {
    fn try_steal(&self) -> bool {
        // Try each other scheduler in random order
        let start = fastrand::usize(..self.other_stealers.len());
        for i in 0..self.other_stealers.len() {
            let idx = (start + i) % self.other_stealers.len();
            match self.other_stealers[idx].steal_batch_and_pop(&self.steal_deque) {
                crossbeam_deque::Steal::Success(fiber) => {
                    self.ready_queue.push_back(fiber);
                    // Drain any batch-stolen fibers from our deque
                    while let Some(f) = self.steal_deque.pop() {
                        self.ready_queue.push_back(f);
                    }
                    return true;
                }
                _ => continue,
            }
        }
        false
    }
}
```

> **Fiber migration:** When a fiber is stolen, its `VMContinuation` (captured frames, stack, handler stack) moves to the new scheduler. Since `Value` uses `Arc<VMObject>` (which is `Send`), the continuation is safe to send across threads. The stealing scheduler resumes the fiber on its own `VM` instance.

### Effect Handling (Multi-Threaded)

```rust
impl Scheduler {
    fn handle_effect(&mut self, pid: u64, signal: EffectSignal) {
        let name = self.vm.shared.interner.resolve(signal.op_name);
        match name {
            // === Layer 1: Fibers + Channels ===

            "fiber-spawn" => {
                // Extract closure from args
                let closure = extract_closure(&signal.args[0]);
                let child_pid = self.vm.shared.process_registry.spawn(self.id);

                // Resume parent FIRST (critical for monitor/link setup)
                self.ready_queue.push_back(Fiber::Suspended {
                    continuation: signal.continuation,
                    resume_value: Value::from_object(Arc::new(VMObject::Pid(child_pid))),
                    pid,
                });

                // Distribute child to least-loaded scheduler (or self)
                let target = self.pick_scheduler_for_spawn();
                if target == self.id {
                    self.ready_queue.push_back(Fiber::New { closure, pid: child_pid });
                } else {
                    self.send_to_scheduler(target, SchedulerMessage::Fiber(
                        Fiber::New { closure, pid: child_pid }
                    ));
                }
            }

            "fiber-yield" => {
                // Re-queue at back
                self.ready_queue.push_back(Fiber::Suspended {
                    continuation: signal.continuation,
                    resume_value: Value::NIL,
                    pid,
                });
            }

            "fiber-self" => {
                self.ready_queue.push_back(Fiber::Suspended {
                    continuation: signal.continuation,
                    resume_value: Value::from_object(Arc::new(VMObject::Pid(pid))),
                    pid,
                });
            }

            "chan-new" => {
                let id = self.vm.shared.channel_registry.create();
                self.ready_queue.push_back(Fiber::Suspended {
                    continuation: signal.continuation,
                    resume_value: Value::from_object(Arc::new(VMObject::Port(id))),
                    pid,
                });
            }

            "chan-send" => {
                let chan_id = extract_channel_id(&signal.args[0]);
                let value = signal.args[1].clone();
                // Channel send may wake a receiver on ANOTHER scheduler
                let wake_target = self.vm.shared.channel_registry.send(chan_id, value);
                if let Some((sched_id, receiver_pid, val)) = wake_target {
                    if sched_id == self.id {
                        self.wake_fiber(receiver_pid, val);
                    } else {
                        self.send_to_scheduler(sched_id, SchedulerMessage::ChannelWake {
                            channel_id: chan_id, value: val,
                        });
                    }
                }
                // Resume sender
                self.ready_queue.push_back(Fiber::Suspended {
                    continuation: signal.continuation,
                    resume_value: Value::NIL,
                    pid,
                });
            }

            "chan-recv" => {
                let chan_id = extract_channel_id(&signal.args[0]);
                match self.vm.shared.channel_registry.try_recv(chan_id, self.id, pid) {
                    Some(value) => {
                        // Data available — resume immediately
                        self.ready_queue.push_back(Fiber::Suspended {
                            continuation: signal.continuation,
                            resume_value: value,
                            pid,
                        });
                    }
                    None => {
                        // Park until data arrives
                        self.parked.insert(pid, ParkedFiber {
                            continuation: signal.continuation,
                            pid,
                        });
                    }
                }
            }

            // === Layer 2: Actors ===

            "actor-send" => {
                let target_pid = extract_pid(&signal.args[0]);
                let message = signal.args[1].clone();

                // Route to the scheduler that owns the target PID
                match self.vm.shared.process_registry.lookup(target_pid) {
                    Some(sched_id) if sched_id == self.id => {
                        self.deliver_actor_message(target_pid, message);
                    }
                    Some(sched_id) => {
                        self.send_to_scheduler(sched_id, SchedulerMessage::ActorMessage {
                            target_pid, message,
                        });
                    }
                    None => { /* dead process — silently drop (Erlang semantics) */ }
                }
                // Resume sender immediately
                self.ready_queue.push_back(Fiber::Suspended {
                    continuation: signal.continuation,
                    resume_value: Value::NIL,
                    pid,
                });
            }

            "actor-receive" | "actor-receive-match" => {
                // Check local mailbox
                if let Some(msg) = self.mailboxes.get_mut(&pid).and_then(|mb| mb.pop_front()) {
                    self.ready_queue.push_back(Fiber::Suspended {
                        continuation: signal.continuation,
                        resume_value: msg,
                        pid,
                    });
                } else {
                    // Park until message arrives
                    self.parked.insert(pid, ParkedFiber {
                        continuation: signal.continuation,
                        pid,
                    });
                }
            }

            "actor-monitor" => { /* register monitor in local + registry */ }
            "actor-link"    => { /* bidirectional link — may cross schedulers */ }
            "actor-unlink"  => { /* remove link */ }
            "actor-trap-exit" => { /* enable exit trapping */ }

            // === Layer 3: IO ===
            "print" | "println" | "read-line" | "read-file" | "write-file" => {
                let result = try_default_handler(&mut self.vm, signal.op_name, &signal.args)
                    .unwrap_or(Value::NIL);
                self.ready_queue.push_back(Fiber::Suspended {
                    continuation: signal.continuation,
                    resume_value: result,
                    pid,
                });
            }

            // === Layer 4: TCP Networking ===
            // All 6 TCP effects implemented in handle_effect():
            //   tcp-connect — async connect, spawns background read task, parks fiber
            //   tcp-listen  — synchronous bind via block_on, spawns background accept loop
            //   tcp-accept  — returns queued connection or parks fiber; nil on closed listener
            //   tcp-send    — async write via write_all, parks fiber
            //   tcp-recv    — returns queued data or parks fiber; nil on EOF/close
            //   tcp-close   — closes socket or listener, wakes parked acceptors
            //
            // Cross-scheduler operations route via TcpOp / TcpOpResult messages.
            // See "TCP Networking (Multi-Threaded)" section for full architecture.

            "http-fetch" => {
                let url = extract_string(&signal.args[0]);
                let sender = self.inbox_sender.clone();
                tokio::spawn(async move {
                    let result = reqwest::get(&url).await;
                    let _ = sender.send(SchedulerMessage::IoComplete {
                        fiber_pid: pid,
                        result: wrap_http_result(result),
                    });
                });
                self.parked.insert(pid, ParkedFiber {
                    continuation: signal.continuation, pid,
                });
            }

            "http-serve"        => { /* bind Hyper server */ }
            "http-accept"       => { /* park until request arrives */ }
            "http-respond"      => { /* send HTTP response */ }
            "http-close-server" => { /* shutdown server */ }

            _ => panic!("unhandled effect: {}", name),
        }
    }

    /// Pick which scheduler should run a newly spawned fiber.
    /// Simple round-robin with bias toward self (cache locality).
    fn pick_scheduler_for_spawn(&self) -> usize {
        // Local bias: keep fiber on same scheduler ~70% of the time
        // to maximize cache locality. Only distribute under load.
        if self.ready_queue.len() < 4 {
            self.id
        } else {
            // Round-robin across schedulers
            self.vm.shared.process_registry.next_pid
                .load(Ordering::Relaxed) as usize % self.num_schedulers()
        }
    }

    fn send_to_scheduler(&self, sched_id: usize, msg: SchedulerMessage) {
        self.scheduler_senders[sched_id].send(msg)
            .expect("scheduler channel disconnected");
    }
}
```

### Runtime Initialization

```rust
/// The top-level runtime — creates N scheduler threads and manages shutdown.
pub struct Runtime {
    shared: Arc<SharedState>,
    scheduler_handles: Vec<std::thread::JoinHandle<()>>,
    scheduler_senders: Vec<crossbeam_channel::Sender<SchedulerMessage>>,
    num_schedulers: usize,
}

impl Runtime {
    /// Create a runtime with N scheduler threads (default: number of CPU cores).
    pub fn new(num_schedulers: Option<usize>) -> Self {
        let n = num_schedulers.unwrap_or_else(num_cpus::get);
        let shared = Arc::new(SharedState::new());

        // Create crossbeam work-stealing deques
        let workers: Vec<_> = (0..n).map(|_| crossbeam_deque::Worker::new_fifo()).collect();
        let stealers: Vec<_> = workers.iter().map(|w| w.stealer()).collect();

        // Create inbox channels
        let channels: Vec<_> = (0..n)
            .map(|_| crossbeam_channel::unbounded::<SchedulerMessage>())
            .collect();
        let senders: Vec<_> = channels.iter().map(|(s, _)| s.clone()).collect();

        let mut handles = Vec::with_capacity(n);

        for (i, (worker, (_, receiver))) in workers.into_iter().zip(channels).enumerate() {
            let shared = shared.clone();
            let other_stealers: Vec<_> = stealers.iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, s)| s.clone())
                .collect();
            let all_senders = senders.clone();

            let handle = std::thread::Builder::new()
                .name(format!("marlang-sched-{}", i))
                .spawn(move || {
                    // Each scheduler thread gets its own Tokio runtime
                    // for driving async IO callbacks.
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap();

                    let mut vm = VM::new(shared.clone());
                    register_builtins(&mut vm);

                    let scheduler = Scheduler {
                        id: i,
                        vm,
                        ready_queue: VecDeque::new(),
                        steal_deque: worker,
                        other_stealers,
                        inbox: receiver,
                        inbox_sender: all_senders[i].clone(),
                        scheduler_senders: all_senders,
                        pending_io: HashMap::new(),
                        active_servers: 0,
                        parked: HashMap::new(),
                        mailboxes: HashMap::new(),
                        monitors: HashMap::new(),
                        links: HashMap::new(),
                        trap_exit: HashSet::new(),
                        result: None,
                    };

                    rt.block_on(scheduler.run());
                })
                .expect("failed to spawn scheduler thread");

            handles.push(handle);
        }

        Runtime {
            shared,
            scheduler_handles: handles,
            scheduler_senders: senders,
            num_schedulers: n,
        }
    }

    /// Run a Marlang program with fibers. The main closure runs on scheduler 0.
    pub fn run_fibers(&self, closure: Arc<Closure>) -> Value {
        let main_pid = self.shared.process_registry.spawn(0);
        self.scheduler_senders[0].send(SchedulerMessage::Fiber(
            Fiber::New { closure, pid: main_pid }
        )).unwrap();

        // Wait for completion (scheduler 0 sends result via a oneshot channel)
        // ...
    }

    /// Shut down all scheduler threads.
    pub fn shutdown(self) {
        for sender in &self.scheduler_senders {
            let _ = sender.send(SchedulerMessage::Shutdown);
        }
        for handle in self.scheduler_handles {
            handle.join().ok();
        }
    }
}
```

### Global Termination Detection

The runtime must detect when all fibers have completed and no IO is pending. This uses a distributed termination protocol:

```rust
impl Runtime {
    /// All schedulers report idle status via an atomic counter.
    /// When all N schedulers are idle AND no pending IO exists, the runtime terminates.
    fn check_global_termination(&self) -> bool {
        self.shared.idle_count.load(Ordering::SeqCst) == self.num_schedulers
            && self.shared.pending_io_count.load(Ordering::SeqCst) == 0
            && self.shared.active_server_count.load(Ordering::SeqCst) == 0
    }
}
```

Each scheduler increments `idle_count` when it has no ready fibers and no local pending IO, and decrements it when it gets new work (from inbox, work stealing, or IO completion).

### TCP Networking (Multi-Threaded)

TCP networking uses Tokio's multi-threaded runtime. All TCP effects are only available in `run_fibers_parallel()` (the multi-threaded scheduler); the single-threaded scheduler rejects them with a clear error message.

#### Architecture

TCP state is **per-scheduler** (no locks on the hot path):

```rust
// Per-scheduler (thread-local, no locking)
struct PerScheduler {
    tcp_sockets: HashMap<u64, TcpSocketState>,
    tcp_listeners: HashMap<u64, TcpListenerState>,
    // ...
}

struct TcpSocketState {
    write_handle: Arc<tokio::sync::Mutex<OwnedWriteHalf>>,
    recv_queue: VecDeque<Vec<u8>>,
    waiting_recv: Option<(FiberId, Value)>, // parked fiber waiting for data
    closed: bool,
}

struct TcpListenerState {
    accept_queue: VecDeque<u64>,              // socket IDs of accepted connections
    waiting_accept: Option<(FiberId, Value)>, // parked fiber waiting for connection
    shutdown_tx: Option<oneshot::Sender<()>>, // stop the background accept loop
}
```

**Ownership tracking** is in `SharedState` (mutex-protected):

```rust
struct SharedState {
    tcp_socket_owners: Mutex<HashMap<u64, usize>>,   // socket_id → scheduler_id
    tcp_listener_owners: Mutex<HashMap<u64, usize>>,  // listener_id → scheduler_id
    next_tcp_id: AtomicU64,
    active_listeners: AtomicUsize,  // prevents premature shutdown
    // ...
}
```

#### Cross-Scheduler Routing

When a fiber on scheduler A operates on a socket owned by scheduler B, the operation is routed via `SchedulerMessage::TcpOp`:

```rust
enum TcpOp {
    Send { socket_id: u64, data: String, fiber_pid: FiberId, reply_scheduler: usize },
    Recv { socket_id: u64, fiber_pid: FiberId, reply_scheduler: usize, continuation: Value },
    Close { socket_id: u64, fiber_pid: FiberId, reply_scheduler: usize },
}
```

The owning scheduler processes the `TcpOp` and sends the result back via `TcpOpResult { fiber_pid, result }` to the requesting scheduler's inbox. The requesting fiber is parked in `io_parked` until the result arrives.

#### Effect Handlers

| Effect | Args | Returns | Behavior |
|--------|------|---------|----------|
| `tcp-connect` | `host: String`, `port: Int` | `(Ok Port)` or `(Err String)` | Async connect via tokio, spawns background read task, parks fiber until connected |
| `tcp-listen` | `port: Int` | `(Ok Port)` or `(Err String)` | Synchronous bind via `block_on`, spawns background accept loop with `tokio::select!` for shutdown |
| `tcp-accept` | `listener: Port` | `Port(socket_id)` or `nil` | Returns queued connection or parks fiber; returns `nil` if listener is closed |
| `tcp-send` | `socket: Port`, `data: String` | `(Ok nil)` or `(Err String)` | Async write via `write_all`, parks fiber until complete |
| `tcp-recv` | `socket: Port` | `String` or `nil` | Returns queued data or parks fiber; returns `nil` on EOF/close |
| `tcp-close` | `socket_or_listener: Port` | `nil` | Closes socket or listener; wakes any parked acceptor with `nil` |
| `throw` | `msg: String` | (never returns) | Default handler crashes the fiber; user `handle` blocks can catch it |

#### Error Handling

Failable TCP operations (`tcp-connect`, `tcp-listen`, `tcp-send`) return `Result` ADT values (`(Ok value)` or `(Err msg)`) instead of crashing the fiber. This allows two patterns:

1. **Throwing pattern:** The `net.mar` stdlib wrappers (`connect`, `listen`, `send`) unwrap the Result and `(perform throw msg)` on error. The default `throw` handler crashes the fiber. Users get clean crash semantics without needing to match on Results.

2. **Try pattern:** The `net.mar` stdlib `try-` wrappers (`try-connect`, `try-listen`, `try-send`) return the Result directly. Users pattern-match on `(Ok v)` / `(Err msg)` to handle errors gracefully.

The `throw` effect is declared in the prelude as `(effect Exception (throw (-> String Unit)))`. Both schedulers handle it as a default "crash the fiber" operation. User code can install `(handle ... ((throw msg k) -> ...))` blocks to catch exceptions, though this pattern does not work reliably across scheduler effect boundaries (e.g., inside the same handle block as TCP effects that park the fiber).

#### Connection Lifecycle

1. **Connect:** `tcp-connect` spawns an async task that calls `TcpStream::connect`. On success, returns `(Ok Port)`. On failure (connection refused, DNS error), returns `(Err msg)`. On success, the stream is split into `OwnedReadHalf` + `OwnedWriteHalf`. A background read task continuously reads from the socket and sends `TcpData { socket_id, data }` or `TcpClosed { socket_id }` messages to the owning scheduler. The write handle is stored in `TcpSocketState`. A `TcpAccepted` message (with sentinel `listener_id = u64::MAX`) registers the socket state, followed by `IoComplete` to wake the fiber.

2. **Listen:** `tcp-listen` binds synchronously (via `block_on`). On success, returns `(Ok Port)`. On failure (port in use, permission denied), returns `(Err msg)`. A background accept loop uses `tokio::select!` to wait on either a new connection or a shutdown signal. Accepted connections get their own background read task and are queued via `TcpAccepted` messages. Socket IDs for accepted connections are pre-allocated from a standalone `Arc<AtomicU64>` (batch of 1000) to avoid needing `SharedState` inside the `'static` tokio task.

3. **Accept:** `tcp-accept` checks the listener's `accept_queue`. If a connection is waiting, it returns immediately. If the listener is closed (no `shutdown_tx`), it returns `nil`. Otherwise, the fiber is parked until a `TcpAccepted` message arrives.

4. **Close:** `tcp-close` handles both sockets and listeners. For listeners, it sends the shutdown signal, wakes any parked acceptor with `nil`, decrements `active_listeners`, and removes ownership. For sockets, it drops the write handle and removes state.

#### Termination Detection

`check_termination()` prevents shutdown while `active_listeners > 0`, ensuring server programs stay alive while listening for connections.

#### SchedulerMessage Variants for TCP

```rust
enum SchedulerMessage {
    TcpData { socket_id: u64, data: Vec<u8> },       // background read task → owner
    TcpClosed { socket_id: u64 },                      // EOF or error on read
    TcpAccepted { listener_id: u64, socket_id: u64,   // accept loop → owner
                  write_handle: Arc<Mutex<OwnedWriteHalf>> },
    TcpOp(TcpOp),                                      // cross-scheduler operation request
    TcpOpResult { fiber_pid: FiberId,                  // cross-scheduler operation result
                  result: Result<Value, String> },
    // ...
}
```

> **Design rationale:** TCP state is per-scheduler to avoid locks on the hot path. Background read tasks and accept loops run on Tokio's thread pool, pushing data into the scheduler's inbox via crossbeam channels. Cross-scheduler operations use the existing `SchedulerMessage` routing infrastructure. Socket/listener handles reuse `VMObject::Port(u64)` — ID spaces don't collide because channels use `next_channel_id` and TCP uses `next_tcp_id`.

### Value Thread Safety

`Value` (NaN-boxed `u64`) and `VMObject` (behind `Arc`) are `Send`:

- **Numbers, bools, nil** — immediate values in the `u64`, trivially `Send`.
- **Objects** (`Arc<VMObject>`) — `Arc` is `Send + Sync` when `VMObject: Send + Sync`.
- **Closures** — `Arc<FunctionPrototype>` (immutable bytecode) + `Box<[Value]>` (captured upvalues, themselves `Send`).
- **Continuations** — `Vec<CallFrame>` + `Vec<Value>` — all `Send`.

Values can safely cross thread boundaries in actor messages, channel sends, and fiber migrations.

### Single-Threaded Fallback

For WASM and testing, the runtime supports `num_schedulers = 1`, which degrades to a single-threaded scheduler (equivalent to the TypeScript model). The crossbeam channels and work stealing deques still work but never actually cross threads, so the overhead is one extra indirection per message.

---

## 9. FFI ✅

The FFI system provides **dlopen FFI** for calling C libraries from Marlang code, matching the TypeScript `ffi-open`/`ffi-call`/`ffi-close` builtins. Uses `libloading` for cross-platform shared library loading and `unsafe` transmute-based dispatch for known C function signatures.

### 9.1 dlopen FFI (`libloading` + transmute dispatch)

Lives in `crates/vm/src/ffi.rs`, gated behind `#[cfg(feature = "ffi")]` (enabled by default). Uses `libloading` for dlopen/dlsym and transmute-based dispatch for known C signatures — no `libffi` dependency needed since Marlang FFI only calls functions with simple numeric signatures (f64→f64, f64,f64→f64, etc.):

```rust
/// Opened shared library with pre-parsed function signatures.
pub struct FfiLibrary {
    pub lib: libloading::Library,
    pub path: String,
    pub signatures: HashMap<String, FfiSignature>,
}

pub struct FfiSignature {
    pub args: Vec<FfiType>,
    pub ret: FfiType,
}

#[derive(Debug, Clone)]
pub enum FfiType {
    Void, U8, I8, U16, I16, U32, I32, U64, I64,
    F32, F64, Pointer, CString, Bool,
}

/// Struct definition for FFI struct types.
pub struct FfiStructDef {
    pub name: String,
    pub fields: Vec<FfiStructField>,
    pub size: usize,
}

pub struct FfiStructField {
    pub name: String,
    pub ffi_type: FfiType,
    pub offset: usize,
    pub size: usize,
}

/// A live FFI struct instance — raw byte buffer + definition.
pub struct FfiStruct {
    pub def: Arc<FfiStructDef>,
    pub buffer: Vec<u8>,
}
```

#### VMObject Variants

All FFI variants are gated behind `#[cfg(feature = "ffi")]`:

```rust
pub enum VMObject {
    // ... existing variants ...

    // FFI (behind feature flag)
    #[cfg(feature = "ffi")]
    FfiLibrary(Arc<ffi::FfiLibrary>),    // Arc because libloading::Library isn't Clone
    #[cfg(feature = "ffi")]
    FfiPointer(u64),
    #[cfg(feature = "ffi")]
    FfiStructDef(Arc<ffi::FfiStructDef>),
    #[cfg(feature = "ffi")]
    FfiStruct(Box<ffi::FfiStruct>),
}
```

#### Builtin Functions

7 builtins registered in `builtins.rs` behind `#[cfg(feature = "ffi")]`:

```rust
/// Global struct definition registry (mirrors TS vmStructRegistry).
static STRUCT_REGISTRY: LazyLock<Mutex<HashMap<String, Arc<FfiStructDef>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn register_ffi_builtins(vm: &mut VM) {
    // ffi-open, ffi-call, ffi-close, ffi-struct, struct-new, struct-get, struct-set
}
```

#### Transmute-Based Dispatch

Instead of using `libffi` for arbitrary calling conventions, `ffi_call` pattern-matches on the signature shape and uses `transmute` for known patterns:

```rust
pub fn ffi_call(lib: &FfiLibrary, fn_name: &str, args: &[Value]) -> Result<Value, String> {
    let sig = lib.signatures.get(fn_name).ok_or(...)?;
    unsafe {
        let sym: libloading::Symbol<*const ()> = lib.lib.get(fn_name.as_bytes())?;
        let ptr = *sym;
        match (&sig.args[..], &sig.ret) {
            ([FfiType::F64], FfiType::F64) => {
                let f: fn(f64) -> f64 = std::mem::transmute(ptr);
                Ok(Value::number(f(args[0].as_number().unwrap())))
            }
            ([FfiType::F64, FfiType::F64], FfiType::F64) => {
                let f: fn(f64, f64) -> f64 = std::mem::transmute(ptr);
                Ok(Value::number(f(args[0].as_number().unwrap(), args[1].as_number().unwrap())))
            }
            // ... other patterns
        }
    }
}
```

#### Library Name Resolution

Matches the TypeScript behavior — auto-adds platform-specific prefix/suffix:

```rust
fn resolve_library_name(name: &str) -> String {
    if name.contains('/') || name.contains('.') {
        return name.to_string(); // Already a path
    }
    #[cfg(target_os = "macos")]
    { format!("lib{}.dylib", name) }
    #[cfg(target_os = "linux")]
    { format!("lib{}.so", name) }
    #[cfg(target_os = "windows")]
    { format!("{}.dll", name) }
}
```

### 9.2 Rust Function Registration (Embedding API)

> **Deferred to a future phase.** The `FromValue`/`IntoValue` traits and `register_fn` API are not yet implemented. For now, custom builtins can be registered directly by inserting `VMObject::Builtin` values into `vm.globals`.

---

## 10. Module System ✅

The module system lives in `crates/vm/src/modules.rs` (not a separate crate, since it needs tight integration with the VM for expression-at-a-time compilation and execution).

### Architecture: Expression-at-a-Time Processing

The key design challenge: imports must interleave compilation and execution (a module must be *executed* to populate its globals before the importing code can reference them). The solution is `run_program()`, which walks top-level expressions one-by-one:

```rust
pub struct ModuleLoader {
    /// Module cache: canonical path → ModuleState
    cache: HashMap<PathBuf, ModuleState>,
    /// Virtual modules for testing (path → source)
    virtual_modules: HashMap<PathBuf, String>,
    /// Resolved stdlib directory
    stdlib_dir: Option<PathBuf>,
}

enum ModuleState {
    Loading,                                    // Circular dependency detection
    Loaded { globals: HashMap<String, Value> }, // Cached exports
}

enum ImportForm {
    All,                        // (import "./math.mar")
    Selective(Vec<String>),     // (import "./math.mar" (square double))
    Qualified(String),          // (import "./math.mar" as math)
}
```

### Key Functions

```rust
/// Expression-at-a-time top-level executor.
/// For each expr: if import → handle_import(), else → compile + interpret.
pub fn run_program(
    vm: &mut VM,
    source: &str,
    file_path: Option<&str>,
    loader: &mut ModuleLoader,
    interner: &mut Interner,
) -> Result<Value, VMError>

/// Compile a single expression with external constructor/effect knowledge.
pub fn compile_expr_with_context(
    expr: &Expr,
    interner: &Interner,
    known_constructors: &HashSet<Symbol>,
    known_effect_ops: &HashMap<Symbol, Symbol>,
) -> Result<CompileResult, CompileError>
```

### Dirty Globals Tracking

Module export detection uses a `dirty_globals: Option<HashSet<String>>` field on the VM. When enabled, the `SetGlobal` opcode records which globals are written. After running a module, only written globals are exported. This correctly handles modules that redefine constructors with structurally identical values (e.g., `result.mar` redefining `Ok`/`Err`).

### Qualified Import: Unqualified Name Installation

When a module is imported with `(import "mod.mar" as m)`, `merge_exports` installs **both** prefixed names (`m/foo`) and unqualified names (`foo`) into the parent's globals table. This is necessary because closures exported from the module reference their sibling functions by unprefixed names via `PUSH_GLOBAL` (they were compiled in the module's own scope). Without unqualified names, cross-function calls within the module would fail when the closure is invoked from the parent scope.

The unqualified name is only installed if it doesn't already exist (`if !vm.globals.contains_key(name)`), which means the first-imported module "wins" for any name collision. This is a known limitation — see Phase 8 Known Issues for details. The proper long-term fix is module-scoped globals (closures capture their module's globals table).

### Path Resolution

- `"std:name"` → looks up `stdlib_dir/name.mar`
- `"./relative"` / `"../relative"` → relative to importing file, auto-appends `.mar`
- Bare paths → error

### Stdlib Resolution

The stdlib directory is auto-detected at `ModuleLoader::new()`:
1. `MARLANG_HOME` env var + `/stdlib`
2. Source tree `stdlib/` (relative to `CARGO_MANIFEST_DIR` — for dev)
3. Falls back to `None` (stdlib imports error)

Virtual modules (`register_virtual()`) take precedence over filesystem — used for testing.

---

## 11. LSP

The TypeScript LSP (`lsp/server.ts`) implements a hand-rolled JSON-RPC reader/writer over stdin/stdout. In Rust, use the `tower-lsp` crate for a production-grade implementation:

### Architecture

```rust
use tower_lsp::{LspService, Server, Client, LanguageServer, jsonrpc};
use tower_lsp::lsp_types::*;

pub struct MarlangLsp {
    client: Client,
    documents: DashMap<Url, DocumentState>,
}

struct DocumentState {
    version: i32,
    content: String,
    analysis: Option<AnalysisResult>,
}
```

### Server Implementation

```rust
#[tower_lsp::async_trait]
impl LanguageServer for MarlangLsp {
    async fn initialize(&self, _params: InitializeParams) -> jsonrpc::Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec!["(".into(), " ".into()]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "marlang-lsp".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
        })
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let text = params.text_document.text.clone();
        let version = params.text_document.version;
        self.analyze_and_publish(&uri, &text, version).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        if let Some(change) = params.content_changes.last() {
            let uri = params.text_document.uri.clone();
            let version = params.text_document.version;
            self.analyze_and_publish(&uri, &change.text, version).await;
        }
    }

    async fn hover(&self, params: HoverParams) -> jsonrpc::Result<Option<Hover>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        // Look up symbol at position, return type info
        Ok(self.get_hover(uri, pos))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> jsonrpc::Result<Option<GotoDefinitionResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        Ok(self.get_definition(uri, pos))
    }

    async fn completion(
        &self,
        params: CompletionParams,
    ) -> jsonrpc::Result<Option<CompletionResponse>> {
        let uri = &params.text_document_position.text_document.uri;
        let pos = params.text_document_position.position;
        Ok(Some(self.get_completion(uri, pos)))
    }

    async fn shutdown(&self) -> jsonrpc::Result<()> {
        Ok(())
    }
}
```

### Analyzer

The analyzer runs the full pipeline (lex → parse → typecheck) and caches results per document. It mirrors `lsp/analyzer.ts`:

```rust
pub struct AnalysisResult {
    pub diagnostics: Vec<Diagnostic>,
    pub symbols: HashMap<Symbol, SymbolInfo>,
    pub nodes_by_location: Vec<(Span, Expr, Option<Type>)>,
    pub type_env: Option<TypeEnv>,
}

pub fn analyze(source: &str, uri: &Url, interner: &mut Interner) -> AnalysisResult {
    // 1. Lex
    // 2. Parse
    // 3. Type check
    // 4. Collect symbols and node locations
    // Return diagnostics + symbol table
    /* ... */
}
```

### Capabilities Matrix

Matching the TypeScript LSP:

| Feature | Status |
|---------|--------|
| Diagnostics (type errors, parse errors) | Port from TS |
| Hover (type information) | Port from TS |
| Go to definition | Port from TS |
| Completion (symbols, keywords) | Port from TS |
| Document sync (full) | Port from TS |
| Formatting | Future |
| Rename | Future |
| References | Future |

---

## 12. CLI ✅

Use `clap` derive API for argument parsing and `rustyline` for the REPL:

```rust
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "mar", about = "The Marlang programming language")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run a Marlang file
    Run {
        file: PathBuf,
        /// Use the bytecode VM (default — tree-walker removed in Rust port)
        #[arg(long)]
        no_typecheck: bool,
    },
    /// Start interactive REPL
    Repl,
    /// Type check a file without running
    Typecheck {
        file: PathBuf,
        /// Print inferred types
        #[arg(long)]
        verbose: bool,
    },
    /// Disassemble bytecode
    Disasm {
        file: PathBuf,
    },
    /// Start the Language Server Protocol server
    Lsp,
}
```

> **Note:** The Rust port drops the tree-walker interpreter. All execution uses the bytecode VM. The `--vm` flag becomes unnecessary and is removed. This simplifies the codebase significantly — the tree-walker was ~1200 LOC that duplicated VM functionality.

### REPL

```rust
fn start_repl(vm: &mut VM) -> Result<(), Box<dyn Error>> {
    let mut rl = rustyline::DefaultEditor::new()?;
    println!("Marlang REPL - Type .exit or (quit) to exit");

    loop {
        match rl.readline("marlang> ") {
            Ok(line) => {
                if line.trim() == ".exit" || line.trim() == "(quit)" {
                    println!("Goodbye!");
                    break;
                }
                rl.add_history_entry(&line)?;
                match evaluate(vm, &line) {
                    Ok(value) => println!("  => {}", format_value(value, &vm.interner)),
                    Err(e) => eprintln!("Error: {}", e),
                }
            }
            Err(rustyline::error::ReadlineError::Interrupted |
                rustyline::error::ReadlineError::Eof) => break,
            Err(e) => { eprintln!("Error: {}", e); break; }
        }
    }
    Ok(())
}
```

### Runtime Initialization

The CLI initially uses `vm.interpret()` directly for non-concurrent execution. Once the scheduler lands (Phase 7), `run` will be updated to use the multi-scheduler `Runtime` for programs that use concurrency effects:

```rust
fn main() {
    let cli = Cli::parse();

    match cli.command {
        Command::Run { file, no_typecheck } => {
            // Phase 6: direct VM execution (no scheduler)
            let mut interner = Interner::new();
            let chunk = compile_file(&file, &mut interner, no_typecheck);
            let mut vm = VM::new_with_interner(interner);
            register_builtins(&mut vm);
            vm.interpret(&chunk).expect("runtime error");

            // Phase 7 upgrade: detect concurrency effects and use Runtime
            // let runtime = Runtime::new(None);
            // let result = runtime.run_fibers(closure);
            // runtime.shutdown();
        }
        Command::Repl => {
            let mut interner = Interner::new();
            let mut vm = VM::new_with_interner(interner);
            register_builtins(&mut vm);
            start_repl(&mut vm).unwrap();
        }
        Command::Typecheck { file, verbose } => typecheck_file(&file, verbose),
        Command::Disasm { file } => disasm_file(&file),
        Command::Lsp => {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(start_lsp());
        }
    }
}
```

---

## 13. Testing Strategy

### Porting the Test Suite

The TypeScript codebase has **467 tests** across 13 test files:

| Test File | Tests | Rust Crate | Strategy |
|-----------|-------|------------|----------|
| `lexer.test.ts` | 37 | `core` | Unit tests in `lexer.rs` — 37 ported ✅ |
| `parser.test.ts` | 34 | `core` | Unit tests in `parser.rs` — 34 ported ✅ |
| `interpreter.test.ts` | 60 | `vm` | Integration tests (VM replaces tree-walker) |
| `type-system.test.ts` | 63 | `types` | Unit tests in `tests.rs` — 54 ported (type-checking only; 9 runtime-only tests skipped) ✅ |
| N/A (compiler+value) | N/A | `vm` | 65 unit tests: compiler (57) + NaN-boxed Value (8) ✅ |
| `effects.test.ts` | 53 | `vm` | Integration tests — ported in Phase 4 ✅ |
| `vm.test.ts` | 67 | `vm` | Integration tests — ported in Phase 4 ✅ |
| `tco.test.ts` | 7 | `vm` | Integration tests — ported in Phase 4 ✅ |
| `modules.test.ts` | 18 | `vm` | Integration tests — 17 ported in Phase 5 ✅ |
| `stdlib.test.ts` | 69 | `vm` | Integration tests — 9 key tests ported in Phase 5 (Option, Result, IO) ✅ |
| N/A (FFI) | N/A | `vm` | 6 FFI tests ported in Phase 5 ✅ |
| `concurrent.test.ts` | 25 | `vm` | 24 ported as synchronous tests ✅ |
| `editor-ops.test.ts` | 25 | `lsp` | Integration tests |
| `lsp.test.ts` | 0 | `lsp` | Placeholder (future tests) |
| `net.test.ts` | 4 | `vm` | Async integration tests |
| `http.test.ts` | 5 | `vm` | Async integration tests |

### Test Helper

A shared `eval()` helper that compiles and runs Marlang source, returning the result as a Rust value:

```rust
#[cfg(test)]
pub mod test_helpers {
    use super::*;

    /// Compile and run Marlang source, return the result.
    pub fn eval(source: &str) -> Value {
        let mut interner = Interner::new();
        let tokens = lex(source, &mut interner).expect("lex error");
        let ast = parse(&tokens).expect("parse error");
        let chunk = compile_program(&ast, &mut interner);
        let mut vm = VM::new_with_interner(interner);
        register_builtins(&mut vm);
        vm.interpret(&chunk).expect("runtime error")
    }

    /// Eval and assert the result is a number.
    pub fn eval_number(source: &str) -> f64 {
        eval(source).as_f64().expect("expected number")
    }

    /// Eval and assert the result is a string.
    pub fn eval_string(source: &str) -> String {
        let val = eval(source);
        val.as_object()
            .and_then(|o| match o.as_ref() {
                VMObject::String(s) => Some(s.to_string()),
                _ => None,
            })
            .expect("expected string")
    }

    /// Eval and expect a type error.
    pub fn eval_type_error(source: &str) -> TypeError {
        let mut interner = Interner::new();
        let tokens = lex(source, &mut interner).unwrap();
        let ast = parse(&tokens).unwrap();
        let env = create_builtin_env(&interner);
        infer_program(&env, &ast, None).expect_err("expected type error")
    }
}
```

### Example Test

```rust
#[cfg(test)]
mod tests {
    use crate::test_helpers::*;

    #[test]
    fn arithmetic() {
        assert_eq!(eval_number("(+ 1 2)"), 3.0);
        assert_eq!(eval_number("(* 3 4)"), 12.0);
        assert_eq!(eval_number("(- 10 3)"), 7.0);
        assert_eq!(eval_number("(/ 15 5)"), 3.0);
    }

    #[test]
    fn fibonacci() {
        let source = r#"
            (define fib (lambda (n)
                (if (<= n 1) n
                    (+ (fib (- n 1)) (fib (- n 2))))))
            (fib 10)
        "#;
        assert_eq!(eval_number(source), 55.0);
    }

    #[test]
    fn pattern_matching() {
        let source = r#"
            (data Shape
                (Circle Float)
                (Rect Float Float))
            (define area (lambda (s)
                (match s
                    ((Circle r) (* 3.14159 (* r r)))
                    ((Rect w h) (* w h)))))
            (area (Circle 5.0))
        "#;
        assert!((eval_number(source) - 78.53975).abs() < 0.001);
    }

    #[tokio::test]
    async fn fiber_spawn_and_channel() {
        let source = r#"
            (define main (lambda ()
                (let ((ch (perform chan-new)))
                    (begin
                        (perform fiber-spawn (lambda ()
                            (perform chan-send ch 42)))
                        (perform chan-recv ch)))))
            (run-fibers main)
        "#;
        assert_eq!(eval_async_number(source).await, 42.0);
    }
}
```

### Benchmarks

Port the existing `bench/` infrastructure using `criterion`:

```rust
use criterion::{criterion_group, criterion_main, Criterion};

fn bench_fib40(c: &mut Criterion) {
    let source = include_str!("../../bench/fib.mar");
    c.bench_function("fib(40)", |b| {
        b.iter(|| eval(source))
    });
}

criterion_group!(benches, bench_fib40);
criterion_main!(benches);
```

---

## 14. Migration Milestones

### Phase 1: Core ✅

**Goal:** Lex and parse Marlang source into a Rust AST.

- [x] Set up Cargo workspace with `core` crate
- [x] Implement `Span`, `LineMap`, `Interner`
- [x] Implement `Token`, `Expr`, `LispValue` types
- [x] Port lexer (39 tests)
- [x] Port parser (34 tests)
- [x] `list_to_array()` and other AST utilities

**Deliverable:** `cargo test -p marlang-core` passes all 75 tests.

**Implementation notes:**
- All code lives under `port/` directory (not at workspace root)
- Workspace: `port/Cargo.toml`, crate: `port/crates/core/`
- Edition 2024 (Rust 1.85+)
- Lexer is cursor-based (`&[u8]` with `pos`), not functional-style like the TS version — cleaner and faster
- TS lexer emits `spc` tokens for whitespace/comments then filters them; Rust lexer skips them entirely in `skip_whitespace_and_comments()` — simpler, fewer allocations
- TS lexer tracks line/column eagerly on every character; Rust uses byte-offset `Span` and resolves line/column lazily via `LineMap` — 8 bytes per span vs 48 bytes in TS
- Negative numbers (`-7`) are handled in `lex_identifier()`: when `-` is followed by a digit, it lexes as a negative number literal. Inside expressions like `(- 3)`, the `-` is a standalone identifier (correct behavior matching TS)
- `Interner` uses `RwLock<HashMap>` + leaked `&'static str` — thread-safe for future multi-scheduler use
- `LispValue::String` uses `Arc<str>` (not `String`) for cheap cloning
- 75 tests total: 2 intern + 2 span + 37 lexer + 34 parser (exceeds spec's 71 estimate due to extra edge case coverage)

### Phase 2: Type System ✅

**Goal:** Full HM type inference with typeclasses and exhaustiveness.

- [x] Port type AST (`Type`, `TypeScheme`, `Constraint`, `Pattern`, `EffectRow`)
- [x] Port `TypeEnv`, `Substitution`, `apply_subst`, `compose_subst`
- [x] Port Robinson's unification
- [x] Port constraint generation and inference (`infer_expr`, `infer_program`)
- [x] Port built-in types and typeclasses (`Num`, `Eq`, `Ord`, `Show`)
- [x] Port exhaustiveness checking
- [x] Port error types and formatting
- [x] Port type checker entry point

**Deliverable:** `cargo test -p marlang-types` passes 54 tests (all type-checking tests; 9 runtime-only tests from TS suite skipped since they test the tree-walker interpreter, not the type system).

**Implementation notes:**
- Crate: `port/crates/types/` = `marlang-types`, depends on `marlang-core`
- 8 source files: `ast.rs`, `env.rs`, `unify.rs`, `infer.rs`, `builtins.rs`, `checker.rs`, `errors.rs`, `lib.rs` + `tests.rs`
- **String-based type variables** — The spec originally proposed `Symbol` for type var names, class names, effect names, etc. The implementation uses `String` instead. Type variables (`t0`, `t1`, ...) and effect row variables (`ε0`, `ε1`, ...) are generated fresh and compared by value, never interned. This keeps the type system decoupled from the interner, which is only needed in `infer.rs` to resolve AST symbols during dispatch.
- **Clone-based environments** — `TypeEnv` uses `parent: Option<Box<TypeEnv>>` with clone-on-extend, matching the TS immutable Map pattern. Simple and correct for type-checking workloads.
- **`Result<T, TypeError>` everywhere** — Direct Rust `Result` instead of TS's custom `TypeResult<T>` monad. Enables `?` operator throughout.
- **Thread-local fresh var counters** — `thread_local!` with `Cell<u32>` for `fresh_type_var()` and `fresh_effect_row_var()`, matching TS's global counter pattern but thread-safe.
- **TypeEnv fields** — The spec proposed `type_constructors`, `effect_ops` as TypeEnv fields. The implementation instead mirrors the TS structure more closely: `data_types`, `constructors`, `classes`, `instances`, `aliases`, `effects` (all `HashMap<String, _>`) plus `parent: Option<Box<TypeEnv>>`.
- **TypeClass methods** — Stored as `Vec<(String, Type)>` instead of `HashMap` for simplicity.
- **Import processing** — Stubbed as no-op (full module loading deferred to Phase 5). The 54 type system tests don't test imports.
- **`mergeEnv()` deferred** — The TS `mergeEnv()` function (used for module imports with qualified/selective modes) is not ported yet since import processing is stubbed. Will be needed in Phase 5.
- **54 tests**: basic types (4), arithmetic (3), type annotations (5), functions (2), if expressions (3), let expressions (3), ADTs (2), typeclasses (2), recursive mu-types (4), exhaustiveness checking (7), Show typeclass (7), variadic arithmetic (9), integration (3)

### Phase 3: Bytecode Compiler ✅

**Goal:** Compile Marlang AST to bytecode.

- [x] Port opcodes, `Instruction`, `Chunk`, `FunctionPrototype`
- [x] Implement NaN-boxed `Value`
- [x] Port `VMObject` enum
- [x] Port compiler: expressions, special forms, closures, pattern matching
- [x] Port binary operator optimization
- [x] Port data constructor compilation
- [x] Port handler descriptor compilation
- [x] Port disassembler

**Deliverable:** `cargo test -p marlang-vm` passes 65 tests (57 compiler + 8 value). Compiler produces correct bytecode for all language features. Disassembler output matches TypeScript.

**Implementation notes:**
- Crate: `port/crates/vm/` = `marlang-vm`, depends only on `marlang-core` (not `marlang-types` — type annotations are handled syntactically)
- 8 source files: `bytecode.rs`, `value.rs`, `object.rs`, `compiler.rs`, `debug.rs`, `errors.rs`, `lib.rs` + `tests.rs`
- **ParentScope snapshot pattern** — The TS compiler uses `enclosing: CompilerContext | null` (mutable reference to parent). Rust's borrow checker prevents `&mut` chains. Solution: when creating a child compiler (for lambda/handle), clone parent's `locals` + `upvalues` into a `ParentScope` struct. Child walks this chain for `resolve_upvalue`. Cost: one `Vec<Local>` clone per nested function — negligible since scopes are small.
- **KnownSymbols pre-interning** — All 26 special form names (`define`, `lambda`, `if`, `let`, `begin`, `cond`, `match`, `data`, `effect`, `perform`, `handle`, `resume`, `import`, `class`, `instance`, `type`, `else`, `->`, `return`, `:`, `_`, `#t`, `#f`, `nil`, `negate`, `-`) are interned once at compiler creation. Dispatch uses `Symbol` equality (u32 comparison) instead of string comparison — faster than the TS `switch` on strings.
- **Separate ConstantValue type** — The spec proposed pre-converting constants to `Value` at compile time (`constants: Vec<Value>` in `Chunk`). The implementation keeps a separate `ConstantValue` enum for compile-time constants and provides `preconvert_constants()` for Phase 4 conversion. This avoids NaN-boxed pointer management during compilation and makes the constant pool serializable.
- **`String` not `Symbol` in bytecode types** — `FunctionPrototype.name`, `ConstantValue::String`, ADT tags in `VMObject` use `String` (not `Symbol`) since these are compile-time data that may outlive the interner or need independent ownership. Runtime interning of globals happens at VM execution time (Phase 4).
- **`BuiltinFn` simplified** — Typed as `fn(&[Value]) -> Value` for Phase 3 (no `&mut VM` parameter). Phase 4 will extend this when implementing the execution engine.
- **NaN-boxing layout** — Uses `QNAN = 0x7FFC_0000_0000_0000` (quiet NaN prefix), tag bits 1/2/3 for nil/true/false, sign bit + 48-bit pointer for objects. Manual `Clone`/`Drop` for Arc refcounting. Differs from spec's `0x7FF8` prefix — the extra `0x4` bit ensures all tagged values are quiet NaN (avoids floating-point hardware traps).
- **65 tests**: literals (7), arithmetic (6), comparisons (5), control flow (5), variables (6), functions (5), ADTs+patterns (8), effects (5), type forms (2), disassembler (3), import error (1), compile API (3), value tests (8)

### Phase 4: VM Execution ✅

**Goal:** Execute bytecode with effects and continuations.

- [x] Implement `VM` struct with stack, frames, globals, handler stack
- [x] Implement `run()` main loop with `VMSignal` enum
- [x] Port all opcode implementations
- [x] Port `call_closure()` and `resume_continuation()`
- [x] Port `run_with_handler()` with deep handler semantics
- [x] Port default IO handlers
- [x] Port built-in functions (arithmetic, comparison, list, show, ADTs)
- [x] Port `interpret()` entry point

**Deliverable:** `cargo test -p marlang-vm` passes 191 tests (126 execution + 65 compiler/value).

**Implementation notes:**
- 2 new source files: `vm.rs` (~1500 lines), `builtins.rs` (~240 lines); modified `errors.rs`, `object.rs`, `value.rs`, `lib.rs`, `tests.rs`
- **`VMSignal` enum instead of `RunResult`** — The spec proposed `RunResult { Ok, Effect }`. The implementation uses `Result<Value, VMSignal>` where `VMSignal` is `Error(VMError) | Effect(EffectSignal)`. This lets the `?` operator propagate both errors and effects through the call chain naturally.
- **Local frame copy pattern** — The `run()` loop clones the current frame into a local variable, works on the copy, and calls `save_frame()` before any operation that might read `vm.frames` (calls, errors). This avoids borrow-checker conflicts between frame reads and mutable VM access. Cost: one clone per frame switch — acceptable since frames are small.
- **Handler identity via `u64` IDs** — TypeScript uses reference equality (`signal.targetHandler === handlerFrame`) to match effects to handlers. Rust uses a global `AtomicU64` counter to assign unique IDs to each `VMHandlerFrame`. `EffectSignal.target_handler_id` matches against `VMHandlerFrame.id`.
- **One-shot continuation enforcement** — The spec proposed `cont.resumed = true` on the `VMContinuation` struct, but this doesn't work with Arc-backed values (aliases share the same data via Arc, but the `resumed` flag is only set on the clone). Solution: each `VMContinuation` gets a unique `id: u64`, and the VM tracks resumed IDs in `resumed_continuations: HashSet<u64>`. This correctly prevents aliased continuations from being resumed multiple times.
- **Stack slot cleanup on unwind** — `VM::clear_stack_slots(new_top)` replaces slots `[new_top..stack_top)` with `Value::nil()` before lowering `stack_top`, releasing Arc references that would otherwise leak.
- **Manual JSON parsing** — Handler descriptors (`\0handler:{...}`) are parsed with a hand-rolled JSON parser (~90 lines). No `serde_json` dependency.
- **Pre-convert constants per frame** — `preconvert_constants_recursive()` converts `ConstantValue` to `Value` on each frame switch. A future optimization could cache these per-chunk.
- **`captured_output: Option<String>`** — Test IO capture: when `Some`, `print`/`println` write to the buffer instead of stdout.
- **126 execution tests**: literals, arithmetic, string concat, comparison, control flow, variables, functions, closures, TCO (100k iterations), ADTs, pattern matching, show, not, string ops, effects (handle/perform/resume, stateful counter, abort, state effect, one-shot enforcement), default IO, error handling, type forms, complex programs (map, fold, mutual recursion, higher-order, MkPair, Ok/Err)

### Phase 5: Modules + FFI ✅

**Goal:** Import resolution with filesystem-based stdlib, and dlopen FFI.

#### 5a: Module System

- [x] Implement `ModuleLoader` with path resolution (`std:`, `./`, `../`)
- [x] Implement stdlib directory auto-detection (MARLANG_HOME → source tree)
- [x] Implement module caching with circular dependency detection
- [x] Implement virtual modules (for testing)
- [x] Implement `run_program()` — expression-at-a-time top-level processing
- [x] Implement `compile_expr_with_context()` for single-expression compilation
- [x] Implement three import forms: all, selective, qualified
- [x] Implement dirty globals tracking for accurate module export detection
- [x] Create test fixtures (6 `.mar` files)

#### 5b: FFI

- [x] Add `libloading` dependency (optional, behind `ffi` feature)
- [x] Implement `FfiLibrary`, `FfiSignature`, `FfiType`, `FfiStructDef`, `FfiStruct` types
- [x] Add 4 FFI `VMObject` variants (behind `#[cfg(feature = "ffi")]`)
- [x] Implement `ffi-open` builtin (library name resolution, signature parsing, dlopen)
- [x] Implement `ffi-call` builtin (transmute-based dispatch for known signatures)
- [x] Implement `ffi-close` builtin
- [x] Implement `ffi-struct`, `struct-new`, `struct-get`, `struct-set` builtins

**Deliverable:** `cargo test -p marlang-vm` passes 223 tests (191 existing + 17 module + 9 stdlib + 6 FFI).

**Implementation notes:**
- Module system lives in `crates/vm/src/modules.rs` (not a separate `modules` crate) — tight VM integration needed for expression-at-a-time compile+execute
- FFI lives in `crates/vm/src/ffi.rs`, gated behind `#[cfg(feature = "ffi")]` (default-on)
- **No `libffi` dependency** — uses `unsafe` transmute-based dispatch for known C signatures (f64→f64, f64,f64→f64, etc.). Sufficient for Marlang's FFI use cases (libm). Add `libffi` later if arbitrary signatures needed.
- **`FfiLibrary` uses `Arc`** — `libloading::Library` doesn't implement `Clone`, so `VMObject::FfiLibrary` wraps `Arc<FfiLibrary>` instead of `Box`
- **Dirty globals tracking** — The VM has `dirty_globals: Option<HashSet<String>>`. When enabled, `SetGlobal` records writes. Module loading enables this before running a module, then exports only written globals. This correctly handles modules that redefine constructors structurally identical to builtins (e.g., `result.mar` redefining `Ok`/`Err`).
- **`compile_expr_with_context()`** — New public API in `compiler.rs` that compiles a single expression with external `known_constructors` and `known_effect_ops`. Returns `CompileResult` with updated knowledge for accumulation across expressions.
- **`STRUCT_REGISTRY`** — Global `LazyLock<Mutex<HashMap<String, Arc<FfiStructDef>>>>` for FFI struct definitions (mirrors TS `vmStructRegistry`)
- **Test fixtures** — 6 `.mar` files in `port/tests/fixtures/modules/`: math, types, uses-math, effects, circular-a, circular-b
- **Stdlib tests** — 9 representative tests covering Option (map, unwrap-or, is-some/is-none), Result (map, is-ok/is-err, unwrap-or), IO (println with handler)

### Phase 6: CLI ✅

**Goal:** Standalone `mar` binary for non-concurrent programs. Uses `vm.interpret()` directly — no scheduler dependency yet.

- [x] Add `crates/cli/` binary crate with `clap` derive API
- [x] Implement `run` subcommand (compile + interpret via VM, optional `--no-typecheck`)
- [x] Implement `typecheck` subcommand (with `--verbose` for inferred types)
- [x] Implement `disasm` subcommand (bytecode disassembly)
- [x] Implement `repl` subcommand with `rustyline` and history
- [x] Wire module loader into all commands
- [x] Update CLAUDE.md and documentation

**Deliverable:** `mar run file.mar`, `mar repl`, `mar typecheck file.mar`, `mar disasm file.mar` — all working as a single ~5-10 MB binary. Concurrency effects (`run-fibers`) not yet supported; `mar lsp` added in Phase 9.

**Implementation notes:**
- Binary size: 2.1 MB (release build, macOS arm64) — well under 10 MB target
- Startup time: ~5 ms (meets <10 ms target)
- Runtime performance: needs optimization (fib(35) takes 18s vs 3.5s in TypeScript VM) — tracked for future work
- REPL history saved to `~/.marlang_history`
- Stdlib imports work via `(import "std:option" as opt)` — module loader auto-detects stdlib directory
- Qualified imports use `qualifier/name` syntax (e.g., `opt/map`)

### Phase 7: Concurrency — Single-Threaded ✅

**Goal:** Full fiber scheduler with channels, actors, and synchronous IO on a single scheduler thread.

- [x] Implement `SchedulerState` struct with ready queue, fiber types, channels, mailboxes
- [x] Implement `Fiber` enum (`New`/`Suspended`) with PID tracking
- [x] Port Layer 1: fiber-spawn, fiber-yield, fiber-self, chan-new, chan-send, chan-recv, chan-close
- [x] Port Layer 2: actor-send, actor-receive, actor-receive-match, actor-monitor, actor-link, actor-unlink, actor-trap-exit
- [x] Port Layer 3: synchronous IO effects (print, println, read-line, read-file, write-file)
- [x] Port `run-fibers` builtin
- [x] Implement Erlang-style supervision (monitors deliver `(Down pid reason)`, links propagate crashes, trap-exit converts to messages)
- [x] Dummy handler frame with 18 handled effects installed/reinstalled around each fiber run
- [x] 24 concurrency tests (fibers, channels, actors, supervision patterns)

**Deliverable:** `cargo test -p marlang-vm` passes 249 tests (225 existing + 24 concurrent).

**Implementation notes:**
- Purely synchronous, cooperative scheduler — no Tokio, no async/await
- Single file: `crates/vm/src/scheduler.rs` (~760 lines)
- `run_fibers(vm, closure)` → `Result<Value, VMError>` — main entry point
- Effect dispatch via `VMSignal::Effect` — PERFORM removes handler, scheduler reinstalls before next fiber
- Parent resumed FIRST on `fiber-spawn` (critical for monitor/link setup before child runs)
- Channels support buffered and unbuffered (rendezvous) semantics
- `actor-receive-match` does selective receive with predicate closure
- Exit handling: normal exit notifies monitors, crash propagates through links unless trap-exit enabled
- No SharedState/ProcessRegistry/ChannelRegistry — all state local to single scheduler (deferred to Phase 7b)

### Phase 7b: Concurrency — Multi-Threaded ✅

**Goal:** Scale to N scheduler threads with work stealing and cross-thread messaging.

- [x] Add `crossbeam-deque` work-stealing deques
- [x] Add `crossbeam-channel` inboxes for inter-scheduler messaging
- [x] Implement `pick_scheduler_for_spawn()` with local bias
- [x] Implement work stealing (`try_steal()`)
- [x] Implement cross-thread actor message routing via shared state
- [x] Implement cross-thread channel sends via shared state
- [x] Implement global termination detection (atomic idle counter)
- [x] Add multi-threaded concurrency tests (parallel fib, cross-thread actors)

**Deliverable:** Multi-threaded runtime (`runtime.rs`) passes 11 parallel tests + all existing concurrent tests. Uses `std::thread::scope` for safe spawning, `VM::new_with_globals()` per thread.

### Phase 7c: Async IO (Tokio Integration) ✅

**Goal:** Non-blocking IO in the multi-threaded scheduler via tokio.

- [x] Add `tokio` dependency (`rt-multi-thread`, `fs`, `time`, `io-util`)
- [x] Create shared tokio runtime (2 worker threads) in `run_fibers_parallel()`
- [x] Convert all 5 IO effects to async: park fiber → spawn tokio task → `IoComplete` message
- [x] `read-file`/`write-file` use `tokio::fs` for true async file IO
- [x] `read-line` uses `tokio::task::spawn_blocking` (stdin is inherently blocking)
- [x] `print`/`println` async for consistency (write to captured_output or stdout)
- [x] `IoComplete { pid, result }` handled in `drain_inbox` and idle-wait path
- [x] `pending_io: AtomicUsize` prevents premature termination while IO is in flight
- [x] `captured_output` changed to `Arc<Mutex<Option<String>>>` for cloning into async tasks
- [x] Add 4 async IO tests (read, write, concurrent, error handling)

**Deliverable:** 305 total tests pass (264 at time of writing, plus 41 added in later sub-phases for macros, prelude, etc.). Single-threaded scheduler (`scheduler.rs`) unchanged — IO stays synchronous there.

### Phase 8: TCP Networking (Weeks 14-15)

**Goal:** Raw TCP socket support via Tokio, integrated with the multi-threaded scheduler.

- [x] Add tokio features: `net`, `sync`, `macros` to `marlang-vm` Cargo.toml
- [x] Implement TCP state types: `TcpSocketState`, `TcpListenerState`, `TcpOp` enum
- [x] Add `SchedulerMessage` variants: `TcpData`, `TcpClosed`, `TcpAccepted`, `TcpOp`, `TcpOpResult`
- [x] Add `SharedState` fields: `tcp_socket_owners`, `tcp_listener_owners`, `next_tcp_id`, `active_listeners`
- [x] Add `PerScheduler` fields: `tcp_sockets`, `tcp_listeners`
- [x] Implement 6 TCP effect handlers: `tcp-connect`, `tcp-listen`, `tcp-accept`, `tcp-send`, `tcp-recv`, `tcp-close`
- [x] Implement cross-scheduler TCP routing via `TcpOp` / `TcpOpResult`
- [x] Background read tasks push `TcpData` / `TcpClosed` to owning scheduler
- [x] Background accept loop with `tokio::select!` for graceful shutdown
- [x] Pre-allocated socket ID batches (`Arc<AtomicU64>`) for accept loop
- [x] Termination detection: `active_listeners` prevents premature shutdown
- [x] Single-threaded scheduler rejects TCP effects with clear error message
- [x] TCP helper functions: `tcp_close_socket`, `tcp_handle_data`, `tcp_handle_closed`, `tcp_handle_accepted`, `tcp_handle_op`
- [x] Extract `process_single_message()` to eliminate duplication between `drain_inbox` and idle-wait path
- [x] 9 TCP tests: listen+close, echo server, recv-nil-on-close, send+receive, listener-close-wakes-acceptor, multiple-clients, bidirectional, connect-failure, single-threaded-rejects
- [x] Exception effect: `throw` in `HANDLED_EFFECTS` (both schedulers), default handler crashes fiber
- [x] TCP effects return `Result` ADT values (`(Ok value)` / `(Err msg)`) instead of crashing fibers
- [x] `make_ok()` / `make_err()` helpers for constructing Result ADTs in runtime
- [x] `net.mar` updated: throwing wrappers (connect, listen, send) and try wrappers (try-connect, try-listen, try-send)
- [x] Exception effect `(effect Exception (throw (-> String Unit)))` declared in prelude
- [x] Fix: single-threaded scheduler child fiber effect errors no longer propagate via `?` (crash fiber instead)
- [x] Fix: `resume_continuation` uses `run_restored_continuation` for restored user handler dispatch
- [x] 4 exception/throw tests: throw-crashes-fiber, throw-caught-by-handle, tcp-connect-result-matching, tcp-listen-duplicate-port-error
- [x] Fix: module globals resolution — `merge_exports` for `Qualified` imports now installs unqualified names alongside prefixed names, so module-internal cross-function calls work when the closure is invoked from the parent scope
- [x] Fix: `examples/tcp.mar` — multi-expression match branches wrapped with `begin`, Example 3 changed from recursive to unrolled server pattern

**Deliverable:** 318 total tests pass (9 TCP + 4 exception tests). Exception effect available in both schedulers. TCP errors are recoverable via Result matching.

**Known Issues (Phase 8):**

1. **Module globals resolution is a workaround, not a proper fix.** When a module is imported with `(import "std:net" as net)`, `merge_exports` installs both prefixed (`net/listen`) and unqualified (`listen`) names into the parent's globals. The unqualified names are needed because closures exported from the module reference sibling functions by unprefixed names via `PUSH_GLOBAL`. However, the `contains_key` guard means whichever module is imported first "wins" for unqualified names — two modules defining the same unqualified name would collide. The proper fix is for closures to capture a reference to their module's globals table (module-scoped globals), or for the compiler to resolve module-internal references as upvalues rather than globals.

2. **Recursive `define` inside `fiber-spawn` + TCP effects hangs.** Using `(define handle-client (lambda (n) ... (handle-client (+ n 1))))` inside a `fiber-spawn` lambda, where the recursive body performs TCP effects (accept, recv, send, close), causes the program to hang. The `define` sets a global on the scheduler thread's VM, but after a TCP effect suspends/resumes the fiber, `PUSH_GLOBAL "handle-client"` may not resolve correctly. **Workaround:** Unroll the recursion, or avoid combining recursive `define` with scheduler-suspending effects.

3. **Type checker rejects `net.mar` effect signatures.** Running with type checking (without `--no-typecheck`) fails with `"function type: expected 2 arguments, got 3"`. The type checker doesn't support parameterized types like `(Result String TcpSocket)` in effect operation signatures — it interprets the 3-element list as a function application. **Workaround:** Always use `--no-typecheck` for programs importing `std:net`.

4. **User `handle` blocks don't work reliably across scheduler effect boundaries.** When a fiber is parked by a scheduler effect (e.g., `tcp-connect`) and later resumed, user-level `handle` frames are captured in the continuation and restored. If `throw` fires after resume, the restored handler catches it, but the handler's return value goes to the scheduler's dispatch (not back into the fiber's code flow). **Workaround:** Use `try-` wrappers from `net.mar` and match on Result directly instead of wrapping `handle`/`throw` around TCP operations.

5. **`match` branches require single-expression bodies.** Match branches must be `(pattern body)` where body is a single expression. Multi-expression bodies need explicit `(begin ...)` wrapping. This is by design in the compiler (`compile_match` checks `branch_elements.len() != 2`), but is a common source of user errors when writing side-effecting match branches.

### Phase 9: LSP (Weeks 15-16)

**Goal:** Language server with diagnostics, hover, definition, completion.

- [ ] Set up `tower-lsp` server
- [ ] Port analyzer (lex → parse → typecheck pipeline)
- [ ] Port symbol collection and position-based lookup
- [ ] Port hover handler (type information)
- [ ] Port go-to-definition handler
- [ ] Port completion handler
- [ ] Port diagnostics publishing
- [ ] Add `mar lsp` subcommand to CLI

**Deliverable:** `cargo test -p marlang-lsp` passes editor-ops (25) tests.

### Phase 10: WASM & Web Editor (Weeks 16-18)

**Goal:** Compile core+types+vm to WASM; web editor imports the WASM module instead of bundling TypeScript.

- [ ] Add `crates/wasm/` cdylib crate with `wasm-bindgen` exports
- [ ] Implement `Value` tagged-enum fallback behind `#[cfg(target_arch = "wasm32")]`
- [ ] Gate Tokio/networking behind `#[cfg(not(target_arch = "wasm32"))]` in `vm` crate
- [ ] Export WASM API: `parse`, `typeCheck`, `run`, `buildTypeMap`, `formatValue`, `getBuiltinReference`
- [ ] Build with `wasm-pack build --target web`
- [ ] Update `web/` to import `marlang_wasm` instead of `@marlang/browser`
- [ ] Wire up IO shims (print→console.log, read-line→prompt) via WASM imports
- [ ] Test web editor end-to-end: parse, typecheck, run, hover types, structural editing

**Deliverable:** Web editor at `web/` works with the WASM module. `wasm-pack build` produces a `<500 KB` `.wasm` file.

---

## 15. WASM & Web Editor

### Background

The current web editor (`web/`) is a SolidJS app with a structural code editor (Vim-style AST navigation). It imports Marlang's TypeScript source directly via Vite, using only:

- **Lexer + Parser** — `Marlang.parse(code)`
- **Type checker** — `Marlang.typeCheck(exprs)`, `Marlang.buildTypeMap(exprs)`
- **Tree-walker interpreter** — `Marlang.run(source)` (no VM, no concurrency, no networking)
- **IO shims** — `print`→`console.log`, `read-line`→`prompt()`, file IO→error

It does **not** use FFI, networking, concurrency, or the VM scheduler. This makes it an ideal WASM target: only the core pipeline (lex, parse, typecheck, compile, execute) needs to cross the WASM boundary.

### Value Representation: Tagged Enum Fallback

NaN-boxing packs object pointers into 48 bits, which doesn't work on WASM (32-bit pointers). Use conditional compilation:

```rust
// crates/vm/src/value.rs

/// On native (64-bit): NaN-boxed u64 — 8 bytes, zero-cost arithmetic.
#[cfg(not(target_arch = "wasm32"))]
#[repr(transparent)]
pub struct Value(u64);

/// On WASM (32-bit): tagged enum — 16 bytes, still fast.
/// Discriminant + payload. Numbers are unboxed (no allocation).
#[cfg(target_arch = "wasm32")]
pub enum Value {
    Num(f64),
    Nil,
    Bool(bool),
    Obj(Arc<VMObject>),
}
```

Both representations expose the same public API (`from_f64`, `as_f64`, `from_object`, `as_object`, `is_falsy`, `NIL`, `TRUE`, `FALSE`), so the rest of the VM is unchanged. The cost on WASM is 16 bytes per stack slot instead of 8 — acceptable given WASM's 4 GB address space and the fact that the web editor runs small programs.

### Conditional Compilation for VM Features

The `vm` crate uses feature flags to exclude native-only code from the WASM build:

```toml
# crates/vm/Cargo.toml
[features]
default = ["ffi"]
ffi = ["libloading"]
# Future: native = ["ffi", "tokio", "reqwest", "hyper", "crossbeam-deque", "crossbeam-channel", "num_cpus", "fastrand"]
```

```rust
// crates/vm/src/lib.rs
pub mod bytecode;
pub mod compiler;
pub mod vm;
pub mod builtins;
pub mod debug;

#[cfg(feature = "native")]
pub mod scheduler;  // Multi-scheduler with work stealing — Tokio + crossbeam
#[cfg(feature = "native")]
pub mod runtime;    // Runtime initialization, thread management
```

The WASM crate depends on `vm` without the `native` feature:

```toml
# crates/wasm/Cargo.toml
[dependencies]
marlang-core = { path = "../core" }
marlang-types = { path = "../types" }
marlang-vm = { path = "../vm", default-features = false }  # no FFI in WASM
wasm-bindgen = "0.2"
serde = { version = "1", features = ["derive"] }
serde-wasm-bindgen = "0.6"
```

### WASM API Surface

The `wasm-bindgen` exports mirror the current `browser.ts` API that the web editor already uses:

```rust
// crates/wasm/src/lib.rs
use wasm_bindgen::prelude::*;
use marlang_core::{lex, parse, Expr};
use marlang_types::{type_check, build_type_map, format_error, format_scheme};
use marlang_vm::{compile_program, VM, interpret, format_value, Interner};
use marlang_modules::register_stdlib;

/// Global state for the WASM module (single-threaded, no Send needed).
thread_local! {
    static INTERNER: RefCell<Interner> = RefCell::new(Interner::new());
}

/// Parse source code into a serialized AST.
#[wasm_bindgen(js_name = "parse")]
pub fn wasm_parse(source: &str) -> Result<JsValue, JsError> {
    INTERNER.with_borrow_mut(|interner| {
        let tokens = lex(source, interner).map_err(|e| JsError::new(&e.to_string()))?;
        let ast = parse(&tokens).map_err(|e| JsError::new(&e.to_string()))?;
        Ok(serde_wasm_bindgen::to_value(&ast)?)
    })
}

/// Type check parsed expressions. Returns { ok, error?, env? }.
#[wasm_bindgen(js_name = "typeCheck")]
pub fn wasm_type_check(source: &str) -> Result<JsValue, JsError> {
    INTERNER.with_borrow_mut(|interner| {
        let tokens = lex(source, interner)?;
        let ast = parse(&tokens)?;
        let result = type_check(&ast, None);
        Ok(serde_wasm_bindgen::to_value(&result)?)
    })
}

/// Build a type map: node path → inferred type string.
#[wasm_bindgen(js_name = "buildTypeMap")]
pub fn wasm_build_type_map(source: &str) -> Result<JsValue, JsError> {
    // Returns Map<string, string> for hover info in the editor
    /* ... */
}

/// Parse, optionally typecheck, and execute. Returns formatted result string.
#[wasm_bindgen]
pub fn run(source: &str, typecheck: bool) -> Result<String, JsError> {
    INTERNER.with_borrow_mut(|interner| {
        let tokens = lex(source, interner)?;
        let ast = parse(&tokens)?;

        if typecheck {
            let tc = type_check(&ast, None);
            if let Err(e) = tc {
                return Err(JsError::new(&format!("Type error: {}", format_error(&e))));
            }
        }

        let chunk = compile_program(&ast, interner);
        let mut vm = VM::new_with_interner_ref(interner);
        register_builtins(&mut vm);
        register_wasm_io(&mut vm);  // print→console.log, etc.

        let result = interpret(&mut vm, &chunk)
            .map_err(|e| JsError::new(&e.to_string()))?;
        Ok(format_value(result, interner))
    })
}

/// Get builtin reference data for the editor's reference panel.
#[wasm_bindgen(js_name = "getBuiltinReference")]
pub fn wasm_get_builtin_reference() -> JsValue {
    /* ... */
}
```

### IO Shims in WASM

The web editor needs `print` and `println` to work. In WASM, these call into JavaScript:

```rust
// crates/wasm/src/lib.rs

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console)]
    fn log(s: &str);
}

/// Register browser-compatible IO handlers.
fn register_wasm_io(vm: &mut VM) {
    vm.register_fn("print", |s: String| { log(&s); });
    vm.register_fn("println", |s: String| { log(&s); });
    // read-line, read-file, write-file → error in browser
    vm.register_fn("read-line", || -> Result<String, VMError> {
        Err(VMError::new("read-line is not available in the browser"))
    });
}
```

### Web Editor Changes

The web editor (`web/`) keeps its existing SolidJS UI, structural editor, and Vim keybindings. The only change is the import source:

```typescript
// Before (TypeScript — bundles entire Marlang source via Vite):
import * as Marlang from '@marlang/browser';

// After (WASM — thin 500KB module):
import init, * as Marlang from 'marlang-wasm';
await init(); // Load WASM binary
```

The API signatures are identical (`parse`, `typeCheck`, `run`, `buildTypeMap`, `formatValue`, `getBuiltinReference`), so the rest of `App.tsx`, `editor-ops.ts`, `ASTRenderer.tsx`, and `ReferencePanel.tsx` remain unchanged.

### Build Pipeline

```bash
# Build WASM module
cd crates/wasm
wasm-pack build --target web --out-dir ../../web/pkg

# Dev server (web editor)
cd web
npm run dev    # Vite serves on :3000, imports ./pkg/marlang_wasm.js

# Production build
cd web
npm run build  # Vite bundles SolidJS + WASM into dist/
```

The Vite config drops the `@marlang` alias (no longer imports TypeScript source) and the `bun:ffi` shim (FFI not compiled into WASM):

```typescript
// web/vite.config.ts (updated)
import { defineConfig } from 'vite';
import solid from 'vite-plugin-solid';

export default defineConfig({
  plugins: [solid()],
  server: { port: 3000 },
});
```

### Size Budget

| Component | Estimated WASM Size |
|-----------|-------------------|
| Core (lexer + parser) | ~50 KB |
| Types (inference + unification) | ~120 KB |
| VM (compiler + execution) | ~150 KB |
| Modules (stdlib bundled) | ~30 KB |
| wasm-bindgen glue | ~20 KB |
| **Total** | **~370 KB** (gzipped ~120 KB) |

This is significantly smaller than the current approach (Vite bundles the entire TypeScript source + Bun polyfills).

---

## Appendix: Cargo Dependencies

```toml
[workspace]
members = ["crates/*"]

[workspace.dependencies]
# Core
# (no external deps — lexer/parser are self-contained)

# Types
# (no external deps — pure algorithms)

# VM + Scheduler
tokio = { version = "1", features = ["rt", "rt-multi-thread", "net", "io-util", "sync", "time"] }
reqwest = { version = "0.12", features = ["rustls-tls"], default-features = false }
hyper = { version = "1", features = ["server", "http1"] }
crossbeam-deque = "0.8"        # Work-stealing deques for multi-scheduler
crossbeam-channel = "0.5"      # Lock-free MPSC for inter-scheduler messaging
dashmap = "6"                  # Concurrent hashmap for ProcessRegistry, ChannelRegistry
num_cpus = "1"                 # Detect core count for default scheduler count
fastrand = "2"                 # Fast thread-local RNG for work-stealing randomization
libloading = "0.8"             # Cross-platform dlopen for FFI
libffi = "3"                   # Calling convention marshalling for arbitrary C signatures

# Modules
dirs = "6"                     # Home directory resolution for stdlib lookup

# LSP
tower-lsp = "0.20"
# dashmap — shared with VM (see above)

# CLI
clap = { version = "4", features = ["derive"] }
rustyline = "14"

# WASM
wasm-bindgen = "0.2"
serde = { version = "1", features = ["derive"] }
serde-wasm-bindgen = "0.6"

# Testing
criterion = { version = "0.5", features = ["html_reports"] }
```

## Appendix: What Gets Dropped

The Rust port intentionally drops:

1. **Tree-walker interpreter** (`src/core/interpreter.ts`, ~1200 LOC) — the VM is strictly better. The 60 interpreter tests become VM integration tests.
2. **`bun:ffi` runtime** — replaced by `libloading` + `libffi` for dlopen FFI. Same Marlang API (`ffi-open`, `ffi-call`, etc.).
3. **`--vm` flag** — no longer needed since everything runs on the VM.
4. **Node.js `readline`** — replaced by `rustyline` with history support.
5. **Hand-rolled LSP protocol** (`lsp/protocol.ts`) — replaced by `tower-lsp`.
6. **Vite/SolidJS bundling of TypeScript source** — the web editor (`web/`) instead imports a WASM module (see Section 15).

## Appendix: Value Representation Comparison

| | TypeScript | Rust |
|---|---|---|
| Numbers | Unboxed JS `number` (f64) | NaN-boxed `u64` (zero-cost f64) |
| Booleans | `VMObject { kind: 'boolean' }` | Immediate tag in `u64` (no allocation) |
| Nil | `VMObject { kind: 'nil' }` | Immediate tag in `u64` (no allocation) |
| Strings | `VMObject { kind: 'string' }` | `Arc<VMObject::String>` (8-byte `Value`) |
| Closures | `VMObject { kind: 'vm-closure' }` | `Arc<VMObject::Closure>` (8-byte `Value`) |
| ADTs | `VMObject { kind: 'adt-value' }` | `Arc<VMObject::AdtValue>` (8-byte `Value`) |
| Stack slot | 8 bytes (V8 tagged) or ~32 bytes (object) | 8 bytes (always) |

**Key win:** Every `Value` is exactly 8 bytes on the stack. No JS object headers, no V8 hidden classes, no GC tracing overhead.
