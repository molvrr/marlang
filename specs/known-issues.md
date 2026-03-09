# Known Issues — Rust Port

**Last updated:** 2026-03-03

Comprehensive catalog of known bugs, limitations, workarounds, and architectural debt in the Rust port (`port/`). Issues are ordered by severity.

---

## Table of Contents

1. [Runtime Performance Regression](#1-runtime-performance-regression)
2. [Per-Thread Globals Divergence with `define` in Nested Scopes](#2-per-thread-globals-divergence-with-define-in-nested-scopes)
3. [Cross-Scheduler TCP Not Supported](#3-cross-scheduler-tcp-not-supported)
4. [`match` Branches Require Single Expressions](#4-match-branches-require-single-expressions)
5. [Hygiene Mark Fallback Is Pervasive](#5-hygiene-mark-fallback-is-pervasive)
6. [Silent Error Swallowing in Macro Expansion](#6-silent-error-swallowing-in-macro-expansion)
7. [TCP Unavailable in Single-Threaded Scheduler](#7-tcp-unavailable-in-single-threaded-scheduler)
8. [Sentinel `u64::MAX` for TCP Type Disambiguation](#8-sentinel-u64max-for-tcp-type-disambiguation)
9. [Type Inference Fallbacks for Unknown Structs](#9-type-inference-fallbacks-for-unknown-structs)
10. [Dead Code (`#[allow(dead_code)]`)](#10-dead-code-allowdead_code)
11. [Per-Instruction Allocation in `PushConst`](#11-per-instruction-allocation-in-pushconst)
12. [Closure Clone on Every Function Call](#12-closure-clone-on-every-function-call)
13. [Mutex `unwrap()` in Hot Paths](#13-mutex-unwrap-in-hot-paths)
14. [LSP Not Implemented](#14-lsp-not-implemented)
15. [WASM & Web Editor Not Implemented](#15-wasm--web-editor-not-implemented)

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

2. **Closure clone on every `Op::Call`** — At `vm.rs:1010`, every function call does `(**closure).clone()`, which copies the upvalue `Vec<Value>` (each element is a NaN-boxed `Value` — cheap for primitives, but triggers `Arc` refcount bumps for object values). For closures with many captured variables in hot loops, this is a per-call allocation. See also Issue #12.

3. **`Arc` refcounting overhead** — Every `Value::clone()` for object values increments/decrements an atomic refcount. Hot loops with closures pay this cost on every call.

**Note on frame access:** The run loop previously used a clone-and-save pattern for `CallFrame` access. This has been replaced with unsafe raw pointers cached via the `cache_frame!` macro (`vm.rs:629-636`), which reads `code`, `constants`, and `locals` pointers directly from `vm.frames[frame_idx]`. This eliminated the per-instruction frame copy overhead, but the constant allocation and closure clone issues remain.

**Workaround:** None. Programs run correctly, just slowly.

---

### 2. Per-Thread Globals Divergence with `define` in Nested Scopes

**Severity:** Medium (downgraded from High — primary case resolved)
**Status:** Mostly fixed — narrow edge case remains
**Locations:** `runtime.rs:2486-2513` (per-thread globals clone), `compiler.rs:747` (nested-scope define fallthrough)

The original issue — recursive `(define f (lambda () ... (f)))` inside `fiber-spawn` with TCP effects causing hangs — is **resolved**. Three fixes addressed the root causes:

1. **Module-scoped globals** (`dc69208`) eliminated flat namespace collisions.
2. **Handler state preservation** across scheduler suspension is fixed.
3. **Letrec local defines** (`ffc2c7b`) — leading `define` forms in lambda bodies now compile as locals with shared upvalue cells (`Arc<Mutex<Value>>`). Recursive calls resolve via `PushUpvalue`, not `PushGlobal`, so per-thread globals divergence is irrelevant for this pattern.

**Remaining edge case:** Each scheduler thread still gets an independent `module_globals.clone()` at startup (`runtime.rs:2486`). A `SetGlobal` on one thread is invisible to others. This can manifest if:

- A `define` occurs in a **nested scope** (e.g., inside `let`, where `scope_depth > 0`) or in a **handler-clause lambda** (where `prescan_leading_defines` is skipped) — these still emit `SetGlobal` instead of `SetLocal`.
- A fiber referencing that global gets spawned to a different scheduler thread via round-robin (`pick_scheduler`).

Note: TCP suspend/resume sends the fiber back to the **same** scheduler thread, so TCP-specific hangs should not occur. The edge case only applies to cross-scheduler fiber spawning with nested-scope defines.

**Workaround:** Use leading `define` forms in lambda bodies (which get the letrec treatment), or avoid `define` inside nested scopes like `let` when fibers may be distributed across scheduler threads.

---

## Medium Impact

### 3. Cross-Scheduler TCP Not Supported

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

### 4. `match` Branches Require Single Expressions

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

### 5. Hygiene Mark Fallback Is Pervasive

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

### 6. Silent Error Swallowing in Macro Expansion

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

### 7. ~~TCP Unavailable in Single-Threaded Scheduler~~ (Resolved)

**Status:** Resolved — the always-on scheduler architecture removed the distinction between single-threaded and multi-threaded schedulers. All effects (IO, TCP, HTTP, fibers, channels, actors) now work at the top level through a single unified scheduler with a built-in tokio runtime.

---

### 8. Sentinel `u64::MAX` for TCP Type Disambiguation

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

### 9. Type Inference Fallbacks for Unknown Structs

**Severity:** Low
**Status:** Open
**Location:** `types/infer.rs:1444-1447`

When FFI struct definitions are not found during type inference, the type system silently falls back to fresh type variables instead of producing an error:

- Line 1444: `"unknown struct def → polymorphic fallback"`
- Line 1447: `"can't determine struct type → polymorphic fallback"`

This means struct-related type errors are never caught at type-check time.

---

### 10. Dead Code (`#[allow(dead_code)]`)

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

### 11. Per-Instruction Allocation in `PushConst`

**Severity:** Low (performance, related to Issue #1)
**Status:** Open
**Location:** `vm.rs:646`

The `PushConst` opcode calls `ConstantValue::as_value()` on every execution. For string constants, this allocates a new `Arc<VMObject::String>` each time. Numeric and boolean constants are cheap (NaN-boxed inline), but string-heavy code pays an allocation per string push.

A `preconvert_constants()` function exists in `value.rs:327` that batch-converts an entire `&[ConstantValue]` to `Vec<Value>`, but it is only used in a test (`value.rs:420`). The fix is to call this once per chunk (or per frame switch) and index into the cached `Vec<Value>` in the hot loop instead of calling `as_value()`.

---

### 12. Closure Clone on Every Function Call

**Severity:** Low (performance, related to Issue #1)
**Status:** Open
**Locations:** `vm.rs:982` (`Op::Call`), `vm.rs:1154` (`Op::TailCall`)

Every `Op::Call` does `(**closure).clone()` at `vm.rs:1010`, which copies the `Closure` struct including its `Vec<Value>` of upvalues. For closures with no upvalues, the Vec clone is trivial (empty Vec). For closures with captured variables, each `Value` element is cloned — primitives are cheap (NaN-boxed), but object values increment an `Arc` refcount.

`Op::TailCall` (`vm.rs:1183`) similarly clones the closure but reuses the existing frame, avoiding a new `CallFrame` allocation.

For hot recursive closures (e.g., `fib`), this is a significant per-call cost. A potential fix is to use `Arc<Closure>` and share the prototype, only cloning upvalues when mutation is needed.

---

### 13. Mutex `unwrap()` in Hot Paths

**Severity:** Low (robustness)
**Status:** Open
**Locations:** `vm.rs:1106`, `vm.rs:1282`

The `STRUCT_REGISTRY` global mutex is accessed with `.lock().unwrap()` at two points in the `Op::Call` and `Op::TailCall` handlers (FFI struct call path). If the mutex is poisoned (e.g., a panic occurred in another thread while holding the lock), these `unwrap()` calls will panic rather than returning a recoverable error.

In practice, `STRUCT_REGISTRY` is only written during FFI setup and read during FFI calls, so poisoning is unlikely. But the pattern is not consistent with the VM's otherwise error-returning design.

---

### 14. LSP Not Implemented

**Severity:** Low (feature gap, not a bug)
**Status:** Phase 9 — not started
**Spec:** `specs/rust-port.md` Section 11

The `tower-lsp` server, analyzer, hover, definition, completion, and diagnostics are all pending. The TypeScript LSP still works for development.

---

### 15. WASM & Web Editor Not Implemented

**Severity:** Low (feature gap, not a bug)
**Status:** Phase 10 — not started
**Spec:** `specs/rust-port.md` Section 15

Tagged enum `Value` fallback for 32-bit WASM, `wasm-bindgen` exports, and web editor migration from TypeScript imports to WASM module are all pending.
