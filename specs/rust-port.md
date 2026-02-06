# Porting Marlang to Rust

**Type:** Architecture Specification
**Scope:** Full port — lexer, parser, type system, bytecode VM, scheduler, modules, LSP, CLI
**Goals:** Performance (5× fib40, <10 ms startup, 3× less memory) + standalone binary

---

## Table of Contents

1. [Overview & Goals](#1-overview--goals)
2. [Crate Structure](#2-crate-structure)
3. [Core Data Structures](#3-core-data-structures)
4. [Lexer & Parser](#4-lexer--parser)
5. [Type System](#5-type-system)
6. [Bytecode Compiler](#6-bytecode-compiler)
7. [VM Execution Engine](#7-vm-execution-engine)
8. [Concurrency](#8-concurrency)
9. [FFI](#9-ffi)
10. [Module System](#10-module-system)
11. [LSP](#11-lsp)
12. [CLI](#12-cli)
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
│   │       ├── scheduler.rs # Fiber scheduler, channels, actors
│   │       └── debug.rs    # Disassembler
│   ├── modules/            # marlang-modules
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── loader.rs   # Path resolution, file loading
│   │       └── stdlib.rs   # Bundled stdlib (include_str!)
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
cli ──→ lsp ──→ modules ──→ types ──→ core
  │              │                     ↑
  └──→ vm ──────→──────→ types ───────┘
        ↑
wasm ───┘──→ modules ──→ types ──→ core
```

Every crate depends on `core` (for AST and spans). The `vm` crate depends on `core` and `types` (for the compiler which must handle type annotations). The `modules` crate depends on `core`. The `lsp` crate depends on `modules` and `types`. The `cli` crate depends on everything. The `wasm` crate depends on `vm`, `modules`, `types`, and `core` — but **not** `lsp` or `cli`.

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

```rust
/// Core type representation for HM inference.
#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    Var(Symbol),                                      // type-var
    Const(Symbol),                                    // type-const (Int, Bool, String, ...)
    App { constructor: Box<Type>, args: Vec<Type> },  // type-app (List Int), (Option a)
    Arrow {
        params: Vec<Type>,
        effects: Option<EffectRow>,
        result: Box<Type>,
    },
    Rec { var: Symbol, body: Box<Type> },             // recursive types
}

#[derive(Debug, Clone, PartialEq)]
pub enum EffectRow {
    Closed(Vec<Symbol>),                     // {IO, State}
    Open { effects: Vec<Symbol>, row_var: Symbol }, // {IO | r}
}

#[derive(Debug, Clone)]
pub struct Constraint {
    pub class: Symbol,    // Num, Eq, Ord, Show
    pub type_arg: Type,
}

#[derive(Debug, Clone)]
pub struct QualType {
    pub constraints: Vec<Constraint>,
    pub ty: Type,
}

#[derive(Debug, Clone)]
pub struct TypeScheme {
    pub quantified_vars: Vec<Symbol>,
    pub qual_type: QualType,
}

/// Patterns for match expressions and exhaustiveness checking.
#[derive(Debug, Clone)]
pub enum Pattern {
    Wildcard,
    Var(Symbol),
    Literal(LiteralPattern),
    Constructor { name: Symbol, args: Vec<Pattern> },
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
///   0x7FF8_0000_0000_0000 | tag | payload → tagged pointer or immediate
///
/// Tags (bits 48..50):
///   000 → object pointer (48-bit, aligned — lower 3 bits always zero)
///   001 → nil
///   010 → true
///   011 → false
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct Value(u64);

const QNAN: u64          = 0x7FF8_0000_0000_0000;
const TAG_OBJ: u64       = 0x0000_0000_0000_0000;
const TAG_NIL: u64       = 0x0001_0000_0000_0000;
const TAG_TRUE: u64       = 0x0002_0000_0000_0000;
const TAG_FALSE: u64      = 0x0003_0000_0000_0000;
const PTR_MASK: u64       = 0x0000_FFFF_FFFF_FFFF;

impl Value {
    pub const NIL: Value = Value(QNAN | TAG_NIL);
    pub const TRUE: Value = Value(QNAN | TAG_TRUE);
    pub const FALSE: Value = Value(QNAN | TAG_FALSE);

    #[inline]
    pub fn from_f64(n: f64) -> Value {
        Value(n.to_bits())
    }

    #[inline]
    pub fn from_object(obj: Arc<VMObject>) -> Value {
        let ptr = Arc::into_raw(obj) as u64;
        debug_assert!(ptr & !PTR_MASK == 0, "pointer exceeds 48 bits");
        Value(QNAN | TAG_OBJ | ptr)
    }

    #[inline]
    pub fn is_number(&self) -> bool {
        // Any bit pattern where the QNAN prefix is NOT set is a real f64
        (self.0 & QNAN) != QNAN
    }

    #[inline]
    pub fn as_f64(&self) -> Option<f64> {
        if self.is_number() { Some(f64::from_bits(self.0)) } else { None }
    }

    #[inline]
    pub fn as_object(&self) -> Option<Arc<VMObject>> {
        if (self.0 & (QNAN | 0x0007_0000_0000_0000)) == QNAN {
            let ptr = (self.0 & PTR_MASK) as *const VMObject;
            // Reconstruct the Arc without dropping the original (increment refcount)
            let arc = unsafe { Arc::from_raw(ptr) };
            let clone = arc.clone();
            std::mem::forget(arc); // Don't decrement — we still hold it in the Value
            Some(clone)
        } else {
            None
        }
    }

    #[inline]
    pub fn is_falsy(&self) -> bool {
        // Only nil and #f are falsy (matches TS behavior)
        self.0 == Self::NIL.0 || self.0 == Self::FALSE.0
    }
}

impl Drop for Value {
    fn drop(&mut self) {
        // If this Value holds an object pointer, drop the Arc
        if (self.0 & (QNAN | 0x0007_0000_0000_0000)) == QNAN && (self.0 & PTR_MASK) != 0 {
            let ptr = (self.0 & PTR_MASK) as *const VMObject;
            unsafe { Arc::from_raw(ptr); } // Reconstructs and drops, decrementing refcount
        }
    }
}

impl Clone for Value {
    fn clone(&self) -> Self {
        if (self.0 & (QNAN | 0x0007_0000_0000_0000)) == QNAN && (self.0 & PTR_MASK) != 0 {
            // Object pointer — increment refcount
            let ptr = (self.0 & PTR_MASK) as *const VMObject;
            let arc = unsafe { Arc::from_raw(ptr) };
            std::mem::forget(arc.clone()); // Increment refcount, leak the clone into the new Value
            std::mem::forget(arc);         // Don't decrement the original
        }
        Value(self.0)
    }
}
```

> **Platform assumptions:** NaN-boxing relies on 48-bit virtual addresses (x86-64 canonical addresses, ARM64 default). For WASM (32-bit pointers), a tagged enum fallback is used — see [Section 15: WASM & Web Editor](#15-wasm--web-editor) for details.
>
> **Note:** The exact bit layout will be finalized during implementation. The `Clone`/`Drop` impls above demonstrate the refcounting discipline: every `Value` that holds an object pointer owns one `Arc` refcount. The key property: numbers are zero-cost, object access requires one branch + refcount increment.

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

    // Concurrency
    Pid(u64),
    Channel(u64),

    // FFI (Phase 2)
    // FfiLibrary(..),
    // FfiPointer(..),
    // FfiStructDef(..),
    // FfiStruct(..),

    // Networking
    TcpSocket(u64),    // Socket ID, resolved by scheduler
    TcpListener(u64),
    HttpServer(u64),
    HttpRequest(u64),
}

pub type BuiltinFn = fn(&mut VM, &[Value]) -> Result<Value, VMError>;
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

/// Constant pool entry — pre-converted to Value at load time.
pub struct Chunk {
    pub code: Vec<Instruction>,
    pub constants: Vec<Value>,      // Pre-converted (no separate ConstantValue type)
    pub local_count: u16,
    pub source_file: Option<Arc<str>>,
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

## 4. Lexer & Parser

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

## 5. Type System

The type system is a Hindley-Milner inference engine with typeclasses, algebraic effects, and exhaustiveness checking. It's entirely pure-functional (no mutation of the inference state) making it a clean fit for Rust.

### Architecture

```
types/
├── ast.rs       — Type, TypeScheme, Constraint, Pattern, EffectRow
├── env.rs       — TypeEnv (immutable bindings map), Substitution
├── unify.rs     — Robinson's unification
├── infer.rs     — Constraint generation + HM inference
├── builtins.rs  — Built-in types (Int, Float, Bool, String) and classes (Num, Eq, Ord)
├── checker.rs   — Top-level type_check() entry point
└── errors.rs    — TypeError, formatting
```

### Type Environment

```rust
/// Immutable type environment. Clone-on-extend for lexical scoping.
#[derive(Clone)]
pub struct TypeEnv {
    pub bindings: HashMap<Symbol, TypeScheme>,
    pub type_constructors: HashMap<Symbol, Vec<ConstructorInfo>>,
    pub classes: HashMap<Symbol, ClassInfo>,
    pub instances: Vec<InstanceInfo>,
    pub effect_ops: HashMap<Symbol, EffectOpInfo>,
}

/// Substitution: mapping from type variables to types.
pub type Substitution = HashMap<Symbol, Type>;

pub fn apply_subst(subst: &Substitution, ty: &Type) -> Type { /* ... */ }
pub fn compose_subst(s1: &Substitution, s2: &Substitution) -> Substitution { /* ... */ }
```

### Unification

```rust
/// Robinson's unification. Returns a substitution or a type error.
pub fn unify(t1: &Type, t2: &Type, env: &TypeEnv) -> Result<Substitution, TypeError> {
    match (t1, t2) {
        (Type::Var(a), _) => bind(*a, t2),
        (_, Type::Var(b)) => bind(*b, t1),
        (Type::Const(a), Type::Const(b)) if a == b => Ok(HashMap::new()),
        (Type::Arrow { params: p1, result: r1, .. },
         Type::Arrow { params: p2, result: r2, .. }) => {
            // Unify parameter lists and results
            /* ... */
        }
        (Type::App { constructor: c1, args: a1 },
         Type::App { constructor: c2, args: a2 }) => {
            let s = unify(c1, c2, env)?;
            // Unify args pairwise under accumulated substitution
            /* ... */
        }
        _ => Err(TypeError::Mismatch { expected: t1.clone(), found: t2.clone() }),
    }
}
```

### Inference

```rust
pub struct InferResult {
    pub ty: Type,
    pub subst: Substitution,
    pub constraints: Vec<Constraint>,
}

pub fn infer_expr(env: &TypeEnv, expr: &Expr) -> Result<InferResult, TypeError> { /* ... */ }

pub fn infer_program(
    env: &TypeEnv,
    exprs: &[Expr],
    file_path: Option<&str>,
) -> Result<InferProgramResult, TypeError> { /* ... */ }
```

### Exhaustiveness Checking

Pattern matching exhaustiveness follows the same algorithm as TypeScript: for each `match` expression, compute the set of uncovered constructors after all arms and report an error if non-empty. The `Pattern` type maps directly (Section 3.5).

### Error Types

```rust
pub enum TypeError {
    Mismatch { expected: Type, found: Type, span: Span },
    UnboundVariable { name: Symbol, span: Span },
    OccursCheck { var: Symbol, ty: Type, span: Span },
    InexhaustiveMatch { missing: Vec<String>, span: Span },
    NoInstance { class: Symbol, ty: Type, span: Span },
    ArityMismatch { expected: usize, found: usize, span: Span },
    // ... other variants matching TypeScript
}

impl fmt::Display for TypeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { /* ... */ }
}
```

---

## 6. Bytecode Compiler

The compiler transforms `Vec<Expr>` into a `Chunk`. Architecture matches TypeScript exactly:

### Compiler State

```rust
pub struct Compiler<'a> {
    /// Current chunk being compiled into.
    chunk: Chunk,
    /// Local variable slots for the current scope.
    locals: Vec<Local>,
    /// Upvalue descriptors for the current function.
    upvalues: Vec<UpvalueDescriptor>,
    /// Enclosing compiler (for nested functions/closures).
    enclosing: Option<&'a mut Compiler<'a>>,
    /// String interner (shared).
    interner: &'a mut Interner,
    /// Scope depth for variable resolution.
    scope_depth: u32,
}

struct Local {
    name: Symbol,
    depth: u32,
    captured: bool,
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

## 7. VM Execution Engine

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
                    resume_value: Value::from_object(Arc::new(VMObject::Channel(id))),
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

            // === Layer 4: Networking ===
            "tcp-connect" => {
                let addr = extract_string(&signal.args[0]);
                let port = extract_int(&signal.args[1]);
                let io_id = self.next_io_id();
                let sender = self.inbox_sender.clone();
                // Spawn async IO on Tokio's thread pool (NOT on scheduler thread)
                tokio::spawn(async move {
                    let result = TcpStream::connect(format!("{}:{}", addr, port)).await;
                    let _ = sender.send(SchedulerMessage::IoComplete {
                        fiber_pid: pid,
                        result: wrap_tcp_result(result),
                    });
                });
                // Park fiber until IO completes
                self.parked.insert(pid, ParkedFiber {
                    continuation: signal.continuation, pid,
                });
            }

            "tcp-listen"  => { /* bind listener synchronously */ }
            "tcp-accept"  => { /* park fiber until connection arrives */ }
            "tcp-send"    => { /* write to socket */ }
            "tcp-recv"    => { /* park fiber until data arrives */ }
            "tcp-close"   => { /* close socket */ }

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

### Networking (Multi-Threaded)

TCP and HTTP IO use Tokio's **multi-threaded runtime** features. Unlike the single-threaded design, async IO tasks are spawned with `tokio::spawn` (not `spawn_local`), which runs them on Tokio's thread pool. IO completion messages are routed back to the owning scheduler via its inbox:

```rust
// TCP connect — spawns on Tokio's thread pool, notifies scheduler on completion
"tcp-connect" => {
    let addr = extract_string(&signal.args[0]);
    let port = extract_int(&signal.args[1]);
    let sender = self.inbox_sender.clone();
    let fiber_pid = pid;

    tokio::spawn(async move {
        let result = TcpStream::connect(format!("{}:{}", addr, port)).await;
        let value = match result {
            Ok(stream) => {
                let socket_id = register_socket(stream);
                Value::from_object(Arc::new(VMObject::TcpSocket(socket_id)))
            }
            Err(e) => make_error_value(e),
        };
        let _ = sender.send(SchedulerMessage::IoComplete { fiber_pid, result: value });
    });

    // Park the fiber until IO completes
    self.parked.insert(pid, ParkedFiber { continuation: signal.continuation, pid });
}
```

> **Design rationale:** Each scheduler thread has its own `current_thread` Tokio runtime for driving its local `select!` loop (inbox + IO wakeups). Async IO tasks are spawned on the Tokio thread pool via `tokio::spawn`, and results flow back through the crossbeam inbox. This avoids the `LocalSet` limitation of the single-threaded design while keeping all fiber execution `&mut self` (no locks on the hot path).

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

## 9. FFI

### Design: Rust Function Registration API

Instead of the TypeScript `dlopen` approach (which uses `bun:ffi`), the Rust port provides a native embedding API. Host applications register Rust functions directly:

```rust
/// Trait for converting Marlang values to Rust types.
pub trait FromValue: Sized {
    fn from_value(value: Value, vm: &VM) -> Result<Self, VMError>;
}

/// Trait for converting Rust types to Marlang values.
pub trait IntoValue {
    fn into_value(self, vm: &mut VM) -> Value;
}

// Implementations for common types
impl FromValue for f64 { /* extract number */ }
impl FromValue for i64 { /* extract integer */ }
impl FromValue for String { /* extract string */ }
impl FromValue for bool { /* extract bool */ }
impl FromValue for Value { /* identity */ }

impl IntoValue for f64 { /* Value::from_f64 */ }
impl IntoValue for i64 { /* Value::from_f64(n as f64) */ }
impl IntoValue for String { /* allocate VMObject::String */ }
impl IntoValue for bool { /* Value::TRUE / Value::FALSE */ }
impl IntoValue for () { /* Value::NIL */ }
impl IntoValue for Value { /* identity */ }
```

### Registration API

```rust
impl VM {
    /// Register a Rust function as a Marlang global.
    ///
    /// # Example
    /// ```rust
    /// vm.register_fn("square", |x: f64| -> f64 { x * x });
    /// vm.register_fn("greet", |name: String| -> String {
    ///     format!("Hello, {}!", name)
    /// });
    /// ```
    pub fn register_fn<F, Args, Ret>(&mut self, name: &str, func: F)
    where
        F: IntoBuiltin<Args, Ret> + 'static,
    {
        let sym = self.interner.intern(name);
        let builtin_fn = func.into_builtin();
        self.globals.insert(sym, Value::from_object(Arc::new(
            VMObject::Builtin { name: sym, func: builtin_fn }
        )));
    }
}

/// Auto-implements for closures with 0..8 arguments.
pub trait IntoBuiltin<Args, Ret> {
    fn into_builtin(self) -> BuiltinFn;
}

// Macro generates impls for various arities:
// impl<F, R> IntoBuiltin<(), R> for F where F: Fn() -> R, R: IntoValue { ... }
// impl<F, A, R> IntoBuiltin<(A,), R> for F where F: Fn(A) -> R, A: FromValue, R: IntoValue { ... }
// impl<F, A, B, R> IntoBuiltin<(A, B), R> for F where F: Fn(A, B) -> R, ... { ... }
// ... up to 8 args
```

### Embedding Example

```rust
use marlang_vm::{VM, Value, compile_program};
use marlang_core::{lex, parse};

fn main() {
    let mut vm = VM::new();

    // Register custom functions
    vm.register_fn("square", |x: f64| -> f64 { x * x });
    vm.register_fn("http-get", |url: String| -> String {
        reqwest::blocking::get(&url).unwrap().text().unwrap()
    });

    // Run Marlang code
    let source = r#"(println (show (square 7)))"#;
    let tokens = lex(source);
    let ast = parse(&tokens);
    let chunk = compile_program(&ast, &mut vm.interner);
    vm.interpret(&chunk);
}
```

### dlopen FFI (Optional, Phase 2)

For loading shared libraries at runtime (matching the TypeScript `ffi-open`/`ffi-call` builtins), use the `libloading` crate. This is deferred to Phase 2 since the Rust registration API covers the primary use case.

---

## 10. Module System

### Architecture

```rust
pub struct ModuleLoader {
    /// Prefix resolvers: "std" → stdlib, "mylib" → custom path
    prefixes: HashMap<String, Box<dyn PrefixResolver>>,
    /// Module cache: absolute path → parsed AST
    cache: HashMap<PathBuf, Vec<Expr>>,
    /// Virtual modules (for testing)
    virtual_modules: HashMap<PathBuf, String>,
}

pub trait PrefixResolver: Send + Sync {
    fn resolve(&self, name: &str, from_file: &Path) -> Result<PathBuf, ModuleError>;
}

impl ModuleLoader {
    pub fn new() -> Self {
        let mut loader = ModuleLoader::default();
        loader.register_prefix("std", StdlibResolver);
        loader
    }

    pub fn resolve(&self, import_path: &str, from_file: &Path) -> Result<PathBuf, ModuleError> {
        if let Some(colon_pos) = import_path.find(':') {
            let prefix = &import_path[..colon_pos];
            let name = &import_path[colon_pos + 1..];
            self.prefixes.get(prefix)
                .ok_or(ModuleError::UnknownPrefix(prefix.to_string()))?
                .resolve(name, from_file)
        } else if import_path.starts_with('.') {
            // Relative path
            let base = from_file.parent().unwrap_or(Path::new("."));
            Ok(base.join(import_path).with_extension("mar"))
        } else {
            Err(ModuleError::InvalidPath(import_path.to_string()))
        }
    }

    pub fn load(&mut self, path: &Path, interner: &mut Interner) -> Result<&[Expr], ModuleError> {
        if !self.cache.contains_key(path) {
            let source = if let Some(virt) = self.virtual_modules.get(path) {
                virt.clone()
            } else {
                std::fs::read_to_string(path)?
            };
            let tokens = lex(&source, interner)?;
            let ast = parse(&tokens)?;
            self.cache.insert(path.to_path_buf(), ast);
        }
        Ok(&self.cache[path])
    }
}
```

### Stdlib Bundling

Standard library files are embedded at compile time using `include_str!`:

```rust
struct StdlibResolver;

impl PrefixResolver for StdlibResolver {
    fn resolve(&self, name: &str, _from_file: &Path) -> Result<PathBuf, ModuleError> {
        // Return a virtual path — the source is bundled
        Ok(PathBuf::from(format!("$stdlib/{}.mar", name)))
    }
}

/// Register all bundled stdlib modules.
pub fn register_stdlib(loader: &mut ModuleLoader) {
    let modules = [
        ("option",  include_str!("../../stdlib/option.mar")),
        ("result",  include_str!("../../stdlib/result.mar")),
        ("io",      include_str!("../../stdlib/io.mar")),
        ("net",     include_str!("../../stdlib/net.mar")),
        ("http",    include_str!("../../stdlib/http.mar")),
    ];
    for (name, source) in modules {
        loader.register_virtual(
            PathBuf::from(format!("$stdlib/{}.mar", name)),
            source.to_string(),
        );
    }
}
```

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

## 12. CLI

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

The CLI creates a multi-scheduler `Runtime` for commands that use concurrency. Non-concurrent commands (typecheck, disasm) use a single VM directly:

```rust
fn main() {
    let cli = Cli::parse();

    match cli.command {
        Command::Run { file, no_typecheck } => {
            // Create multi-scheduler runtime (one scheduler per core)
            let runtime = Runtime::new(None);
            let chunk = compile_file(&file, &runtime.shared, no_typecheck);
            let closure = Arc::new(Closure { proto: Arc::new(chunk.into_proto()), upvalues: Box::new([]) });
            let result = runtime.run_fibers(closure);
            runtime.shutdown();
        }
        Command::Repl => {
            // REPL uses single scheduler
            let runtime = Runtime::new(Some(1));
            start_repl(&runtime).unwrap();
            runtime.shutdown();
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
| `lexer.test.ts` | 37 | `core` | Unit tests in `lexer.rs` |
| `parser.test.ts` | 34 | `core` | Unit tests in `parser.rs` |
| `interpreter.test.ts` | 60 | `vm` | Integration tests (VM replaces tree-walker) |
| `type-system.test.ts` | 63 | `types` | Unit tests in `infer.rs` |
| `effects.test.ts` | 53 | `vm` | Integration tests |
| `vm.test.ts` | 67 | `vm` | Integration tests |
| `tco.test.ts` | 7 | `vm` | Integration tests |
| `modules.test.ts` | 18 | `modules` | Integration tests |
| `stdlib.test.ts` | 69 | `modules` | Integration tests |
| `concurrent.test.ts` | 25 | `vm` | Async integration tests |
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

### Phase 2: Type System (Weeks 3-5)

**Goal:** Full HM type inference with typeclasses and exhaustiveness.

- [ ] Port type AST (`Type`, `TypeScheme`, `Constraint`, `Pattern`, `EffectRow`)
- [ ] Port `TypeEnv`, `Substitution`, `apply_subst`, `compose_subst`
- [ ] Port Robinson's unification
- [ ] Port constraint generation and inference (`infer_expr`, `infer_program`)
- [ ] Port built-in types and typeclasses (`Num`, `Eq`, `Ord`, `Show`)
- [ ] Port exhaustiveness checking
- [ ] Port error types and formatting
- [ ] Port type checker entry point

**Deliverable:** `cargo test -p marlang-types` passes all 63 type system tests.

### Phase 3: Bytecode Compiler (Weeks 5-6)

**Goal:** Compile Marlang AST to bytecode.

- [ ] Port opcodes, `Instruction`, `Chunk`, `FunctionPrototype`
- [ ] Implement NaN-boxed `Value`
- [ ] Port `VMObject` enum
- [ ] Port compiler: expressions, special forms, closures, pattern matching
- [ ] Port binary operator optimization
- [ ] Port data constructor compilation
- [ ] Port handler descriptor compilation
- [ ] Port disassembler

**Deliverable:** Compiler produces correct bytecode for all language features. Disassembler output matches TypeScript.

### Phase 4: VM Execution (Weeks 6-8)

**Goal:** Execute bytecode with effects and continuations.

- [ ] Implement `VM` struct with stack, frames, globals, handler stack
- [ ] Implement `run()` main loop with `RunResult` enum
- [ ] Port all opcode implementations
- [ ] Port `call_closure()` and `resume_continuation()`
- [ ] Port `run_with_handler()` with deep handler semantics
- [ ] Port default IO handlers
- [ ] Port built-in functions (arithmetic, comparison, list, show, ADTs)
- [ ] Port `interpret()` entry point

**Deliverable:** `cargo test -p marlang-vm` passes VM (67), effects (53), interpreter (60), TCO (7) = **187 tests**.

### Phase 5: Module System (Weeks 8-9)

**Goal:** Import resolution with stdlib bundling.

- [ ] Port `ModuleLoader` with prefix resolution
- [ ] Bundle stdlib via `include_str!`
- [ ] Port module caching
- [ ] Port virtual modules (for testing)
- [ ] Wire module loading into compiler

**Deliverable:** `cargo test -p marlang-modules` passes modules (18) + stdlib (69) = **87 tests**.

### Phase 6: Concurrency — Single-Threaded (Weeks 9-11)

**Goal:** Full fiber scheduler with channels, actors, and async IO on a single scheduler thread.

- [ ] Implement `Scheduler` struct with ready queue, fiber types, and inbox
- [ ] Implement `SharedState`, `ProcessRegistry`, `ChannelRegistry`
- [ ] Implement `Runtime` with `num_schedulers = 1` (single-threaded)
- [ ] Port Layer 1: fiber-spawn, fiber-yield, fiber-self, channels
- [ ] Port Layer 2: actors, mailboxes, monitors, links, supervision
- [ ] Port Layer 3: async IO effects (print, println, read-line, read-file, write-file)
- [ ] Integrate with Tokio (`current_thread` runtime per scheduler)
- [ ] Port `run-fibers` builtin

**Deliverable:** `cargo test -p marlang-vm` passes concurrent (25) tests with a single scheduler.

### Phase 6b: Concurrency — Multi-Threaded (Weeks 11-12)

**Goal:** Scale to N scheduler threads with work stealing and cross-thread messaging.

- [ ] Add `crossbeam-deque` work-stealing deques
- [ ] Add `crossbeam-channel` inboxes for inter-scheduler messaging
- [ ] Implement `pick_scheduler_for_spawn()` with local bias
- [ ] Implement work stealing (`try_steal()`)
- [ ] Implement cross-thread actor message routing via `ProcessRegistry`
- [ ] Implement cross-thread channel sends via `ChannelRegistry`
- [ ] Implement global termination detection (atomic idle counter)
- [ ] Runtime defaults to `num_cpus::get()` scheduler threads
- [ ] Add multi-threaded concurrency tests (parallel fib, cross-thread actors)

**Deliverable:** Multi-threaded runtime passes all 25 existing concurrent tests + new parallel scaling tests.

### Phase 7: Networking (Weeks 12-13)

**Goal:** TCP and HTTP support via Tokio's thread pool.

- [ ] Port TCP: connect, listen, accept, send, recv, close
- [ ] Port HTTP client (replace Bun fetch with `reqwest`)
- [ ] Port HTTP server (replace Bun.serve with `hyper`)
- [ ] Async IO tasks via `tokio::spawn` (thread pool), results routed to scheduler inbox
- [ ] Port cleanup logic (shutdown servers, close sockets on `Runtime::shutdown()`)

**Deliverable:** Net (4) + HTTP (5) = **9 tests** pass.

### Phase 8: LSP (Weeks 13-14)

**Goal:** Language server with diagnostics, hover, definition, completion.

- [ ] Set up `tower-lsp` server
- [ ] Port analyzer (lex → parse → typecheck pipeline)
- [ ] Port symbol collection and position-based lookup
- [ ] Port hover handler (type information)
- [ ] Port go-to-definition handler
- [ ] Port completion handler
- [ ] Port diagnostics publishing

**Deliverable:** `cargo test -p marlang-lsp` passes editor-ops (25) tests.

### Phase 9: CLI & Polish (Weeks 14-16)

**Goal:** Standalone binary with all commands working.

- [ ] Implement `clap` CLI with all subcommands (run, repl, typecheck, disasm, lsp)
- [ ] Implement `rustyline` REPL with history
- [ ] Wire up Tokio runtime for async commands
- [ ] Cross-compile and test on Linux, macOS, Windows
- [ ] Performance benchmarks: verify targets (5× fib40, <10 ms startup, 3× less memory)
- [ ] Binary size optimization (`strip`, LTO, `opt-level = "z"` if needed)
- [ ] Update CLAUDE.md and documentation

**Deliverable:** `mar run file.mar`, `mar repl`, `mar typecheck file.mar`, `mar disasm file.mar`, `mar lsp` — all working as a single ~5-10 MB binary.

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
default = ["native"]
native = ["tokio", "reqwest", "hyper", "crossbeam-deque", "crossbeam-channel", "num_cpus", "fastrand"]
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
marlang-vm = { path = "../vm", default-features = false }
marlang-modules = { path = "../modules" }
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

# Modules
# (no external deps beyond std::fs)

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
2. **`bun:ffi` dlopen** — replaced by Rust function registration API. `libloading`-based dlopen deferred to Phase 2.
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
