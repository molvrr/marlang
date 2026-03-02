# Known Issues — Rust Port

**Last updated:** 2026-03-02

Comprehensive catalog of known bugs, limitations, workarounds, and architectural debt in the Rust port (`port/`). Issues are ordered by severity. Issues #2, #3, and #4 are fixed.

---

## Table of Contents

1. [Runtime Performance Regression](#1-runtime-performance-regression)
2. [~~Module Globals Workaround — Flat Namespace Collisions~~](#2-module-globals-workaround--flat-namespace-collisions) **[FIXED]**
3. [~~Type Checker Rejects `net.mar`~~](#3-type-checker-rejects-netmar) **[FIXED]**
4. [~~`handle`/`throw` Unreliable Across Scheduler Effects~~](#4-handlethrow-unreliable-across-scheduler-effects) **[FIXED]**
5. [Recursive `define` + TCP Effects Hangs](#5-recursive-define--tcp-effects-hangs)
6. [Cross-Scheduler TCP Not Supported](#6-cross-scheduler-tcp-not-supported)
7. [`match` Branches Require Single Expressions](#7-match-branches-require-single-expressions)
8. [Hygiene Mark Fallback Is Pervasive](#8-hygiene-mark-fallback-is-pervasive)
9. [Silent Error Swallowing in Macro Expansion](#9-silent-error-swallowing-in-macro-expansion)
10. [TCP Unavailable in Single-Threaded Scheduler](#10-tcp-unavailable-in-single-threaded-scheduler)
11. [Sentinel `u64::MAX` for TCP Type Disambiguation](#11-sentinel-u64max-for-tcp-type-disambiguation)
12. [`PopHandler` Opcode Is a No-Op](#12-pophandler-opcode-is-a-no-op)
13. [Type Inference Fallbacks for Unknown Structs](#13-type-inference-fallbacks-for-unknown-structs)
14. [Dead Code (`#[allow(dead_code)]`)](#14-dead-code-allowdead_code)
15. [Per-Instruction Allocation in `PushConst`](#15-per-instruction-allocation-in-pushconst)
16. [Closure Clone on Every Function Call](#16-closure-clone-on-every-function-call)
17. [Mutex `unwrap()` in Hot Paths](#17-mutex-unwrap-in-hot-paths)
18. [LSP Not Implemented](#18-lsp-not-implemented)
19. [WASM & Web Editor Not Implemented](#19-wasm--web-editor-not-implemented)

---

## Critical / High Impact

### 1. Runtime Performance Regression

**Severity:** Critical
**Status:** Open
**Locations:** `vm.rs:612` (run loop), `vm.rs:646` (PushConst), `vm.rs:982` (Call)

The Rust port is currently ~5x **slower** than the TypeScript VM, the opposite of the spec's 5x improvement target.

| Benchmark | TypeScript (Bun) | Rust | Expected |
|-----------|-----------------|------|----------|
| `fib(35)` | ~3.5s | ~18s | < 0.7s |

**Likely causes:**

1. **Per-instruction constant allocation** — The `PushConst` opcode (`vm.rs:646`) calls `ConstantValue::as_value()` on every execution, which allocates a new `Arc<VMObject>` for string constants. A `preconvert_constants()` function exists in `value.rs:327` that pre-converts an entire constant pool to NaN-boxed `Value`s, but it is only used in tests (`value.rs:420`). Constants should be pre-converted per-chunk and cached so `PushConst` becomes a simple array index.

2. **Closure clone on every `Op::Call`** — At `vm.rs:1010`, every function call does `(**closure).clone()`, which copies the upvalue `Vec<Value>` (each element is a NaN-boxed `Value` — cheap for primitives, but triggers `Arc` refcount bumps for object values). For closures with many captured variables in hot loops, this is a per-call allocation. See also Issue #16.

3. **`Arc` refcounting overhead** — Every `Value::clone()` for object values increments/decrements an atomic refcount. Hot loops with closures pay this cost on every call.

**Note on frame access:** The run loop previously used a clone-and-save pattern for `CallFrame` access. This has been replaced with unsafe raw pointers cached via the `cache_frame!` macro (`vm.rs:629-636`), which reads `code`, `constants`, and `locals` pointers directly from `vm.frames[frame_idx]`. This eliminated the per-instruction frame copy overhead, but the constant allocation and closure clone issues remain.

**Workaround:** None. Programs run correctly, just slowly.

---

### 2. ~~Module Globals Workaround — Flat Namespace Collisions~~ [FIXED]

**Severity:** ~~Critical~~ Resolved
**Status:** **FIXED** — module-scoped globals implemented

The VM previously had a single flat global namespace. When two modules defined the same name and were imported with qualifiers, the first-imported module silently won for unqualified name collisions.

#### Resolution

Replaced the flat `vm.globals` with per-module globals tables:

1. **`ModuleId` type** (`bytecode.rs`): `u32` index into the VM's `module_globals` table. `ROOT_MODULE = 0` for builtins + prelude + main program.

2. **Per-module globals** (`vm.rs`): `vm.module_globals: Vec<FxHashMap<String, Value>>` indexed by `ModuleId`. `vm.current_module` tracks which module is being executed.

3. **`module_id` on `FunctionPrototype` and `Closure`** (`bytecode.rs`, `object.rs`): Every compiled function carries its home module ID. Closures copy it from the prototype at creation for fast access.

4. **`PushGlobal` resolution** (`vm.rs`): Looks up `module_globals[closure.module_id]` first, then falls back to `module_globals[ROOT_MODULE]`. Module closures always resolve their own globals even when called from a different module.

5. **`SetGlobal`** (`vm.rs`): Writes to `module_globals[vm.current_module]`.

6. **`merge_exports` simplified** (`modules.rs`): Qualified import branch only installs `"qualifier/name"` in the importing module's table. The old unqualified fallback (first-imported-wins) is removed.

7. **`handle_import` refactored** (`modules.rs`): Allocates a new `ModuleId` for each module, switches `vm.current_module` during module execution, and restores it after. No more `std::mem::take` of globals.

8. **Compiler propagation** (`compiler.rs`): All `Compiler::new` and `FunctionPrototype` constructors accept and propagate `module_id`.

**Tests added:** `module_qualified_no_namespace_collision` (two modules with same name, independently accessible), `module_qualified_internal_cross_reference` (module functions calling each other), `module_qualified_no_unqualified_leak` (unqualified names not visible after qualified import).

---

### 3. ~~Type Checker Rejects `net.mar` / `tcp.mar`~~ [FIXED]

**Severity:** ~~Critical~~ Resolved
**Status:** **FIXED** — Full-stack variadic function support + Option-wrapped TCP returns
**Locations:**
- `stdlib/net.mar:62` — 3-argument `string-append` call
- `port/crates/types/src/builtins.rs:311-315` — `string-append` typed as binary
- `port/crates/vm/src/builtins.rs:183-192` — `string-append` runtime is variadic
- `port/crates/types/src/infer.rs:1403-1461` — `infer_application` (builds expected fn type)
- `port/crates/types/src/unify.rs:112-133` — Arrow unification (param count check)
- `port/crates/vm/src/runtime.rs` — TCP accept/recv returned raw `nil` instead of `Option` ADT

#### Misdiagnosis

The CLAUDE.md known issue states:

> Running with type checking fails on parameterized types like `(Result String TcpSocket)` in effect signatures.

**This is wrong.** Parameterized types in effect signatures work correctly. The expression `(Result String TcpSocket)` is correctly parsed as `Type::App { constructor: Const("Result"), args: [Const("String"), Const("TcpSocket")] }` by `parse_type_expr` in `infer.rs:175-184`.

#### Actual Root Cause

The error comes from `net.mar:62`:

```lisp
(define unwrap-or-throw (lambda (label result)
  (match result
    ((Ok v) v)
    ((Err msg) (perform throw (string-append label ": " msg))))))
```

This calls `(string-append label ": " msg)` with **3 arguments**. The type system declares `string-append` as `(-> String String String)` (binary), but the runtime implementation is variadic.

**Error path:**
1. `infer_application` infers `string-append` as `(-> String String String)` — 2 params
2. It builds `expected_fn_type` as `(-> t0 t1 t2 t3)` — 3 params from the 3 arguments
3. `unify_with_context` tries to unify these arrow types
4. `unify_internal` at `unify.rs:126-132` detects the param count mismatch:
   ```
   ArityMismatch { expected: 2, got: 3, context: "function type" }
   ```
5. `format_error` produces: `"function type: expected 2 arguments, got 3"`

#### Resolution

Full-stack variadic function support was implemented, going well beyond the minimal fix:

1. **Type AST** (`ast.rs`): Added `rest: Option<Box<Type>>` field to `Type::Arrow`, representing the element type for variadic rest parameters. New helpers: `type_arrow_variadic`, `type_arrow_variadic_with_effects`.

2. **Unification** (`unify.rs`): Arrow unification handles all `rest`/non-`rest` combinations. When a variadic arrow is unified with a non-variadic arrow, excess params unify against the rest element type.

3. **Type inference** (`infer.rs`): Parses `(... T)` syntax in arrow type annotations. Detects `.` dot notation in lambda param lists to build variadic arrow types. Rest param gets type `(List T)` in the function body. Removed `list` from `FFI_VARIADIC_FUNCTIONS` — it now uses the general variadic type mechanism.

4. **Builtins** (`builtins.rs`): `string-append` and `list` registered with proper variadic types: `(-> (... String) String)` and `(-> (... a) (List a))`. (`string-concat` was later removed as a redundant alias for `string-append`.)

5. **Compiler** (`compiler.rs`): `compile_lambda` detects `.` in parameter list, sets `is_variadic: true` on `FunctionPrototype`, and gives the rest param a local slot.

6. **VM** (`vm.rs`): `CALL` and `TAIL_CALL` handlers pack excess arguments into a `VMObject::List` for the rest-param local slot when calling a variadic closure.

7. **Bytecode** (`bytecode.rs`): Added `is_variadic: bool` to `FunctionPrototype`.

8. **Disassembler** (`debug.rs`): Shows `+` suffix for variadic functions (e.g., `<fn <lambda>/1+>`).

**User syntax:**
```lisp
(lambda (a b . rest) body)          ; dot notation for rest params
(-> String (... String) String)     ; variadic type annotation
```

**Tests added:** 10 type system tests, 11 VM tests covering variadic builtins, user-defined variadic lambdas, tail calls, arity errors, and type checking.

#### Phase 2: Option-Wrapped TCP Returns

The second issue preventing `tcp.mar` from type-checking: `tcp-accept` and `tcp-recv` returned raw `nil` at runtime but were typed as returning `TcpSocket` and `String` respectively. Code like `(= client nil)` forced `client : Unit` because `nil` is typed as `Unit` and `=` requires both args to be the same type.

**Fix:**

1. **Runtime** (`runtime.rs`): `tcp-accept` now returns `Some(socket)` or `None` (ADT values) instead of raw `Port` or `nil`. Same for `tcp-recv`: returns `Some(data)` or `None`. Added `make_some()`/`make_none()` helpers alongside existing `make_ok()`/`make_err()`.

2. **Effect signatures** (`net.mar`): `tcp-accept` changed to `(-> TcpListener (Option TcpSocket))`, `tcp-recv` changed to `(-> TcpSocket (Option String))`. Added `tcp-close-listener` effect for type-safe listener close (previously `tcp-close` handled both sockets and listeners but was typed only for sockets).

3. **Wrappers** (`net.mar`): `accept` and `recv` wrappers now return `(Option a)`. Added `close-listener` wrapper for `tcp-close-listener`.

4. **Examples** (`tcp.mar`): Replaced `(= client nil)` with `(match (net/accept listener) (None ...) ((Some client) ...))` pattern matching. Uses `net/close-listener` for listener close.

5. **Tests** (`tests.rs`): All 6 TCP tests updated to unwrap `Some`/match `None` from `tcp-accept` and `tcp-recv`.

Both `net.mar` and `tcp.mar` now type check successfully and run correctly with and without `--no-typecheck`.

---

### 4. ~~`handle`/`throw` Unreliable Across Scheduler Effects~~ [FIXED]

**Severity:** ~~Critical~~ Resolved
**Status:** **FIXED** — CPS handler dispatch (see `specs/cps-handler-dispatch.md`)

The old architecture used Rust-stack-recursive `run_with_handler` for deep handler semantics. When a scheduler-suspending effect fired inside a handler clause, the Rust stack was unwound but VM state survived, causing the handler context to be lost on resume.

**Resolution:** Handler dispatch is now entirely in VM state via `HandlerDispatch` markers — no Rust stack involvement. `Op::PushHandler` installs the handler frame + dispatch marker and calls the body thunk inline. `Op::Perform` for user handlers captures the continuation, unwinds, re-installs the handler (deep semantics), and calls the clause inline. `Op::Return` detects dispatch markers and applies return handlers. `Op::Resume` was simplified to inline continuation restore.

Additionally, `throw` was removed from the scheduler's `HANDLED_EFFECTS` — unhandled `throw` now gives "Uncaught exception: ..." instead of going through the scheduler. The deleted functions `run_with_handler` and `run_restored_continuation` are no longer needed.

---

### 5. Recursive `define` + TCP Effects Hangs

**Severity:** High
**Status:** Open
**Locations:** `vm.rs:669` (`PushGlobal`), `runtime.rs` (fiber resume after TCP suspend)

Using `(define f (lambda () ... (f)))` inside `fiber-spawn` where the recursive body performs TCP effects (accept, recv, send, close) causes the program to hang. The `define` sets a global on the scheduler thread's VM, but after a TCP effect suspends/resumes the fiber, `PUSH_GLOBAL "f"` may not resolve correctly.

**Related to:** Issue #2's flat globals root cause is now fixed (module-scoped globals), but this hang may persist due to globals being cloned per-thread — a `define` on one scheduler thread is invisible to others after work stealing. Issue #4 (handler state lost across scheduler suspension) is also fixed, so the remaining cause is likely the per-thread globals snapshot divergence.

**Workaround:** Unroll the recursion, or avoid combining recursive `define` with scheduler-suspending effects.

---

## Medium Impact

### 6. Cross-Scheduler TCP Not Supported

**Severity:** Medium
**Status:** Open — by design, deferred
**Locations:** `runtime.rs:1902-1934`

When a fiber on scheduler A operates on a TCP socket owned by scheduler B, `tcp-recv` and `tcp-accept` return explicit errors:

```
"tcp-recv: cross-scheduler recv not yet supported (no data available)"
"tcp-accept: cross-scheduler accept not yet supported"
```

In practice, sockets are used by the fiber that created them (same scheduler). Cross-scheduler socket operations are rare. The `TcpOp` enum (`runtime.rs:136-154`) and `TcpOpResult` message type exist in the codebase for a future implementation, but are marked `#[allow(dead_code)]` and non-functional.

**Workaround:** Ensure TCP operations happen on the same fiber that created the socket.

---

### 7. `match` Branches Require Single Expressions

**Severity:** Medium
**Status:** By design — but common source of user confusion
**Location:** `compiler.rs:818-822` (`compile_match` checks `branch_elements.len() != 2`)

Match branches must be `(pattern body)` where body is a single expression. Multi-expression bodies need explicit `(begin ...)` wrapping:

```lisp
;; Error:
(match x
  ((Some v) (println v) v))

;; Correct:
(match x
  ((Some v) (begin (println v) v)))
```

**Workaround:** Always wrap multi-expression match bodies in `(begin ...)`.

---

### 8. Hygiene Mark Fallback Is Pervasive

**Severity:** Medium
**Status:** Open — suggests incomplete hygiene propagation
**Locations:**
- `vm.rs:679-680` — `PUSH_GLOBAL`: strip marks and retry
- `expand.rs:218-226` — Macro name lookup: strip marks and retry
- `compiler.rs:407-417` — Constructor lookup (`is_known_constructor`): strip marks and retry
- `compiler.rs:421-428` — Constructor name resolution (`resolve_constructor_name`): strip marks
- `types/infer.rs:390-398` — Type inference variable lookup: strip marks and retry
- `types/infer.rs:1128-1140` — Type inference constructor pattern lookup: strip marks and retry

Six different locations across four subsystems implement the same "strip `~N` hygiene marks and retry" fallback pattern. This suggests the macro hygiene system does not fully propagate marked names through all compilation phases. Each fallback independently calls `strip_all_marks()` from `expand.rs:697-708`.

**Impact:** Works correctly for current programs but could mask hygiene violations. The proper fix would be to ensure marked names are consistently resolved during macro expansion so downstream phases never see them.

---

### 9. Silent Error Swallowing in Macro Expansion

**Severity:** Medium
**Status:** Open
**Location:** `expand.rs:412-473` (`expand_import` function)

Multiple failure points during macro expansion from imports are silently swallowed with `return Ok(())`:

- Line 415: Import with fewer than 2 elements — skipped
- Line 420: Path is not a string literal — skipped
- Line 445: Import path resolution failure — skipped
- Line 457: Circular dependency — skipped
- Line 463: Source file loading failure — skipped
- Line 469: Lexing failure — skipped
- Line 473: Parse failure — skipped

All failures defer to runtime with the comment pattern: `"let runtime handle"`. This can mask bugs during development — a typo in an import path would silently produce no macros rather than an error.

---

## Low Impact / Tech Debt

### 10. TCP Unavailable in Single-Threaded Scheduler

**Severity:** Low (by design)
**Status:** Won't fix — architectural decision
**Location:** `scheduler.rs:691-696`

All 7 TCP effects (including `tcp-close-listener`) return explicit errors in the single-threaded scheduler:

```
"TCP effects require the multi-threaded scheduler (run-fibers-parallel)"
```

TCP requires Tokio's async runtime, which is only available in the multi-threaded scheduler (`runtime.rs`).

---

### 11. Sentinel `u64::MAX` for TCP Type Disambiguation

**Severity:** Low (tech debt)
**Status:** Open
**Locations:** `runtime.rs:982` (set), `runtime.rs:1822` (check)

Uses `listener_id: u64::MAX` as a sentinel value to distinguish `tcp-connect` (which creates a socket directly) from `tcp-accept` (which receives a socket from a listener's accept queue) in `TcpAccepted` messages:

```rust
// sentinel: this is a connect, not accept
listener_id: u64::MAX,
```

Should use a proper enum variant instead of a magic number.

---

### 12. ~~`PopHandler` Opcode Is a No-Op~~ [FIXED]

**Severity:** ~~Low~~ Resolved
**Status:** **FIXED** — `PopHandler` now pops handler frames and dispatch markers as part of CPS handler dispatch (Issue #4 fix).

---

### 13. Type Inference Fallbacks for Unknown Structs

**Severity:** Low
**Status:** Open
**Location:** `types/infer.rs:1444-1447`

When FFI struct definitions are not found during type inference, the type system silently falls back to fresh type variables instead of producing an error:

- Line 1444: `"unknown struct def → polymorphic fallback"`
- Line 1447: `"can't determine struct type → polymorphic fallback"`

This means struct-related type errors are never caught at type-check time.

---

### 14. Dead Code (`#[allow(dead_code)]`)

**Severity:** Low (tech debt)
**Status:** Open

Several items are declared but not actively used:

| File | Line | Item | Note |
|------|------|------|------|
| `vm.rs` | 304 | `resume_continuation()` | "Used by future scheduler (Phase 5)" |
| `runtime.rs` | 53 | `SchedulerMessage` enum | Some variants unused in all paths |
| `runtime.rs` | 135 | `TcpOp` enum | Cross-scheduler TCP ops (unsupported) |
| `runtime.rs` | 219 | `steal_worker` field | Work stealing worker not actively used |
| `expand.rs` | 52 | `quasiquote_sym` field | Stored but not used in expand logic |
| `compiler.rs` | 180 | `function_name` field | Stored but not read back |
| `tests.rs` | 1617 | `eval_modules_with_output()` | Test helper not called |

---

### 15. Per-Instruction Allocation in `PushConst`

**Severity:** Low (performance, related to Issue #1)
**Status:** Open
**Location:** `vm.rs:646`

The `PushConst` opcode calls `ConstantValue::as_value()` on every execution. For string constants, this allocates a new `Arc<VMObject::String>` each time. Numeric and boolean constants are cheap (NaN-boxed inline), but string-heavy code pays an allocation per string push.

A `preconvert_constants()` function exists in `value.rs:327` that batch-converts an entire `&[ConstantValue]` to `Vec<Value>`, but it is only used in a test (`value.rs:420`). The fix is to call this once per chunk (or per frame switch) and index into the cached `Vec<Value>` in the hot loop instead of calling `as_value()`.

---

### 16. Closure Clone on Every Function Call

**Severity:** Low (performance, related to Issue #1)
**Status:** Open
**Locations:** `vm.rs:982` (`Op::Call`), `vm.rs:1154` (`Op::TailCall`)

Every `Op::Call` does `(**closure).clone()` at `vm.rs:1010`, which copies the `Closure` struct including its `Vec<Value>` of upvalues. For closures with no upvalues, the Vec clone is trivial (empty Vec). For closures with captured variables, each `Value` element is cloned — primitives are cheap (NaN-boxed), but object values increment an `Arc` refcount.

`Op::TailCall` (`vm.rs:1183`) similarly clones the closure but reuses the existing frame, avoiding a new `CallFrame` allocation.

For hot recursive closures (e.g., `fib`), this is a significant per-call cost. A potential fix is to use `Arc<Closure>` and share the prototype, only cloning upvalues when mutation is needed.

---

### 17. Mutex `unwrap()` in Hot Paths

**Severity:** Low (robustness)
**Status:** Open
**Locations:** `vm.rs:1106`, `vm.rs:1282`

The `STRUCT_REGISTRY` global mutex is accessed with `.lock().unwrap()` at two points in the `Op::Call` and `Op::TailCall` handlers (FFI struct call path). If the mutex is poisoned (e.g., a panic occurred in another thread while holding the lock), these `unwrap()` calls will panic rather than returning a recoverable error.

In practice, `STRUCT_REGISTRY` is only written during FFI setup and read during FFI calls, so poisoning is unlikely. But the pattern is not consistent with the VM's otherwise error-returning design.

---

### 18. LSP Not Implemented

**Severity:** Low (feature gap, not a bug)
**Status:** Phase 9 — not started
**Spec:** `specs/rust-port.md` Section 11

The `tower-lsp` server, analyzer, hover, definition, completion, and diagnostics are all pending. The TypeScript LSP still works for development.

---

### 19. WASM & Web Editor Not Implemented

**Severity:** Low (feature gap, not a bug)
**Status:** Phase 10 — not started
**Spec:** `specs/rust-port.md` Section 15

Tagged enum `Value` fallback for 32-bit WASM, `wasm-bindgen` exports, and web editor migration from TypeScript imports to WASM module are all pending.
