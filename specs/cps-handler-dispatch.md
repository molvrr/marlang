# CPS Handler Dispatch — Implementation Spec

**Status:** Implemented
**Date:** 2026-03-01
**Fixes:** Known Issue #4 (`handle`/`throw` unreliable across scheduler effects)
**Effort:** XL (5-7 days)

---

## Summary

Replace the Rust-stack-recursive `run_with_handler` mechanism with compiler-emitted trampoline closures that encode handler dispatch entirely in VM state. When an effect is performed, the handler clause is invoked as a normal VM closure call — not via Rust recursion. Deep handler semantics (re-installation of the handler around the clause body) are achieved by wrapping the handler clause in a compiler-generated trampoline that re-installs the handler frame before executing.

This eliminates the dual-state problem (Rust call stack vs VM handler frames) that causes Issue #4: when a scheduler-suspending effect fires inside a handler clause, the Rust stack is unwound but the VM state survives. With CPS dispatch, there is no Rust-level handler state to lose.

Additionally, `throw` is removed from the scheduler's `HANDLED_EFFECTS` list (Approach C), so unhandled `throw` uses `try_default_handler` to crash the fiber rather than going through the scheduler's dummy handler.

---

## Table of Contents

1. [Problem Statement](#1-problem-statement)
2. [Design Overview](#2-design-overview)
3. [Compiler Changes](#3-compiler-changes)
4. [New Opcodes](#4-new-opcodes)
5. [VM Changes](#5-vm-changes)
6. [Object Model Changes](#6-object-model-changes)
7. [Scheduler Changes](#7-scheduler-changes)
8. [Approach C: Remove `throw` from Scheduler](#8-approach-c-remove-throw-from-scheduler)
9. [Scope & Deliverables](#9-scope--deliverables)
10. [Non-Goals](#10-non-goals)
11. [Acceptance Criteria](#11-acceptance-criteria)
12. [Test Strategy](#12-test-strategy)
13. [Risks & Mitigations](#13-risks--mitigations)
14. [Trade-offs](#14-trade-offs)

---

## 1. Problem Statement

**Who:** Any Marlang program that uses `handle`/`throw` alongside scheduler-suspending effects (TCP, channels, IO in multi-threaded mode).

**What:** User `handle` blocks lose their handler dispatch context when a scheduler-suspending effect fires inside the handler clause body. The Rust call stack (which encodes deep handler semantics via `run_with_handler` recursion) is unwound by the scheduler, but the VM state (handler frames, stack, call frames) is captured in a continuation. On resume, the VM state is restored but the Rust-level handler dispatch context is not. Return handlers are not applied. Handler clause return values go to the wrong place.

**Why it matters:** Algebraic effects are a core feature. `handle`/`throw` is the primary exception mechanism. It being unreliable when combined with IO/networking makes the effect system untrustworthy for real programs.

**Root cause:** Handler dispatch uses two representations of the same state:
1. Rust call stack: `run_with_handler` → `run_with_handler` (recursive for deep semantics)
2. VM handler stack: `Vec<VMHandlerFrame>`

Scheduler suspension destroys (1) but preserves (2). On resume, (2) is restored but (1) is not.

---

## 2. Design Overview

### Current architecture

```
(handle body ((op args k) -> clause-body))
       |
       v
  [Op::PushHandler] — parses descriptor, calls run_with_handler()
       |
       v
  [run_with_handler] — installs VMHandlerFrame, calls call_closure(body)
       |                                          |
       |   body does (perform op arg)             v
       |                                     [Op::Perform]
       |                                          |
       |   returns Err(VMSignal::Effect)          |
       |<─────────────────────────────────────────┘
       |
       v
  [run_with_handler match arm] — finds clause, recursively calls
       |                          run_with_handler(clause, [args, k])
       v
  [run_with_handler] — installs SAME handler, calls call_closure(clause)
       |                 (this is deep handler semantics)
       ...
```

The Rust call stack holds the "re-wrap with handler" semantics. Scheduler suspension destroys this.

### New architecture

```
(handle body ((op args k) -> clause-body))
       |
       v
  [Op::PushHandler] — installs VMHandlerFrame with handler entries
       |               (including pre-compiled trampoline closures)
       v
  [Op::Call] — calls body thunk as a normal closure
       |
       v
  body does (perform op arg)
       |
       v
  [Op::Perform] — walks handler stack, finds handler
       |           captures continuation
       |           DOES NOT return Err(VMSignal::Effect) for user handlers
       |           instead: unwinds to handler's frame, calls trampoline
       |
       v
  [trampoline closure] — re-installs the same handler frame
       |                   calls the handler clause with [args, k]
       |                   on return, applies return handler if present
       v
  [handler clause runs as normal VM code]
       |
       v
  clause does (resume k value)
       |
       v
  [Op::Resume] — restores continuation, pushes resume value
       |           if resumed body re-performs → caught by re-installed handler
       |           (which is on the VM handler stack, not the Rust stack)
       v
  execution continues in restored continuation
```

Key insight: the trampoline closure is a compiler-generated function that:
1. Receives the effect args and continuation as parameters
2. Re-installs the handler frame (deep semantics)
3. Calls the user's handler clause closure
4. On return from the clause, optionally applies the return handler

Because the trampoline is a real VM closure, its execution state is captured in continuations. When a scheduler effect fires inside the handler clause, the entire handler dispatch context is in the VM state and survives suspension.

### EffectSignal still exists — for the scheduler

User-level `handle` blocks are dispatched entirely within the VM via the trampoline mechanism. The `EffectSignal` / `VMSignal::Effect` propagation path is preserved **only** for effects that hit the scheduler's dummy handler. The scheduler installs its dummy handler frame with real handler entries, and when PERFORM matches it, the signal still propagates to the scheduler loop via `Err(VMSignal::Effect(...))`.

This is the clean separation:
- **User handlers:** Dispatched inline in the VM via trampoline closures. No Rust stack involvement.
- **Scheduler handlers:** Propagated via EffectSignal to the scheduler loop. No change from current behavior.

The distinction is encoded in the handler frame: user handler frames have `is_scheduler: false` (or don't have the flag), scheduler handler frames have `is_scheduler: true`.

---

## 3. Compiler Changes

### 3.1 New compilation target for `compile_handle`

Currently `compile_handle` emits a single `PushHandler` opcode with a JSON descriptor string. The runtime (`Op::PushHandler` handler) parses this descriptor, creates closures from prototypes, and calls `run_with_handler`.

**New approach:** The compiler emits a sequence of opcodes that:
1. Create the handler entries (closures for each effect clause)
2. Create a **dispatch trampoline** for each effect clause
3. Install the handler frame
4. Call the body thunk
5. On normal return, apply the return handler if present
6. Remove the handler frame

### 3.2 Dispatch trampoline

For each effect clause `((op arg1 ... argN k) -> clause-body)`, the compiler generates an additional closure: the **dispatch trampoline**. This trampoline has the following structure (expressed as pseudo-bytecode):

```
<trampoline:op>(arg1, ..., argN, k, handler_entries_val, return_handler_val):
    ;; Step 1: Re-install the handler frame (deep semantics)
    InstallHandler handler_entries_val   ;; new opcode

    ;; Step 2: Call the user's handler clause with [arg1..argN, k]
    PushLocal arg1
    ...
    PushLocal argN
    PushLocal k
    PushLocal clause_closure             ;; captured as upvalue
    Call N+1

    ;; Step 3: Pop handler (if still installed)
    RemoveHandler                        ;; new opcode

    ;; Step 4: Apply return handler if present
    ;; (conditional — only emitted if return handler exists)
    PushLocal return_handler_val
    ;; ... apply if not nil ...

    Return
```

Wait — this approach requires the trampoline to have access to the handler entries and return handler as values. These can be passed as upvalues from the `PushHandler` site.

**Actually, simpler approach:** Rather than generating trampoline closures at compile time with all this complexity, we can use a different strategy.

### 3.3 Revised approach: Handler-aware PERFORM

Instead of compiler-generated trampolines, the key change is in the **PERFORM opcode handler** and the **handler frame**. The handler frame stores everything needed for dispatch (it already stores `HandlerEntry` closures). The PERFORM opcode, instead of returning `Err(VMSignal::Effect)` for user handlers, directly:

1. Unwinds the VM state to the handler's unwind points
2. Pushes the handler clause closure and its arguments onto the stack
3. Sets up a **synthetic call frame** that will:
   - Call the handler clause
   - On return, re-install the handler frame (for deep semantics via resume)
   - Apply the return handler if present
4. Continues execution (no Rust stack unwinding)

This is cleaner because it avoids generating trampoline closures at compile time. The dispatch logic lives in the VM's PERFORM handler.

But this has a problem: step 3 requires "on return" behavior, which means we need a way to run code after the handler clause returns. This is where we need either:
- A synthetic call frame with post-return logic, or
- A new opcode that PERFORM sets as the return target

### 3.4 Final approach: Compiler-emitted trampoline + HandleReturn opcode

The compiler generates a **single trampoline prototype per `handle` expression** (not per effect clause). This trampoline is responsible for:

1. Receiving the matched clause index, effect args, and continuation
2. Re-installing the handler frame
3. Dispatching to the correct handler clause
4. Cleaning up the handler frame after the clause returns
5. Applying the return handler

The trampoline is compiled as a closure that captures the handler entries and return handler as upvalues.

**However**, this requires the trampoline to have dynamic dispatch logic (switch on clause index), which is hard to express in bytecode without a new opcode.

### 3.5 Simplest viable approach

After considering the options, the simplest approach that eliminates `run_with_handler` is:

**PERFORM dispatches user-handler effects inline. No trampoline closures. The PERFORM opcode itself handles the dispatch by manipulating the VM state directly.**

Here's how:

When PERFORM finds a user handler (not scheduler):

1. Capture continuation (same as today)
2. Unwind VM state to handler's unwind points (same as today)
3. **Instead of returning Err(VMSignal::Effect):**
   - Find the matching handler clause
   - Push a **handler dispatch frame** — a new kind of marker frame that tells `Op::Return` to apply post-handler logic (remove handler, apply return handler)
   - Re-install the handler frame (deep semantics)
   - Set up a call to the handler clause closure with `[args..., continuation]`
   - Continue the run loop (no Rust return)

When the handler clause returns (Op::Return pops back to the dispatch frame):

4. The dispatch frame's marker tells the VM to:
   - Remove the handler from the handler stack
   - If return handler exists, call it with the clause's return value
   - Push the final result

This requires:
- A way to mark a call frame as a "handler dispatch frame"
- Modified `Op::Return` logic to detect dispatch frames

### 3.6 Concrete compiler changes

The compiler's `compile_handle` changes from emitting `PushHandler <descriptor>` to emitting:

```
;; Create body thunk closure
MakeClosure <body_proto>

;; Create handler clause closures
MakeClosure <clause_0_proto>
MakeClosure <clause_1_proto>
...

;; Create return handler closure (or push nil)
MakeClosure <return_proto>  ;; or PushConst nil

;; Install handler and call body
HandleSetup <num_clauses> <clause_0_name_idx> <clause_0_arity> ...
;; HandleSetup:
;;   - pops return_handler, clause closures, body thunk from stack
;;   - installs handler frame
;;   - calls body thunk
;;   - on body return: removes handler, applies return handler, pushes result
```

Actually, encoding variable-length data in a single opcode is awkward. Let me use the existing JSON descriptor approach but change what happens at runtime.

### 3.7 Final compiler design

**No changes to `compile_handle` output format.** The compiler still emits `PushHandler <descriptor_const_idx>`. The JSON descriptor still encodes body prototype, handler clause prototypes, and return handler prototype indices.

**All changes are in the VM's `Op::PushHandler` handler and `Op::Perform` handler.** The descriptor is parsed the same way, closures are created the same way. What changes is how they're executed.

This means: **zero compiler changes**. The entire CPS transform is a VM-only change.

---

## 4. New Opcodes

### 4.1 No new opcodes needed

After careful analysis, no new opcodes are required. The changes are entirely in the runtime behavior of existing opcodes:

- **`Op::PushHandler`**: Instead of calling `run_with_handler`, installs the handler frame and calls the body thunk as a normal closure call (via the same path as `Op::Call`). A **handler dispatch marker** is pushed onto a separate marker stack (not a new frame) to track that we're inside a `handle` expression.

- **`Op::Perform`**: For user handlers, instead of returning `Err(VMSignal::Effect)`, dispatches inline by unwinding, re-installing the handler, and calling the clause closure. A marker is pushed so that `Op::Return` knows to apply post-handler logic.

- **`Op::Return`**: Checks if the returning frame is a handler dispatch. If so, removes the handler and applies the return handler.

- **`Op::PopHandler`**: Currently a no-op. Becomes meaningful — removes the current handler frame from the handler stack when the body completes without performing any handled effects.

Actually, we do need a mechanism for `Op::Return` to know it's returning from a handler dispatch. The cleanest way is:

### 4.2 Handler dispatch marker stack

Add a small stack to the VM:

```rust
pub(crate) handler_dispatch_stack: Vec<HandlerDispatch>,
```

```rust
struct HandlerDispatch {
    /// Frame index of the handler dispatch call (the handler clause invocation)
    dispatch_frame: usize,
    /// The handler entries (for deep re-installation on resume)
    handler_entries: Vec<HandlerEntry>,
    /// Return handler closure (applied to final result)
    return_handler: Option<Closure>,
    /// Handler ID (for handler frame identity)
    handler_id: u64,
    /// Where to unwind if another effect is caught by this handler
    unwind_frame_start: usize,
    unwind_stack_start: usize,
}
```

When `Op::Return` decrements `frame_count` and the resulting `frame_count` matches `dispatch_frame` of the top `HandlerDispatch`, it means we're returning from a handler clause invocation. The VM then:
1. Pops the `HandlerDispatch`
2. Removes the handler frame (if still on the handler stack)
3. Applies the return handler if present

---

## 5. VM Changes

### 5.1 `Op::PushHandler` — new behavior

```
Current:
  1. Parse descriptor
  2. Create closures
  3. Call run_with_handler(vm, entries, return_handler, body, [])
  4. Apply return handler to result
  5. Push result

New:
  1. Parse descriptor (same)
  2. Create closures (same)
  3. Install handler frame on vm.handler_stack (same as run_with_handler's first step)
  4. Push HandlerDispatch {
       dispatch_frame: vm.frame_count,  // body thunk will be called at this frame
       handler_entries: entries,
       return_handler,
       handler_id,
       unwind_frame_start: vm.frame_count,
       unwind_stack_start: vm.stack_top,
     }
  5. Set up call to body thunk (same as Op::Call for a 0-arity closure):
     - push frame for body_closure with ip=0
     - cache_frame!()
     - continue run loop
```

The body thunk executes as normal bytecode. When it returns via `Op::Return`, we detect the handler dispatch marker.

### 5.2 `Op::Return` — handler-aware

After `Op::Return` decrements `frame_count`:

```rust
Op::Return => {
    let result = vm.pop();
    vm.frame_count -= 1;

    // Check if we're returning from a handler dispatch
    if let Some(dispatch) = vm.handler_dispatch_stack.last() {
        if vm.frame_count <= dispatch.dispatch_frame {
            let dispatch = vm.handler_dispatch_stack.pop().unwrap();

            // Remove our handler frame if still present
            if let Some(pos) = vm.handler_stack.iter().position(|h| h.id == dispatch.handler_id) {
                vm.handler_stack.remove(pos);
            }

            // Apply return handler if present
            let final_result = if let Some(ref ret_closure) = dispatch.return_handler {
                // Call ret_closure(result) — push frame, run
                call_inline(vm, ret_closure, &[result])?  // helper that runs synchronously
            } else {
                result
            };

            // Check if there's ANOTHER dispatch below (nested handle returning)
            // ... (same check recursively)

            if vm.frame_count <= base_frame_count {
                return Ok(final_result);
            }

            frame_idx = vm.frame_count - 1;
            ip = vm.frames[frame_idx].ip;
            cache_frame!();
            vm.push(final_result);
            continue;  // back to run loop
        }
    }

    // Normal return path (same as today)
    if vm.frame_count <= base_frame_count {
        return Ok(result);
    }
    frame_idx = vm.frame_count - 1;
    ip = vm.frames[frame_idx].ip;
    cache_frame!();
    vm.push(result);
}
```

**Problem with return handler:** Calling `ret_closure` here still requires either (a) `call_closure` which is a Rust-level call, or (b) pushing a frame inline.

For (b), we can push the return handler as another frame and let the run loop execute it. But then we need another marker to know when the return handler returns.

**Simpler: use a "return handler pending" flag.** When the body completes and a return handler exists:
1. Remove the handler frame
2. Push the return handler closure's frame with the result as its argument
3. Continue the run loop — the return handler executes as normal bytecode
4. When it returns, it falls through to the normal `Op::Return` path

This works because at that point, the handler dispatch marker is already consumed. The return handler is just a normal function call.

Let me revise:

```rust
// In Op::Return, after decrementing frame_count:
if let Some(dispatch) = vm.handler_dispatch_stack.last() {
    if vm.frame_count <= dispatch.dispatch_frame {
        let dispatch = vm.handler_dispatch_stack.pop().unwrap();

        // Remove handler frame
        if let Some(pos) = vm.handler_stack.iter().position(|h| h.id == dispatch.handler_id) {
            vm.handler_stack.remove(pos);
        }

        if let Some(ref ret_closure) = dispatch.return_handler {
            // Push return handler frame — it will execute and return normally
            let mut locals = vm.take_locals(ret_closure.proto.chunk.local_count as usize);
            locals[0] = result;  // pass body result as argument
            let new_frame = CallFrame {
                closure: ret_closure.clone(),
                ip: 0,
                locals,
            };
            vm.push_frame(new_frame);
            frame_idx = vm.frame_count - 1;
            ip = 0;
            cache_frame!();
            continue;  // run the return handler as normal code
        }

        // No return handler — result is the final value
        if vm.frame_count <= base_frame_count {
            return Ok(result);
        }
        frame_idx = vm.frame_count - 1;
        ip = vm.frames[frame_idx].ip;
        cache_frame!();
        vm.push(result);
        continue;
    }
}
```

This is clean. The return handler is a normal closure call that returns via `Op::Return` as usual.

### 5.3 `Op::Perform` — inline dispatch for user handlers

```
Current:
  1. Find handler on handler stack
  2. Capture continuation
  3. Unwind VM state
  4. Return Err(VMSignal::Effect { ... })

New (for user handlers):
  1. Find handler on handler stack
  2. Check if handler is a scheduler handler (is_scheduler flag)
     - If scheduler: same as today — capture continuation, return Err(VMSignal::Effect)
     - If user handler: continue with inline dispatch
  3. Capture continuation (same as today)
  4. Unwind VM state (same as today)
  5. Find matching handler clause
  6. Re-install the handler frame (deep semantics):
     - Push a NEW handler frame with the same entries and a new handler ID
  7. Push HandlerDispatch marker with the new handler ID
  8. Set up call to handler clause closure with [args..., continuation]:
     - Push frame for clause closure
     - cache_frame!()
     - continue run loop (NO Rust return)
```

The critical difference: step 8 does NOT return from `run()`. It sets up the call inline and continues the `loop`. This means the Rust call stack does not grow — handler dispatch is entirely within the VM's run loop.

### 5.4 `Op::Resume` — simplified

The RESUME opcode currently has complex logic for:
- Catching re-performed effects that target the enclosing handler
- Calling `run_with_handler` recursively for deep semantics

With CPS dispatch, **all of this is unnecessary.** When a resumed continuation re-performs an effect:
1. The handler frame is already on the VM handler stack (re-installed by step 6 of PERFORM)
2. PERFORM finds it via the normal handler stack walk
3. PERFORM dispatches inline (same as any other effect)
4. No special logic needed in RESUME

The new RESUME is just:

```
1. Pop value and continuation from stack
2. Enforce one-shot semantics
3. Restore continuation frames onto VM
4. Restore continuation stack, push resume value
5. Restore nested handlers (rebased)
6. Continue the run loop
```

No more `run()` or `run_restored_continuation()` calls. No more `saved_handler_len`. No more `top_handler_data`. RESUME just restores state and continues.

**But wait:** Currently RESUME calls `run(vm, resume_base)` and captures the result, then restores `vm.frame_count`. This is because the resumed continuation runs as a "sub-run" that returns when it hits `base_frame_count`. In the new design, the resumed continuation runs within the same `run()` invocation. The continuation's frames are pushed on top, and when the last continuation frame returns via `Op::Return`, it falls through to the frame that was executing before RESUME.

This is actually simpler and more correct. The RESUME opcode:
1. Saves the current ip to `vm.frames[frame_idx].ip`
2. Restores continuation frames on top of the current frames
3. Restores stack values and pushes resume value
4. Restores nested handlers
5. Sets `frame_idx` to the top continuation frame
6. `cache_frame!()`
7. Continues the run loop

When the continuation completes (its deepest frame does `Op::Return` back past the RESUME point), the run loop's normal Return logic delivers the value back to the caller of `(resume k v)`.

**The continuation's frames use absolute `ip` values**, so they resume where they left off. The last frame in the continuation returns to `frame_count - 1` (the frame before the continuation), which is the frame that was executing the handler clause (which called `resume`). The result lands on the stack as the return value of `(resume k v)`.

### 5.5 Delete `run_with_handler`, `run_restored_continuation`

These functions are no longer needed. Their logic is fully subsumed by the inline PERFORM dispatch and the handler dispatch marker stack.

`resume_continuation` (the Rust-callable version used by the scheduler) is still needed for scheduler fiber resumption. But it becomes simpler — it doesn't need `run_restored_continuation` because restored handlers are just on the handler stack and PERFORM handles them inline.

### 5.6 Revised `resume_continuation` (scheduler use)

```rust
pub(crate) fn resume_continuation(
    vm: &mut VM,
    cont: &VMContinuation,
    value: Value,
) -> Result<Value, VMSignal> {
    // Enforce one-shot
    if !vm.resumed_continuations.insert(cont.id) {
        return Err(VMSignal::Error(VMError::runtime("Continuation already resumed (one-shot)")));
    }

    let resume_base = vm.frame_count;

    // Restore frames
    for f in &cont.frames {
        vm.push_frame(CallFrame {
            closure: f.closure.clone(),
            ip: f.ip,
            locals: f.locals.clone(),
        });
    }

    // Restore stack + push resume value
    for v in &cont.stack {
        vm.push(v.clone());
    }
    vm.push(value);

    // Restore nested handlers (rebased)
    let saved_handler_len = vm.handler_stack.len();
    let frame_offset = resume_base as i64 - cont.frame_base as i64;
    let stack_offset = (vm.stack_top as i64 - cont.stack.len() as i64 - 1) - cont.stack_base as i64;
    for h in &cont.handler_stack {
        let rebased = rebase_handler(h, frame_offset, stack_offset);
        vm.handler_stack.push(rebased);
    }

    // Also restore any handler dispatch markers from the continuation
    // (see section 6 for how these are captured)

    // Run — effects from user handlers are dispatched inline
    // Only scheduler effects escape as VMSignal::Effect
    let result = run(vm, resume_base);

    // Cleanup
    vm.frame_count = resume_base;
    vm.handler_stack.truncate(saved_handler_len);

    result
}
```

No more `run_restored_continuation`. The `run()` call handles everything because PERFORM dispatches user-handler effects inline.

---

## 6. Object Model Changes

### 6.1 `VMHandlerFrame` — add `is_scheduler` flag

```rust
pub struct VMHandlerFrame {
    pub handlers: Vec<HandlerEntry>,
    pub return_handler: Option<Closure>,
    pub unwind_frame_start: usize,
    pub unwind_stack_start: usize,
    pub capture_frame_start: usize,
    pub capture_stack_start: usize,
    pub id: u64,
    pub is_scheduler: bool,  // NEW: true for scheduler dummy handlers
}
```

PERFORM checks this flag: if `is_scheduler`, use the current EffectSignal path. If not, dispatch inline.

### 6.2 `HandlerDispatch` — new struct

```rust
#[derive(Debug, Clone)]
pub(crate) struct HandlerDispatch {
    /// Frame count threshold — when Op::Return drops frame_count to this, we're done
    pub dispatch_frame: usize,
    /// Handler entries for deep re-installation
    pub handler_entries: Vec<HandlerEntry>,
    /// Return handler closure
    pub return_handler: Option<Closure>,
    /// Handler ID for the re-installed handler frame
    pub handler_id: u64,
}
```

### 6.3 `VMContinuation` — capture `handler_dispatch_stack`

The continuation must also capture active handler dispatch markers, since a continuation may be captured while inside a handler clause:

```rust
pub struct VMContinuation {
    pub frames: Vec<CallFrame>,
    pub stack: Vec<Value>,
    pub handler_stack: Vec<VMHandlerFrame>,
    pub handler_dispatch_stack: Vec<HandlerDispatch>,  // NEW
    pub id: u64,
    pub frame_base: usize,
    pub stack_base: usize,
}
```

When PERFORM captures a continuation, it also captures handler dispatch markers whose `dispatch_frame >= capture_frame_start`.

When a continuation is restored (RESUME or `resume_continuation`), the handler dispatch markers are restored (rebased).

### 6.4 VM struct — add handler dispatch stack

```rust
pub struct VM {
    // ... existing fields ...
    pub(crate) handler_dispatch_stack: Vec<HandlerDispatch>,  // NEW
}
```

---

## 7. Scheduler Changes

### 7.1 `install_handler_frame` — set `is_scheduler: true`

Both `scheduler.rs` and `runtime.rs` set `is_scheduler: true` when installing the dummy handler:

```rust
fn install_handler_frame(vm: &mut VM) {
    // ... same as today ...
    let handler_frame = VMHandlerFrame {
        handlers: entries,
        return_handler: None,
        unwind_frame_start: vm.frame_count,
        unwind_stack_start: vm.stack_top,
        capture_frame_start: vm.frame_count,
        capture_stack_start: vm.stack_top,
        id: next_handler_id(),
        is_scheduler: true,  // NEW
    };
    vm.handler_stack.push(handler_frame);
}
```

### 7.2 No other scheduler changes

The scheduler loop, `handle_effect`, fiber suspension/resumption — all unchanged. The only difference is that user-level effects never reach the scheduler (they're dispatched inline by PERFORM).

---

## 8. Approach C: Remove `throw` from Scheduler

### 8.1 Remove from `HANDLED_EFFECTS`

In both `scheduler.rs` and `runtime.rs`, remove `"throw"` from the `HANDLED_EFFECTS` array.

### 8.2 Add `throw` to `try_default_handler`

In `vm.rs`, add a `"throw"` case to `try_default_handler` that returns a `VMError`:

```rust
fn try_default_handler(vm: &mut VM, op_name: &str, args: &[Value]) -> Option<Value> {
    match op_name {
        "throw" => {
            // This is reached only when no user handler catches throw.
            // We return None so PERFORM produces an "Unhandled effect: throw" error.
            // The scheduler catches this error and crashes the fiber.
            None
        }
        // ... existing cases for print, println, etc. ...
    }
}
```

Actually, the existing behavior when no handler is found is to call `try_default_handler`, and if that returns `None`, return `Err(VMError)` with "Unhandled effect: throw". This already crashes the fiber when the scheduler catches the error. So simply removing `throw` from `HANDLED_EFFECTS` is sufficient — no changes to `try_default_handler` needed.

**Wait** — we should give a better error message for unhandled `throw`. Instead of "Unhandled effect: throw", include the throw message:

```rust
"throw" => {
    let msg = args.first().and_then(|v| v.as_string()).unwrap_or("(no message)");
    // Return None, but modify the error path in PERFORM to include the throw message
    None
}
```

Better: in the PERFORM opcode, when the effect is "throw" and no handler is found:

```rust
if op_name == "throw" {
    let msg = args.first().and_then(|v| v.as_string().map(|s| s.to_string()))
        .unwrap_or_else(|| "(no message)".to_string());
    return Err(VMSignal::Error(vm.vm_error(
        format!("Uncaught exception: {}", msg),
        &vm.frames[frame_idx],
    )));
}
```

---

## 9. Scope & Deliverables

| # | Deliverable | Effort | Depends On |
|---|-------------|--------|------------|
| D1 | Add `is_scheduler` flag to `VMHandlerFrame` | S | - |
| D2 | Add `HandlerDispatch` struct and `handler_dispatch_stack` to VM | S | - |
| D3 | Add `handler_dispatch_stack` to `VMContinuation` | S | D2 |
| D4 | Rewrite `Op::PushHandler` — inline body call with dispatch marker | M | D1, D2 |
| D5 | Rewrite `Op::Perform` — inline dispatch for user handlers | L | D1, D2, D3 |
| D6 | Modify `Op::Return` — handler dispatch detection and return handler | M | D2, D4 |
| D7 | Rewrite `Op::Resume` — simplified inline continuation restore | M | D3, D5 |
| D8 | Simplify `resume_continuation` — remove `run_restored_continuation` dependency | M | D5 |
| D9 | Delete `run_with_handler`, `run_restored_continuation` | S | D4, D5, D6, D7, D8 |
| D10 | Remove `throw` from `HANDLED_EFFECTS` (scheduler.rs + runtime.rs) | S | D5 |
| D11 | Better error message for unhandled `throw` | S | D10 |
| D12 | Set `is_scheduler: true` in scheduler's `install_handler_frame` | S | D1 |
| D13 | Update `rebase_handler` for new field | S | D1 |
| D14 | Capture/restore `handler_dispatch_stack` in PERFORM and RESUME | M | D2, D3, D5, D7 |
| D15 | Verify all 33 handler/effect tests pass | L | D4-D14 |
| D16 | Add new tests for cross-scheduler-effect handler survival | M | D5, D10 |
| D17 | Update known-issues.md — mark #4 as fixed, update #12 | S | D15 |

---

## 10. Non-Goals

- **Scheduler unification** — Scheduler effects (`fiber-spawn`, `tcp-connect`, etc.) continue to use the `EffectSignal` propagation path. Unifying them into VM-level handler closures is a separate project that requires a scheduler context bridging mechanism.
- **Multi-shot continuations** — Continuations remain one-shot. Multi-shot would require `Clone` for continuations and is orthogonal to this change.
- **Performance optimization** — This change is not expected to improve performance (may slightly regress due to handler dispatch markers). Performance work (Issue #1) is separate.
- **Compiler changes** — The compiler's `compile_handle` output is unchanged. All changes are in the VM runtime.
- **Handler composition** — No new user-facing syntax or semantics. `handle`/`perform`/`resume` work exactly as before, just with correct behavior across scheduler suspensions.

---

## 11. Acceptance Criteria

- [ ] All 33 existing handler/effect tests pass (tests #1-#33 from inventory)
- [ ] All 331 existing VM tests pass (`cargo test -p marlang-vm`)
- [ ] `run_with_handler` and `run_restored_continuation` functions are deleted
- [ ] `Op::PopHandler` is no longer a no-op (it removes the handler frame)
- [ ] New test: `handle`/`throw` works correctly when handler clause does `perform println` (the exact failure mode from Issue #4)
- [ ] New test: Nested user handlers survive scheduler suspension
- [ ] New test: `throw` without a user handler crashes the fiber with "Uncaught exception: ..." message (not "Unhandled effect: throw")
- [ ] New test: Deep handler semantics work — handler re-catches effects from resumed continuations
- [ ] `throw` is not in `HANDLED_EFFECTS` in either `scheduler.rs` or `runtime.rs`
- [ ] `net.mar` and `tcp.mar` still run correctly with and without `--no-typecheck`

---

## 12. Test Strategy

| Layer | What | How |
|-------|------|-----|
| Unit | Handler dispatch marker push/pop | Test that dispatch markers are created and consumed correctly in isolation |
| Unit | PERFORM inline dispatch | Test that user effects don't produce VMSignal::Effect |
| Unit | RESUME simplified | Test that resumed continuations execute inline |
| Integration | All existing tests | `cargo test -p marlang-vm` — all 331 tests must pass |
| Integration | Cross-suspension handlers | New tests with `handle`/`throw` + scheduler effects (`println`, `fiber-yield`) |
| Integration | `run_program` with effects | Test programs that combine user handlers with IO/scheduler effects |
| E2E | `net.mar`, `tcp.mar` | Run via `cargo run -p marlang-cli -- run` both with and without `--no-typecheck` |

---

## 13. Risks & Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Handler dispatch marker not captured/restored correctly in continuations | Medium | High — breaks deep handler semantics | Thorough testing of nested handle + resume + re-perform scenarios. The `exec_stateful_counter` test (accumulation via handler wrapping) is the key correctness test. |
| Return handler invocation via frame push interacts badly with `base_frame_count` checks | Medium | Medium — return handler may not execute | Careful testing of return handlers in all contexts (top-level, nested, inside scheduler fibers) |
| Performance regression from dispatch marker stack operations | Low | Low — markers are rare (only on handle/perform, not on every instruction) | Benchmark fib(35) before/after. Dispatch markers are small and stack operations are O(1). |
| `Op::Return` handler-awareness adds branching to the hot path | Medium | Low-Medium — Return is on every function return | The check is a single `if let Some(...) = handler_dispatch_stack.last()` — one pointer comparison. Only enters the handler path when dispatch_frame matches, which is rare. |
| Scheduler's `resume_continuation` breaks because it relied on `run_restored_continuation` | Medium | High — scheduler fibers crash | Simplify `resume_continuation` as specified. Test with all scheduler test suites. |

---

## 14. Trade-offs

| Chose | Over | Because |
|-------|------|---------|
| VM-only changes (no compiler changes) | Compiler-emitted trampolines | Smaller diff, same correctness guarantees. The handler dispatch logic is in PERFORM which already has all the information needed. Compiler trampolines would add complexity for no benefit since the dispatch decision (which clause to call) is a runtime decision based on effect name. |
| Handler dispatch marker stack | Synthetic call frames with marker bits | Separate stack is simpler to reason about. Adding marker bits to `CallFrame` would complicate every frame operation. A separate small stack (`Vec<HandlerDispatch>`) is cache-friendly and independent. |
| Keep EffectSignal for scheduler | Full scheduler unification | Scheduler effects fundamentally require scheduler-level control flow (enqueue/park fibers, async TCP, cross-thread messaging). VM closures can't access this state. The EffectSignal path already works correctly for the scheduler — Issue #4 only affects user handlers. |
| Remove `throw` from scheduler dummy | Keep `throw` in scheduler | With CPS dispatch for user handlers, `throw` should always be caught by user handlers first. Removing it from the scheduler eliminates a source of confusion and ensures the scheduler never swallows exceptions. |

---

Phase: IMPLEMENTED | Completed: 2026-03-01
