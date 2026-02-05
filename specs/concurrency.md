# Concurrency System for Marlang

**Type:** Feature Plan
**Effort:** XL (>2 days)
**Status:** Ready for implementation

## Problem

Marlang has no concurrency primitives. Programs run single-threaded with no way to express concurrent computations, communicate between independent tasks, or build fault-tolerant systems. A network stack, server applications, and interactive programs all require concurrency.

## Constraints

| Constraint | Impact |
|------------|--------|
| Build on existing effects | Must use `PERFORM`/continuation machinery, no new opcodes |
| Cooperative scheduling | Single-threaded trampoline, no OS threads |
| Async IO | IO operations park fibers and use Bun async APIs; other fibers keep running |
| Two-layer design | Green threads + channels (Layer 1), actor model (Layer 2) |
| Effect-based API | All concurrency ops are algebraic effects handled by a native scheduler |
| Both backends | Concurrency works on both the bytecode VM and the tree-walker interpreter |
| One-shot continuations | Each fiber yields once per scheduling cycle, creating a fresh continuation |

## Solution

**Async trampoline scheduler implemented as a native effect handler in TypeScript.** Fibers are lightweight green threads whose suspension/resumption is powered by the existing effect/continuation machinery in both backends. Channels provide CSP-style communication. The actor model (PIDs, mailboxes, supervision, selective receive) is layered on top. IO is channel-based and backed by Bun's async APIs — fibers park on IO while others keep running.

Two scheduler implementations share the same core logic but differ in how they capture and resume continuations:

| | VM (`src/vm/scheduler.ts`) | Tree-walker (`src/core/scheduler.ts`) |
|---|---|---|
| Continuation type | `VMContinuation` (frames + stack snapshot) | `Continuation` (generator function) |
| Capture mechanism | `PERFORM` throws `EffectSignal` with captured frames/stack | `yield` suspends generator, yielding `EffectSuspension` |
| Resume mechanism | `resumeContinuation()` restores frames/stack, calls `run()` | `cont.resume(value)` returns a new generator, drive with `.next()` |
| Scheduler pattern | try/catch loop around `callClosure`/`resumeContinuation` | Generator-driving loop: call `.next()`, inspect yielded suspension |

No new VM opcodes are needed. The stdlib files (`concurrent.mar`, `actor.mar`, `io.mar`) are **shared** — they just use `perform`, which works on both backends.

---

## Architecture Overview

```
┌──────────────────────────────────────────────────────────────────┐
│                         User Code (.mar)                         │
├───────────────┬──────────────────┬───────────────────────────────┤
│ stdlib/       │  stdlib/         │  stdlib/                      │
│ concurrent    │  actor.mar       │  io.mar                       │
│ .mar          │  (send, receive, │  (stdout, stdin,              │
│ (spawn,       │   monitor, link, │   read-file, write-file,      │
│  yield,       │   trap-exit,     │   open-file)                  │
│  chan-*)      │   receive-match) │                               │
├───────────────┴──────────────────┴───────────────────────────────┤
│                    Shared scheduler logic                         │
│  Ready queue, channels, mailboxes, supervision, pending IO       │
├──────────────────────────────┬───────────────────────────────────┤
│  src/vm/scheduler.ts         │  src/core/scheduler.ts            │
│  VM backend                  │  Tree-walker backend              │
│                              │                                   │
│  Catches EffectSignal,       │  Drives generators,               │
│  calls resumeContinuation()  │  calls cont.resume(value),        │
│  to restore frames/stack     │  inspects yielded suspensions     │
├──────────────────────────────┼───────────────────────────────────┤
│  src/vm/vm.ts                │  src/core/interpreter.ts          │
│  PERFORM → EffectSignal      │  yield EffectSuspension           │
│  VMContinuation              │  Generator<EffectSuspension, ...> │
├──────────────────────────────┼───────────────────────────────────┤
│  src/vm/bytecode.ts          │  src/core/types.ts                │
│  VMPid, VMChannel            │  Pid, Channel RuntimeValue kinds  │
└──────────────────────────────┴───────────────────────────────────┘
```

### How the Async Trampoline Works

1. `run-fibers` builtin installs a handler frame on `vm.handlerStack` with entries for all concurrency + IO effect names
2. Calls `callClosure(vm, body, [])` to start the main fiber (PID 0)
3. When a fiber performs `fiber-spawn`/`chan-send`/`io-read-file`/etc., `PERFORM` finds the handler frame, captures a one-shot continuation, and throws `EffectSignal`
4. The scheduler's try/catch catches the `EffectSignal`, processes the effect:
   - **Sync effects** (spawn, yield, chan-send, actor-send, etc.): update queues/channels/mailboxes, enqueue the fiber to resume
   - **Async IO effects** (read-file, read-line, etc.): start a Bun async operation, park the fiber in `pendingIO`, do NOT enqueue it
5. Pick the next ready fiber and resume via `resumeContinuation(vm, cont, value)`
6. When that fiber performs an effect, `EffectSignal` propagates back up (step 4)
7. **If no ready fibers but pending IO**: `await Promise.race(pendingIO)` — drain the event loop until an IO operation completes, wake the corresponding fiber
8. Repeat until all fibers complete or deadlock is detected (no ready fibers AND no pending IO)

```
Scheduler                      VM                         OS / Bun
   │                            │                            │
   │── callClosure(body) ──────►│                            │
   │                            │── runs fiber code...       │
   │                            │── PERFORM "fiber-spawn"    │
   │◄── EffectSignal ──────────│   (continuation captured)  │
   │                            │                            │
   │   [create fiber, enqueue]  │                            │
   │── resumeContinuation ─────►│                            │
   │   (resume parent w/ PID)   │── runs fiber code...       │
   │                            │── PERFORM "io-read-file"   │
   │◄── EffectSignal ──────────│                            │
   │                            │                            │
   │   [start async read,       │                            │
   │    park fiber in pendingIO] │── Bun.file().text() ─────►│
   │                            │                            │
   │── resumeContinuation ─────►│   (run other fiber)        │
   │   (resume child fiber)     │── runs...                  │
   │                            │── PERFORM "fiber-yield"    │
   │◄── EffectSignal ──────────│                            │
   │                            │                            │
   │   [no ready fibers,        │                            │
   │    await pending IO]       │                            │
   │                            │◄── file content ──────────│
   │   [wake parked fiber]      │                            │
   │── resumeContinuation ─────►│                            │
   │   (resume w/ file content) │── runs...                  │
   ...                          ...                          ...
```

---

## Layer 1: Green Threads (Fibers) + Channels

### Fiber API (`stdlib/concurrent.mar`)

```lisp
(effect Concurrent
  (fiber-spawn (-> (-> Unit Unit) Pid))
  (fiber-yield (-> Unit Unit))
  (fiber-self  (-> Unit Pid)))

;; Convenience wrappers
(define spawn   (lambda (f) (perform fiber-spawn f)))
(define yield   (lambda () (perform fiber-yield)))
(define self    (lambda () (perform fiber-self)))
(define run     (lambda (f) (run-fibers f)))
```

### Channel API (`stdlib/concurrent.mar`)

```lisp
(effect Channel
  (chan-new   (-> Int Channel))
  (chan-send  (-> Channel a Unit))
  (chan-recv  (-> Channel a))
  (chan-close (-> Channel Unit)))

(define chan-new   (lambda (cap) (perform chan-new cap)))
(define chan-send  (lambda (ch v) (perform chan-send ch v)))
(define chan-recv  (lambda (ch) (perform chan-recv ch)))
(define chan-close (lambda (ch) (perform chan-close ch)))
```

### Fiber Semantics

| Operation | Behavior |
|-----------|----------|
| `spawn f` | Create a new fiber running thunk `f`. Assign a fresh PID. Add to ready queue. Resume parent with the PID. |
| `yield` | Save current fiber's continuation to the **end** of the ready queue. Switch to next ready fiber. Resume with `nil`. |
| `self` | Resume immediately with current fiber's PID. No context switch. |

### Channel Semantics

Channels are typed communication pipes with optional buffering.

| Operation | Behavior |
|-----------|----------|
| `chan-new cap` | Create a channel with buffer capacity `cap`. `cap=0` means unbuffered (rendezvous). Resume with a `VMChannel` value. |
| `chan-send ch val` | **Unbuffered/full**: If a receiver is waiting, hand off directly and wake receiver. If buffer has space, enqueue value. Otherwise, block sender (save continuation). Resume sender with `nil`. |
| `chan-recv ch` | **Buffered**: If buffer non-empty, dequeue and return. If a sender is blocked, accept their value, wake sender. If channel closed and empty, return `nil`. Otherwise, block receiver. |
| `chan-close ch` | Mark channel as closed. Wake all blocked receivers with `nil`. Blocked senders get an error. |

**Unbuffered channel (rendezvous) protocol:**
- `chan-send` on empty channel with no waiting receiver → block sender
- `chan-recv` on empty channel with no waiting sender → block receiver
- When both a sender and receiver are blocked on the same channel → hand off value directly, wake both

### Example: Producer-Consumer

```lisp
(import "std:concurrent" as conc)

(conc/run (lambda ()
  (let ((ch (conc/chan-new 0)))
    ;; Producer
    (conc/spawn (lambda ()
      (conc/chan-send ch 1)
      (conc/chan-send ch 2)
      (conc/chan-send ch 3)
      (conc/chan-close ch)))
    ;; Consumer
    (let loop ((v (conc/chan-recv ch)))
      (if (= v nil)
        (println "done")
        (begin
          (println v)
          (loop (conc/chan-recv ch))))))))
;; Output: 1 2 3 done
```

### Example: Fan-Out

```lisp
(import "std:concurrent" as conc)

(conc/run (lambda ()
  (let ((ch (conc/chan-new 10)))
    ;; Spawn 3 workers
    (conc/spawn (lambda () (conc/chan-send ch (+ 1 1))))
    (conc/spawn (lambda () (conc/chan-send ch (+ 2 2))))
    (conc/spawn (lambda () (conc/chan-send ch (+ 3 3))))
    ;; Collect results
    (let ((a (conc/chan-recv ch))
          (b (conc/chan-recv ch))
          (c (conc/chan-recv ch)))
      (println (+ a (+ b c)))))))   ;; → 12
```

---

## Layer 2: Actor Model

Built on fibers. Each actor is a fiber with a PID and a mailbox. Actor operations are additional effects handled by the same scheduler.

### Actor API (`stdlib/actor.mar`)

```lisp
(data Signal
  (Down Pid String)
  (Exit Pid String))

(effect Actor
  (actor-send          (-> Pid a Unit))
  (actor-receive       (-> Unit a))
  (actor-receive-match (-> (-> a Bool) a))
  (actor-monitor       (-> Pid Unit))
  (actor-link          (-> Pid Unit))
  (actor-unlink        (-> Pid Unit))
  (actor-trap-exit     (-> Bool Unit)))

;; Convenience wrappers
(define send          (lambda (pid msg) (perform actor-send pid msg)))
(define receive       (lambda () (perform actor-receive)))
(define receive-match (lambda (pred) (perform actor-receive-match pred)))
(define monitor       (lambda (pid) (perform actor-monitor pid)))
(define link          (lambda (pid) (perform actor-link pid)))
(define unlink        (lambda (pid) (perform actor-unlink pid)))
(define trap-exit     (lambda (b) (perform actor-trap-exit b)))
```

### Actor Semantics

| Operation | Behavior |
|-----------|----------|
| `send pid msg` | Enqueue `msg` in target's mailbox. If target is blocked on `receive` (and predicate matches, if selective), wake it. Non-blocking for sender. Resume sender with `nil`. |
| `receive` | If mailbox non-empty, dequeue and return first message. If empty, block actor (save continuation). |
| `receive-match pred` | Scan mailbox for the first message where `(pred msg)` returns `#t`. If found, remove it and return. If no match, block with predicate — future messages are tested as they arrive. Skipped messages remain in the mailbox (selective receive). |
| `monitor pid` | Register a monitoring relationship. When the monitored actor exits (normally or via crash), the monitor receives `(Down pid reason)` in its mailbox. |
| `link pid` | Create a bidirectional link. When either linked actor crashes, the other is also killed — unless it has trap-exit enabled, in which case it receives `(Exit pid reason)`. |
| `unlink pid` | Remove a bidirectional link. |
| `trap-exit b` | When `#t`, link deaths deliver `(Exit pid reason)` messages instead of killing this actor. When `#f` (default), link deaths propagate. |

### Selective Receive

Selective receive is the key differentiator from channel-based communication. It allows an actor to process messages **out of order** based on a predicate:

```lisp
;; Wait for a specific reply, skipping other messages
(define (wait-for-reply expected-id)
  (receive-match
    (lambda (msg)
      (match msg
        ((Reply id _) (= id expected-id))
        (_ #f)))))
```

Implementation: the scheduler maintains the full mailbox as a list. `receive-match` scans it linearly. When blocking with a predicate, incoming messages are tested against the predicate before deciding to wake the actor.

### Supervision

#### Monitors (unidirectional, non-destructive)

```lisp
;; Monitor a worker — get notified if it dies
(let ((worker (conc/spawn (lambda () (do-work))))
      (me (conc/self)))
  (actor/monitor worker)
  ;; ... later ...
  (match (actor/receive)
    ((Down pid reason) (println (+ "Worker died: " reason)))))
```

- Monitors are unidirectional: monitor watches target, not vice versa
- Target's death delivers `(Down pid reason)` to monitor's mailbox
- Monitor is not affected by target's death (only notified)
- `reason` is `"normal"` for clean exit, or the error message for crashes

#### Links (bidirectional, destructive by default)

```lisp
;; Link to a worker — if it dies, we die too
(let ((worker (conc/spawn (lambda () (do-risky-work)))))
  (actor/link worker)
  ;; If worker crashes, this actor crashes too
  (do-more-work))
```

- Links are bidirectional: if either side dies, the other is killed
- `trap-exit #t` converts death signals into messages:

```lisp
;; Supervisor pattern
(define (supervisor)
  (actor/trap-exit #t)
  (let ((worker (conc/spawn do-work)))
    (actor/link worker)
    (let loop ()
      (match (actor/receive)
        ((Exit pid reason)
          (println (+ "Restarting worker, died with: " reason))
          (let ((new-worker (conc/spawn do-work)))
            (actor/link new-worker)
            (loop)))
        (other
          (loop))))))
```

### Example: Counter Actor

```lisp
(import "std:concurrent" as conc)
(import "std:actor" as actor)

(data CounterMsg
  (Increment Int)
  (GetCount Pid)
  (Stop))

(data CounterReply
  (Count Int))

(define (counter count)
  (let ((msg (actor/receive)))
    (match msg
      ((Increment n) (counter (+ count n)))
      ((GetCount sender)
        (actor/send sender (Count count))
        (counter count))
      ((Stop) nil))))

(conc/run (lambda ()
  (let ((c (conc/spawn (lambda () (counter 0))))
        (me (conc/self)))
    (actor/send c (Increment 5))
    (actor/send c (Increment 3))
    (actor/send c (GetCount me))
    (let ((reply (actor/receive)))
      (match reply
        ((Count n) (println n)))))))   ;; → 8
```

### Example: Ping-Pong

```lisp
(import "std:concurrent" as conc)
(import "std:actor" as actor)

(data PingPong (Ping Pid) (Pong Pid))

(define (ping partner n)
  (if (<= n 0)
    (println "ping done")
    (let ((me (conc/self)))
      (actor/send partner (Ping me))
      (let ((msg (actor/receive)))
        (match msg
          ((Pong _) (ping partner (- n 1))))))))

(define (pong)
  (let ((msg (actor/receive)))
    (match msg
      ((Ping sender)
        (let ((me (conc/self)))
          (actor/send sender (Pong me))
          (pong))))))

(conc/run (lambda ()
  (let ((p (conc/spawn pong)))
    (ping p 1000))))
```

---

## Layer 3: Async IO via Channels

IO operations are modeled as channels and async effects. Fibers interact with the outside world through channels; the scheduler backs them with Bun's async APIs so IO never blocks other fibers.

### IO API (`stdlib/io.mar`)

```lisp
(effect IO
  (io-stdout        (-> Unit Channel))
  (io-stderr        (-> Unit Channel))
  (io-stdin         (-> Unit Channel))
  (io-read-file     (-> String String))
  (io-write-file    (-> String String Unit))
  (io-open-file     (-> String String Channel)))

;; Convenience wrappers
(define stdout     (lambda () (perform io-stdout)))
(define stderr     (lambda () (perform io-stderr)))
(define stdin      (lambda () (perform io-stdin)))
(define read-file  (lambda (path) (perform io-read-file path)))
(define write-file (lambda (path content) (perform io-write-file path content)))
(define open-file  (lambda (path mode) (perform io-open-file path mode)))
```

### Predefined IO Channels

The scheduler creates these on startup and injects them via effects:

| Channel | Direction | Backed by | Behavior |
|---------|-----------|-----------|----------|
| `stdout` | write-only | `Bun.write(Bun.stdout, ...)` | `chan-send stdout "text"` writes to stdout. Non-blocking — the scheduler flushes asynchronously. |
| `stderr` | write-only | `Bun.write(Bun.stderr, ...)` | Same as stdout but for stderr. |
| `stdin` | read-only | Bun async readline | `chan-recv stdin` parks the fiber. The scheduler reads a line asynchronously. When input arrives, the fiber wakes with the string. |

Stdout/stderr channels are buffered (capacity 64). Stdin is unbuffered (rendezvous) — each `chan-recv` triggers exactly one line read.

### One-Shot Async IO

For file operations that return a single result:

| Effect | Async API | Behavior |
|--------|-----------|----------|
| `io-read-file path` | `Bun.file(path).text()` | Park fiber, start async read. When promise resolves, wake fiber with string content. |
| `io-write-file path content` | `Bun.write(path, content)` | Park fiber, start async write. When promise resolves, wake fiber with `nil`. |

These are handled directly by the scheduler as async effects — the fiber gets a continuation captured, the scheduler starts the promise, and the fiber is parked in `pendingIO` until the promise resolves.

### Streaming File IO

`io-open-file` returns a channel for streaming reads or writes:

```lisp
;; Read a file line by line
(import "std:concurrent" as conc)
(import "std:io" as io)

(conc/run (lambda ()
  (let ((ch (io/open-file "big.csv" "r")))
    (let loop ((line (conc/chan-recv ch)))
      (if (= line nil)
        (println "done")
        (begin
          (process-line line)
          (loop (conc/chan-recv ch))))))))
```

The scheduler creates a system fiber that reads the file in chunks and pushes lines into the channel. When the file is exhausted, the channel is closed (receivers get `nil`).

For write mode:

```lisp
(conc/run (lambda ()
  (let ((ch (io/open-file "output.txt" "w")))
    (conc/chan-send ch "line 1\n")
    (conc/chan-send ch "line 2\n")
    (conc/chan-close ch))))
```

### How Async IO Integrates with the Trampoline

The scheduler's trampoline loop has three phases per iteration:

```
async loop:
  1. SYNC PHASE: Pick next ready fiber, run until it yields.
     Process effect — if sync (spawn, yield, chan-send, etc.),
     update state and enqueue fiber(s).

  2. ASYNC PHASE: If the effect is an IO operation:
     - Start the Bun async operation (returns a Promise)
     - Park the fiber: save continuation in pendingIO map
     - Do NOT add fiber to ready queue

  3. DRAIN PHASE: If ready queue is empty but pendingIO is non-empty:
     - await Promise.race(pendingIO) — yield to JS event loop
     - When a promise resolves, move the fiber from pendingIO to
       ready queue with the resolved value
     - Also flush stdout/stderr buffer channels
     - Go to step 1
```

The drain phase is what makes IO truly non-blocking. While fiber A waits on a file read, fibers B and C can keep running. When the file read completes, the event loop wakes the scheduler, which wakes fiber A.

### Backward Compatibility with Default IO Handlers

The existing `tryDefaultHandler()` in vm.ts handles IO effects (`print`, `println`, `read-line`, `read-file`, `write-file`) synchronously as a fallback when no handler frame is installed. This continues to work for programs that don't use `run-fibers`:

```lisp
;; This still works — synchronous, no fibers
(println "hello")
(let ((content (perform read-file "data.txt")))
  (println content))
```

Inside `run-fibers`, the scheduler's handler frame takes priority over `tryDefaultHandler`, routing IO through the async machinery. Outside `run-fibers`, the old synchronous behavior is preserved.

### Mapping Old IO Effects to New

| Old effect name | New effect name | Inside `run-fibers` | Outside `run-fibers` |
|-----------------|-----------------|---------------------|----------------------|
| `print` | `print` | Scheduler writes to stdout channel → async flush | `tryDefaultHandler` → sync `process.stdout.write` |
| `println` | `println` | Scheduler writes to stdout channel + newline → async flush | `tryDefaultHandler` → sync `console.log` |
| `read-line` | `read-line` | Scheduler parks fiber → async readline | `tryDefaultHandler` → sync `prompt()` |
| `read-file` | `read-file` / `io-read-file` | Scheduler parks fiber → `Bun.file().text()` | `tryDefaultHandler` → sync `fs.readFileSync` |
| `write-file` | `write-file` / `io-write-file` | Scheduler parks fiber → `Bun.write()` | `tryDefaultHandler` → sync `fs.writeFileSync` |

This means existing programs that use `println` and `read-file` effects get async behavior for free when wrapped in `run-fibers`, with zero code changes.

### Example: Concurrent File Processing

```lisp
(import "std:concurrent" as conc)
(import "std:io" as io)

(conc/run (lambda ()
  (let ((ch (conc/chan-new 3)))
    ;; Spawn 3 file readers in parallel
    (conc/spawn (lambda () (conc/chan-send ch (io/read-file "a.txt"))))
    (conc/spawn (lambda () (conc/chan-send ch (io/read-file "b.txt"))))
    (conc/spawn (lambda () (conc/chan-send ch (io/read-file "c.txt"))))
    ;; Collect results — all 3 reads happen concurrently
    (let ((a (conc/chan-recv ch))
          (b (conc/chan-recv ch))
          (c (conc/chan-recv ch)))
      (println (+ "Total size: " (show (+ (string-length a)
                                          (+ (string-length b)
                                             (string-length c))))))))))
```

All three file reads run concurrently via async IO. Each fiber parks on its `read-file`, the scheduler starts all three `Bun.file().text()` promises, and fibers wake as their reads complete.

### Example: Interactive IO

```lisp
(import "std:concurrent" as conc)
(import "std:io" as io)

(conc/run (lambda ()
  (let ((out (io/stdout))
        (in  (io/stdin)))
    (conc/chan-send out "What's your name? ")
    (let ((name (conc/chan-recv in)))
      (conc/chan-send out (+ "Hello, " name "\n"))))))
```

---

## Implementation Details

### File Structure

```
src/vm/
  bytecode.ts        ← VMPid, VMChannel types added to VMObject union
  vm.ts              ← Export EffectSignal, callClosure, add resumeContinuation
  scheduler.ts       ← NEW: VM async trampoline scheduler
  builtins.ts        ← run-fibers builtin
  index.ts           ← Updated exports

src/core/
  types.ts           ← Pid, Channel added to RuntimeValue union
  interpreter.ts     ← Export needed internals, register run-fibers builtin
  scheduler.ts       ← NEW: Tree-walker async trampoline scheduler
  io-handlers.ts     ← Existing (unchanged — provides sync fallback outside run-fibers)

src/concurrent/
  scheduler-core.ts  ← NEW: Shared scheduler logic (state, effect dispatch, channels, actors, supervision)

stdlib/
  concurrent.mar     ← Fiber + Channel API (shared by both backends)
  actor.mar          ← Actor API (shared by both backends)
  io.mar             ← Async IO API (shared by both backends)

spec/
  concurrent.test.ts ← Tests (fibers, channels, actors, IO — run on both backends)
```

### 1. `src/vm/bytecode.ts` — New Types

Add to `VMObject` union:

```typescript
export type VMPid = {
  kind: 'pid';
  id: number;
};

export type VMChannel = {
  kind: 'channel';
  id: number;
  buffer: VMValue[];
  capacity: number;
  closed: boolean;
};
```

Update `VMBuiltin` to accept optional VM reference:

```typescript
export type VMBuiltin = {
  kind: 'vm-builtin';
  name: string;
  fn: (args: VMValue[], vm?: any) => VMValue;
};
```

### 2. `src/vm/vm.ts` — Export Internals + `resumeContinuation`

**Export** the following (currently module-private):
- `EffectSignal` class
- `VMHandlerFrame` type
- `callClosure` function
- `isTruthy` function
- `run` function
- `push`, `pop` functions

**Add** `resumeContinuation`:

```typescript
export function resumeContinuation(
  vm: VM,
  cont: VMContinuation,
  value: VMValue,
): VMValue {
  if (cont.resumed) {
    throw new VMError('Continuation already resumed (one-shot)', []);
  }
  cont.resumed = true;

  // Restore continuation frames
  const resumeBase = vm.frameCount;
  for (const f of cont.frames) {
    vm.frames[vm.frameCount++] = {
      closure: f.closure,
      ip: f.ip,
      locals: [...f.locals],
    };
  }

  // Restore continuation stack
  for (const v of cont.stack) {
    push(vm, v);
  }

  // Push the resume value (what PERFORM "returns" to the body)
  push(vm, value);

  // Restore nested handlers, rebasing frame/stack indices
  const savedHandlerLen = vm.handlerStack.length;
  const frameOffset = resumeBase - cont.frameBase;
  const stackOffset = (vm.stackTop - cont.stack.length - 1) - cont.stackBase;
  for (const h of cont.handlerStack as VMHandlerFrame[]) {
    vm.handlerStack.push({
      handlers: h.handlers,
      returnHandler: h.returnHandler,
      unwindFrameCount: h.unwindFrameCount + frameOffset,
      unwindStackTop: h.unwindStackTop + stackOffset,
      captureFrameStart: h.captureFrameStart + frameOffset,
      captureStackStart: h.captureStackStart + stackOffset,
    });
  }

  // Run — let EffectSignal propagate to caller (scheduler catches it)
  const result = run(vm, resumeBase);

  // Clean up
  vm.frameCount = resumeBase;
  vm.handlerStack.length = savedHandlerLen;

  return result;
}
```

Key difference from the RESUME opcode: `resumeContinuation` **does not** catch `EffectSignal` internally — it lets effects propagate to the scheduler's try/catch loop. The RESUME opcode handles deep-handler re-dispatch itself because it runs within a handler body; `resumeContinuation` is called from TypeScript (the scheduler), so propagation is the correct behavior.

**Update** builtin call sites to pass `vm`:
```typescript
// In CALL and TAIL_CALL handlers:
const result = callee.fn(args, vm);  // was: callee.fn(args)
```

**Update** `formatVMValue`, `valuesEqual`, `toRuntimeValue` for `pid` and `channel` kinds.

### 3. `src/vm/scheduler.ts` — VM Trampoline Scheduler

Core data types:

```typescript
import type { VM } from './vm';
import type { VMClosure, VMContinuation, VMValue, VMPid, VMChannel } from './bytecode';
import { NIL } from './bytecode';

type FiberId = number;

type Fiber =
  | { state: 'new'; closure: VMClosure; pid: FiberId }
  | { state: 'suspended'; cont: VMContinuation; resumeValue: VMValue; pid: FiberId };

type ChannelState = {
  buffer: VMValue[];
  capacity: number;
  closed: boolean;
  waitingSenders: Array<{ pid: FiberId; value: VMValue; cont: VMContinuation }>;
  waitingReceivers: Array<{ pid: FiberId; cont: VMContinuation }>;
};

type SchedulerState = {
  vm: VM;
  readyQueue: Fiber[];
  currentPid: FiberId;
  nextPid: FiberId;

  // Channels (Layer 1)
  channels: Map<number, ChannelState>;
  nextChannelId: number;

  // Actor layer (Layer 2)
  mailboxes: Map<FiberId, VMValue[]>;
  blockedOnReceive: Map<FiberId, {
    cont: VMContinuation;
    predicate?: VMClosure;  // for selective receive
  }>;

  // Supervision (Layer 2)
  monitors: Map<FiberId, Set<FiberId>>;   // target → set of monitors
  links: Map<FiberId, Set<FiberId>>;       // pid → set of linked pids
  trapExit: Set<FiberId>;
  livePids: Set<FiberId>;

  // Async IO (Layer 3)
  pendingIO: Map<FiberId, { promise: Promise<VMValue>; cont: VMContinuation }>;
  stdoutChannel: number;   // channel ID for stdout
  stderrChannel: number;   // channel ID for stderr
  stdinChannel: number;    // channel ID for stdin
};
```

**Entry point**: `async runFibers(vm: VM, body: VMClosure): Promise<VMValue>`

```typescript
export async function runFibers(vm: VM, body: VMClosure): Promise<VMValue> {
  const state: SchedulerState = { /* initialize, create IO channels */ };

  // Install handler frame for all concurrency + IO effect names
  installHandlerFrame(state);

  // Create predefined IO channels
  initIOChannels(state);

  // Run main fiber (PID 0)
  let mainResult: VMValue = NIL;

  // Start main fiber
  try {
    mainResult = callClosure(vm, body, []);
    handleFiberExit(state, 0, 'normal', NIL);
  } catch (e) {
    if (e instanceof EffectSignal) {
      handleEffect(state, e);
    } else {
      handleFiberExit(state, 0, 'crash', e);
    }
  }

  // Async trampoline loop
  while (state.readyQueue.length > 0 || state.pendingIO.size > 0) {
    // --- SYNC PHASE: run ready fibers ---
    while (state.readyQueue.length > 0) {
      const fiber = state.readyQueue.shift()!;
      state.currentPid = fiber.pid;

      try {
        let result: VMValue;
        if (fiber.state === 'new') {
          result = callClosure(vm, fiber.closure, []);
        } else {
          result = resumeContinuation(vm, fiber.cont, fiber.resumeValue);
        }
        if (fiber.pid === 0) mainResult = result;
        handleFiberExit(state, fiber.pid, 'normal', NIL);
      } catch (e) {
        if (e instanceof EffectSignal) {
          handleEffect(state, e);
        } else {
          handleFiberExit(state, fiber.pid, 'crash', e);
        }
      }
    }

    // --- DRAIN PHASE: await pending IO if no ready fibers ---
    if (state.readyQueue.length === 0 && state.pendingIO.size > 0) {
      // Flush stdout/stderr buffer channels
      await flushIOChannels(state);

      // Wait for the first IO operation to complete
      const { pid, value } = await Promise.race(
        [...state.pendingIO.entries()].map(([pid, entry]) =>
          entry.promise.then(value => ({ pid, value }))
        )
      );

      // Move completed fiber to ready queue
      const entry = state.pendingIO.get(pid)!;
      state.pendingIO.delete(pid);
      state.readyQueue.push({
        state: 'suspended',
        cont: entry.cont,
        resumeValue: value,
        pid,
      });
    }
  }

  // Remove handler frame, flush remaining IO
  await flushIOChannels(state);
  removeHandlerFrame(state);

  // Deadlock detection: live PIDs remain but nothing can wake them
  const blockedCount = state.blockedOnReceive.size;
  const waitingOnChannels = [...state.channels.values()].reduce(
    (n, ch) => n + ch.waitingSenders.length + ch.waitingReceivers.length, 0
  );
  if (blockedCount > 0 || waitingOnChannels > 0) {
    // Deadlock — some fibers will never wake
  }

  return mainResult;
}
```
```

**Handler frame installation**: The scheduler installs a `VMHandlerFrame` with dummy closures for all recognized effect names. The `PERFORM` opcode only checks `handlers.has(opName)` to decide whether to capture a continuation and throw `EffectSignal` — the closures themselves are never invoked because the scheduler catches the `EffectSignal` before `runWithHandler` would dispatch to them.

```typescript
function installHandlerFrame(state: SchedulerState): void {
  const dummyClosure: VMClosure = {
    kind: 'vm-closure',
    proto: { name: '<scheduler>', arity: 0, upvalues: [], chunk: emptyChunk },
    upvalues: [],
  };

  const handlers = new Map<string, { closure: VMClosure; arity: number }>();
  const effectNames = [
    // Layer 1: Fibers + Channels
    'fiber-spawn', 'fiber-yield', 'fiber-self',
    'chan-new', 'chan-send', 'chan-recv', 'chan-close',
    // Layer 2: Actors
    'actor-send', 'actor-receive', 'actor-receive-match',
    'actor-monitor', 'actor-link', 'actor-unlink', 'actor-trap-exit',
    // Layer 3: IO (intercepts existing IO effects + new ones)
    'print', 'println', 'read-line', 'read-file', 'write-file',
    'io-stdout', 'io-stderr', 'io-stdin',
    'io-read-file', 'io-write-file', 'io-open-file',
  ];
  for (const name of effectNames) {
    handlers.set(name, { closure: dummyClosure, arity: 1 });
  }

  const handlerFrame: VMHandlerFrame = {
    handlers,
    returnHandler: undefined,
    unwindFrameCount: state.vm.frameCount,
    unwindStackTop: state.vm.stackTop,
    captureFrameStart: state.vm.frameCount,
    captureStackStart: state.vm.stackTop,
  };

  state.vm.handlerStack.push(handlerFrame);
}
```

**Effect dispatch** (`handleConcurrencyEffect`):

```typescript
function handleConcurrencyEffect(state: SchedulerState, signal: EffectSignal): void {
  const { opName, args, continuation } = signal;

  switch (opName) {
    case 'fiber-spawn': {
      const thunk = args[0] as VMClosure;
      const pid = state.nextPid++;
      state.livePids.add(pid);
      state.mailboxes.set(pid, []);
      state.readyQueue.push({ state: 'new', closure: thunk, pid });
      // Resume parent with the new PID
      enqueueResumed(state, state.currentPid, continuation, { kind: 'pid', id: pid });
      break;
    }

    case 'fiber-yield': {
      // Save current fiber to end of queue
      enqueueResumed(state, state.currentPid, continuation, NIL);
      break;
    }

    case 'fiber-self': {
      // Resume immediately (add to front of queue for fairness)
      enqueueResumed(state, state.currentPid, continuation, { kind: 'pid', id: state.currentPid });
      break;
    }

    case 'chan-new': {
      const capacity = args[0] as number;
      const id = state.nextChannelId++;
      const ch: ChannelState = {
        buffer: [], capacity, closed: false,
        waitingSenders: [], waitingReceivers: [],
      };
      state.channels.set(id, ch);
      enqueueResumed(state, state.currentPid, continuation, {
        kind: 'channel', id, buffer: ch.buffer, capacity, closed: false,
      });
      break;
    }

    case 'chan-send': {
      const ch = args[0] as VMChannel;
      const value = args[1];
      const chanState = state.channels.get(ch.id)!;

      if (chanState.closed) {
        throw new Error('Cannot send on closed channel');
      }

      if (chanState.waitingReceivers.length > 0) {
        // Hand off directly to waiting receiver
        const receiver = chanState.waitingReceivers.shift()!;
        enqueueResumed(state, receiver.pid, receiver.cont, value);
        enqueueResumed(state, state.currentPid, continuation, NIL);
      } else if (chanState.buffer.length < chanState.capacity) {
        // Buffer the value
        chanState.buffer.push(value);
        enqueueResumed(state, state.currentPid, continuation, NIL);
      } else {
        // Block sender
        chanState.waitingSenders.push({
          pid: state.currentPid, value, cont: continuation,
        });
      }
      break;
    }

    case 'chan-recv': {
      const ch = args[0] as VMChannel;
      const chanState = state.channels.get(ch.id)!;

      if (chanState.buffer.length > 0) {
        const value = chanState.buffer.shift()!;
        // If a sender was blocked, accept their value into buffer
        if (chanState.waitingSenders.length > 0) {
          const sender = chanState.waitingSenders.shift()!;
          chanState.buffer.push(sender.value);
          enqueueResumed(state, sender.pid, sender.cont, NIL);
        }
        enqueueResumed(state, state.currentPid, continuation, value);
      } else if (chanState.waitingSenders.length > 0) {
        // Unbuffered: hand off directly
        const sender = chanState.waitingSenders.shift()!;
        enqueueResumed(state, sender.pid, sender.cont, NIL);
        enqueueResumed(state, state.currentPid, continuation, sender.value);
      } else if (chanState.closed) {
        enqueueResumed(state, state.currentPid, continuation, NIL);
      } else {
        // Block receiver
        chanState.waitingReceivers.push({
          pid: state.currentPid, cont: continuation,
        });
      }
      break;
    }

    case 'chan-close': {
      const ch = args[0] as VMChannel;
      const chanState = state.channels.get(ch.id)!;
      chanState.closed = true;
      ch.closed = true;
      // Wake all blocked receivers with nil
      for (const receiver of chanState.waitingReceivers) {
        enqueueResumed(state, receiver.pid, receiver.cont, NIL);
      }
      chanState.waitingReceivers.length = 0;
      enqueueResumed(state, state.currentPid, continuation, NIL);
      break;
    }

    case 'actor-send': {
      const targetPid = (args[0] as VMPid).id;
      const msg = args[1];

      // Enqueue in target's mailbox
      const mailbox = state.mailboxes.get(targetPid);
      if (mailbox) mailbox.push(msg);

      // Check if target is blocked on receive
      const blocked = state.blockedOnReceive.get(targetPid);
      if (blocked) {
        if (blocked.predicate) {
          // Selective receive: test predicate against new message
          const matches = callPredicate(state, blocked.predicate, msg);
          if (matches) {
            // Remove message from mailbox, wake receiver
            const idx = mailbox!.indexOf(msg);
            if (idx !== -1) mailbox!.splice(idx, 1);
            state.blockedOnReceive.delete(targetPid);
            enqueueResumed(state, targetPid, blocked.cont, msg);
          }
          // If predicate didn't match, message stays in mailbox, receiver stays blocked
        } else {
          // Non-selective: dequeue first message, wake receiver
          const first = mailbox!.shift()!;
          state.blockedOnReceive.delete(targetPid);
          enqueueResumed(state, targetPid, blocked.cont, first);
        }
      }

      enqueueResumed(state, state.currentPid, continuation, NIL);
      break;
    }

    case 'actor-receive': {
      const mailbox = state.mailboxes.get(state.currentPid)!;
      if (mailbox.length > 0) {
        const msg = mailbox.shift()!;
        enqueueResumed(state, state.currentPid, continuation, msg);
      } else {
        // Block until message arrives
        state.blockedOnReceive.set(state.currentPid, { cont: continuation });
      }
      break;
    }

    case 'actor-receive-match': {
      const predicate = args[0] as VMClosure;
      const mailbox = state.mailboxes.get(state.currentPid)!;

      // Scan mailbox for first match
      for (let i = 0; i < mailbox.length; i++) {
        const matches = callPredicate(state, predicate, mailbox[i]);
        if (matches) {
          const msg = mailbox.splice(i, 1)[0];
          enqueueResumed(state, state.currentPid, continuation, msg);
          return;
        }
      }

      // No match — block with predicate
      state.blockedOnReceive.set(state.currentPid, {
        cont: continuation,
        predicate,
      });
      break;
    }

    case 'actor-monitor': {
      const targetPid = (args[0] as VMPid).id;
      if (!state.monitors.has(targetPid)) {
        state.monitors.set(targetPid, new Set());
      }
      state.monitors.get(targetPid)!.add(state.currentPid);
      enqueueResumed(state, state.currentPid, continuation, NIL);
      break;
    }

    case 'actor-link': {
      const targetPid = (args[0] as VMPid).id;
      if (!state.links.has(state.currentPid)) {
        state.links.set(state.currentPid, new Set());
      }
      if (!state.links.has(targetPid)) {
        state.links.set(targetPid, new Set());
      }
      state.links.get(state.currentPid)!.add(targetPid);
      state.links.get(targetPid)!.add(state.currentPid);
      enqueueResumed(state, state.currentPid, continuation, NIL);
      break;
    }

    case 'actor-unlink': {
      const targetPid = (args[0] as VMPid).id;
      state.links.get(state.currentPid)?.delete(targetPid);
      state.links.get(targetPid)?.delete(state.currentPid);
      enqueueResumed(state, state.currentPid, continuation, NIL);
      break;
    }

    case 'actor-trap-exit': {
      const enabled = args[0];
      if (isTruthy(enabled)) {
        state.trapExit.add(state.currentPid);
      } else {
        state.trapExit.delete(state.currentPid);
      }
      enqueueResumed(state, state.currentPid, continuation, NIL);
      break;
    }

    // --- Layer 3: IO effects ---

    case 'print': {
      // Write to stdout channel buffer, will be flushed async
      const msg = args[0];
      const text = typeof msg === 'number' ? String(msg) :
                   msg.kind === 'string' ? msg.value : formatVMValue(msg);
      const chanState = state.channels.get(state.stdoutChannel)!;
      chanState.buffer.push({ kind: 'string', value: text });
      enqueueResumed(state, state.currentPid, continuation, NIL);
      break;
    }

    case 'println': {
      const msg = args[0];
      const text = typeof msg === 'number' ? String(msg) + '\n' :
                   msg.kind === 'string' ? msg.value + '\n' : formatVMValue(msg) + '\n';
      const chanState = state.channels.get(state.stdoutChannel)!;
      chanState.buffer.push({ kind: 'string', value: text });
      enqueueResumed(state, state.currentPid, continuation, NIL);
      break;
    }

    case 'read-line': {
      // Async readline — park fiber
      const promise = readLineAsync().then(line => ({ kind: 'string', value: line }) as VMValue);
      state.pendingIO.set(state.currentPid, { promise, cont: continuation });
      break;
    }

    case 'read-file':
    case 'io-read-file': {
      const path = args[0];
      if (typeof path === 'number' || path.kind !== 'string') {
        throw new Error('read-file: path must be a string');
      }
      // Async file read — park fiber
      const promise = Bun.file(path.value).text()
        .then(content => ({ kind: 'string', value: content }) as VMValue);
      state.pendingIO.set(state.currentPid, { promise, cont: continuation });
      break;
    }

    case 'write-file':
    case 'io-write-file': {
      const path = args[0];
      const content = args[1];
      if (typeof path === 'number' || path.kind !== 'string') {
        throw new Error('write-file: path must be a string');
      }
      if (typeof content === 'number' || content.kind !== 'string') {
        throw new Error('write-file: content must be a string');
      }
      const promise = Bun.write(path.value, content.value)
        .then(() => NIL as VMValue);
      state.pendingIO.set(state.currentPid, { promise, cont: continuation });
      break;
    }

    case 'io-stdout': {
      enqueueResumed(state, state.currentPid, continuation, {
        kind: 'channel', id: state.stdoutChannel, buffer: [], capacity: 64, closed: false,
      });
      break;
    }

    case 'io-stderr': {
      enqueueResumed(state, state.currentPid, continuation, {
        kind: 'channel', id: state.stderrChannel, buffer: [], capacity: 64, closed: false,
      });
      break;
    }

    case 'io-stdin': {
      enqueueResumed(state, state.currentPid, continuation, {
        kind: 'channel', id: state.stdinChannel, buffer: [], capacity: 0, closed: false,
      });
      break;
    }

    case 'io-open-file': {
      const path = args[0];
      const mode = args[1];
      if (typeof path === 'number' || path.kind !== 'string') {
        throw new Error('open-file: path must be a string');
      }
      const modeStr = (typeof mode !== 'number' && mode.kind === 'string') ? mode.value : 'r';
      // Create a channel and spawn a system fiber to stream the file
      const chId = state.nextChannelId++;
      const ch: ChannelState = {
        buffer: [], capacity: 16, closed: false,
        waitingSenders: [], waitingReceivers: [],
      };
      state.channels.set(chId, ch);

      // Start async streaming in a system fiber (not user-visible)
      if (modeStr === 'r') {
        streamFileRead(state, chId, path.value);
      }
      // Write mode channels are drained by a flush routine

      enqueueResumed(state, state.currentPid, continuation, {
        kind: 'channel', id: chId, buffer: [], capacity: 16, closed: false,
      });
      break;
    }
  }
}
```

**Fiber exit handling** (`handleFiberExit`):

```typescript
function handleFiberExit(
  state: SchedulerState,
  pid: FiberId,
  exitType: 'normal' | 'crash',
  error: any,
): void {
  state.livePids.delete(pid);
  const reason = exitType === 'normal' ? 'normal' : String(error?.message ?? error);

  // Notify monitors: deliver (Down pid reason) message
  const monitorSet = state.monitors.get(pid);
  if (monitorSet) {
    for (const monitorPid of monitorSet) {
      if (state.livePids.has(monitorPid)) {
        const downMsg: VMValue = {
          kind: 'adt-value',
          constructor: 'Down',
          args: [{ kind: 'pid', id: pid }, { kind: 'string', value: reason }],
        };
        deliverMessage(state, monitorPid, downMsg);
      }
    }
    state.monitors.delete(pid);
  }

  // Process links
  const linkedSet = state.links.get(pid);
  if (linkedSet) {
    for (const linkedPid of linkedSet) {
      state.links.get(linkedPid)?.delete(pid);

      if (!state.livePids.has(linkedPid)) continue;

      if (exitType === 'normal') continue;  // Normal exit doesn't propagate through links

      if (state.trapExit.has(linkedPid)) {
        // Deliver (Exit pid reason) as a message instead of killing
        const exitMsg: VMValue = {
          kind: 'adt-value',
          constructor: 'Exit',
          args: [{ kind: 'pid', id: pid }, { kind: 'string', value: reason }],
        };
        deliverMessage(state, linkedPid, exitMsg);
      } else {
        // Kill linked fiber — remove from ready queue and recurse
        killFiber(state, linkedPid, reason);
      }
    }
    state.links.delete(pid);
  }

  // Clean up
  state.mailboxes.delete(pid);
  state.blockedOnReceive.delete(pid);
}
```

### 4. `src/concurrent/scheduler-core.ts` — Shared Scheduler Logic

The core scheduler logic — state management, effect dispatch, channel operations, actor mailboxes, supervision — is **shared** between both backends. Only the fiber execution and continuation mechanics differ.

```typescript
// src/concurrent/scheduler-core.ts

// Backend-agnostic fiber representation
export type Fiber<Cont> =
  | { state: 'new'; thunk: unknown; pid: number }   // thunk type varies by backend
  | { state: 'suspended'; cont: Cont; resumeValue: unknown; pid: number };

// Backend-agnostic scheduler state
export type SchedulerState<Cont> = {
  readyQueue: Fiber<Cont>[];
  currentPid: number;
  nextPid: number;

  // Channels
  channels: Map<number, ChannelState<Cont>>;
  nextChannelId: number;

  // Actors
  mailboxes: Map<number, unknown[]>;
  blockedOnReceive: Map<number, { cont: Cont; predicate?: unknown }>;

  // Supervision
  monitors: Map<number, Set<number>>;
  links: Map<number, Set<number>>;
  trapExit: Set<number>;
  livePids: Set<number>;

  // Async IO
  pendingIO: Map<number, { promise: Promise<unknown>; cont: Cont }>;
  stdoutChannel: number;
  stderrChannel: number;
  stdinChannel: number;
};

export type ChannelState<Cont> = {
  buffer: unknown[];
  capacity: number;
  closed: boolean;
  waitingSenders: Array<{ pid: number; value: unknown; cont: Cont }>;
  waitingReceivers: Array<{ pid: number; cont: Cont }>;
};

// Shared effect dispatch — called by both backends with backend-specific wrappers
export function dispatchEffect<Cont>(
  state: SchedulerState<Cont>,
  opName: string,
  args: unknown[],
  continuation: Cont,
  enqueue: (state: SchedulerState<Cont>, pid: number, cont: Cont, value: unknown) => void,
): void {
  // Same switch/case logic as shown in section 3, but parameterized
  // over the continuation type via the enqueue callback
  switch (opName) {
    case 'fiber-spawn': // ...
    case 'fiber-yield': // ...
    case 'chan-send':    // ...
    case 'chan-recv':    // ...
    case 'actor-send':  // ...
    // ... all effect cases
  }
}

// Shared fiber exit handling
export function handleFiberExit<Cont>(
  state: SchedulerState<Cont>,
  pid: number,
  exitType: 'normal' | 'crash',
  error: unknown,
  deliverMessage: (state: SchedulerState<Cont>, pid: number, msg: unknown) => void,
): void {
  // Same logic as shown in section 3
}
```

Each backend provides:
1. A `Cont` type (VM: `VMContinuation`, tree-walker: `Continuation`)
2. An `enqueue` function that wraps a continuation + value into a `Fiber<Cont>`
3. A `runFiber` function that executes a fiber and returns the next suspension
4. A `deliverMessage` function that formats and delivers actor messages

### 5. `src/core/scheduler.ts` — Tree-Walker Trampoline Scheduler

The tree-walker scheduler uses the same trampoline pattern as the VM scheduler, but drives **generators** instead of catching exceptions.

```typescript
// src/core/scheduler.ts

import type { RuntimeValue, Continuation, EffectSuspension, Env } from './types';
import type { SchedulerState, Fiber } from '../concurrent/scheduler-core';
import { dispatchEffect, handleFiberExit } from '../concurrent/scheduler-core';

type TWCont = Continuation;  // Generator-based continuation
type TWFiber = Fiber<TWCont>;
type EvalGen = Generator<EffectSuspension, RuntimeValue, RuntimeValue>;

export async function runFibersTW(
  env: Env,
  body: RuntimeValue,  // a closure
): Promise<RuntimeValue> {
  const state: SchedulerState<TWCont> = { /* initialize */ };
  let mainResult: RuntimeValue = { kind: 'nil' };

  // Start main fiber by calling the body thunk as a generator
  const mainGen = applyAsGenerator(body, []);
  let genResult = mainGen.next();

  // Process initial fiber
  if (genResult.done) {
    mainResult = genResult.value;
    handleFiberExit(state, 0, 'normal', null, deliverMessage);
  } else {
    // First suspension — dispatch the effect
    dispatchEffect(state, genResult.value.opName, genResult.value.args,
      wrapGenerator(mainGen), enqueue);
  }

  // Async trampoline loop
  while (state.readyQueue.length > 0 || state.pendingIO.size > 0) {
    // SYNC PHASE: run ready fibers
    while (state.readyQueue.length > 0) {
      const fiber = state.readyQueue.shift()!;
      state.currentPid = fiber.pid;

      try {
        let gen: EvalGen;
        if (fiber.state === 'new') {
          gen = applyAsGenerator(fiber.thunk as RuntimeValue, []);
        } else {
          // Resume the continuation — returns a new generator
          gen = fiber.cont.resume(fiber.resumeValue as RuntimeValue);
        }

        // Drive the generator until it yields (effect) or completes
        let result = gen.next();
        while (!result.done) {
          const suspension = result.value;

          // Check if this is a concurrency/IO effect we handle
          if (isConcurrencyEffect(suspension.opName)) {
            // Wrap the remaining generator as a continuation
            const cont: TWCont = wrapGenerator(gen);
            dispatchEffect(state, suspension.opName, suspension.args, cont, enqueue);
            break;  // Fiber yielded — go to next fiber
          } else {
            // Not a concurrency effect — try default handlers (e.g., type system side effects)
            const defaultResult = tryDefaultHandler(suspension);
            if (defaultResult !== undefined) {
              result = gen.next(defaultResult);
              continue;
            }
            throw new Error(`Unhandled effect: ${suspension.opName}`);
          }
        }

        if (result.done) {
          // Fiber completed normally
          if (fiber.pid === 0) mainResult = result.value;
          handleFiberExit(state, fiber.pid, 'normal', null, deliverMessage);
        }
      } catch (e) {
        handleFiberExit(state, fiber.pid, 'crash', e, deliverMessage);
      }
    }

    // DRAIN PHASE: await pending IO
    if (state.readyQueue.length === 0 && state.pendingIO.size > 0) {
      await flushIOChannels(state);
      const { pid, value } = await Promise.race(
        [...state.pendingIO.entries()].map(([pid, entry]) =>
          entry.promise.then(value => ({ pid, value }))
        )
      );
      state.pendingIO.delete(pid);
      enqueue(state, pid, state.pendingIO.get(pid)!.cont,  value);
    }
  }

  return mainResult;
}

// Wrap a suspended generator as a Continuation
function wrapGenerator(gen: EvalGen): TWCont {
  return {
    kind: 'continuation',
    resume: function* (value: RuntimeValue): EvalGen {
      let r = gen.next(value);
      while (!r.done) {
        const resumeValue: RuntimeValue = yield r.value;
        r = gen.next(resumeValue);
      }
      return r.value;
    }
  };
}
```

**Key difference from the VM scheduler**: Instead of catching `EffectSignal` exceptions, the tree-walker scheduler inspects the `IteratorResult` from `gen.next()`. If `result.done === false`, the yielded value is an `EffectSuspension` — the scheduler checks `opName` and dispatches. If `result.done === true`, the fiber completed.

**Non-concurrency effects**: When the generator yields an effect that isn't a concurrency/IO effect (e.g., a user-defined effect handled by an outer `handle` block), the scheduler tries default handlers. If none match, it throws. User `handle` blocks within fibers work naturally — `evalExprGen` handles them internally before the suspension reaches the scheduler.

### 6. `src/core/types.ts` — New Runtime Value Kinds

Add `Pid` and `Channel` to the tree-walker's `RuntimeValue` union:

```typescript
export type RuntimeValue =
  // ... existing kinds ...
  | { kind: 'pid'; id: number }
  | { kind: 'channel'; id: number; capacity: number; closed: boolean };
```

Update `formatValue` in `interpreter.ts` for the new kinds.

### 7. `src/core/interpreter.ts` — Tree-Walker Integration

Register `run-fibers` as a builtin in the tree-walker:

```typescript
// In builtins array:
{ name: 'run-fibers', fn: (args: RuntimeValue[]) => {
  // This is async — needs special handling at call site
  // Return a sentinel that the CLI awaits
  return runFibersTW(currentEnv, args[0]!);
}}
```

Export `applyAsGenerator` (convert a closure + args into an `EvalGen`):

```typescript
export function applyAsGenerator(fn: RuntimeValue, args: RuntimeValue[]): EvalGen {
  if (fn.kind !== 'function') throw new Error('Expected function');
  let fnEnv = fn.closure;
  for (let i = 0; i < fn.params.length; i++) {
    fnEnv = Environment.define(fnEnv, fn.params[i]!, args[i] ?? { kind: 'nil' });
  }
  return evalSequenceGen({ env: fnEnv }, fn.body);
}
```

### 8. `src/vm/builtins.ts` — `run-fibers` Builtin (VM)

Since `runFibers` is now `async`, the builtin uses Bun's top-level await support to block until the scheduler completes:

```typescript
builtin('run-fibers', (args, vm) => {
  const closure = args[0];
  if (typeof closure === 'number' || closure.kind !== 'vm-closure') {
    throw new Error('run-fibers: argument must be a closure');
  }
  // Block on the async scheduler — Bun supports top-level await
  // The VM call stack is unwound at this point (builtin returns to CALL handler)
  // so blocking here is safe
  return runFibersSync(vm!, closure);
});
```

`runFibersSync` is a wrapper that uses Bun's synchronous execution of async functions at the top level. Alternatively, `run-fibers` could be a special opcode or handled at the CLI level where `await` is available:

```typescript
// In index.ts CLI entry point:
if (useVM) {
  const chunk = compileProgram(exprs);
  const vm = createVM();
  const result = interpret(vm, chunk);
  // If result triggers run-fibers, the async scheduler runs via top-level await
}
```

The cleanest approach: `run-fibers` returns a `VMPromise` sentinel that the CLI `await`s. This keeps the VM synchronous internally while the outermost caller handles the async boundary.

### 9. `src/types/type-builtins.ts` — Type System

Add `Pid` and `Channel` as built-in types. Add type scheme for `run-fibers`:

```typescript
// In createBuiltinEnv():
env.addType('Pid', { kind: 'constructor', name: 'Pid', args: [] });
env.addType('Channel', { kind: 'constructor', name: 'Channel', args: [tvar('a')] });

env.addScheme('run-fibers', forall(['a'],
  fn([fn([], unit)], tvar('a'))
));
```

### 10. `stdlib/concurrent.mar` — Fiber + Channel API

```lisp
(effect Concurrent
  (fiber-spawn (-> (-> Unit Unit) Pid))
  (fiber-yield (-> Unit Unit))
  (fiber-self  (-> Unit Pid)))

(effect Channel
  (chan-new   (-> Int Channel))
  (chan-send  (-> Channel a Unit))
  (chan-recv  (-> Channel a))
  (chan-close (-> Channel Unit)))

(define spawn     (lambda (f) (perform fiber-spawn f)))
(define yield     (lambda () (perform fiber-yield)))
(define self      (lambda () (perform fiber-self)))
(define chan-new   (lambda (cap) (perform chan-new cap)))
(define chan-send  (lambda (ch v) (perform chan-send ch v)))
(define chan-recv  (lambda (ch) (perform chan-recv ch)))
(define chan-close (lambda (ch) (perform chan-close ch)))
(define run       (lambda (f) (run-fibers f)))
```

### 11. `stdlib/actor.mar` — Actor API

```lisp
(data Signal
  (Down Pid String)
  (Exit Pid String))

(effect Actor
  (actor-send          (-> Pid a Unit))
  (actor-receive       (-> Unit a))
  (actor-receive-match (-> (-> a Bool) a))
  (actor-monitor       (-> Pid Unit))
  (actor-link          (-> Pid Unit))
  (actor-unlink        (-> Pid Unit))
  (actor-trap-exit     (-> Bool Unit)))

(define send          (lambda (pid msg) (perform actor-send pid msg)))
(define receive       (lambda () (perform actor-receive)))
(define receive-match (lambda (pred) (perform actor-receive-match pred)))
(define monitor       (lambda (pid) (perform actor-monitor pid)))
(define link          (lambda (pid) (perform actor-link pid)))
(define unlink        (lambda (pid) (perform actor-unlink pid)))
(define trap-exit     (lambda (b) (perform actor-trap-exit b)))
```

### 12. `stdlib/io.mar` — Async IO API

```lisp
(effect IO
  (io-stdout    (-> Unit Channel))
  (io-stderr    (-> Unit Channel))
  (io-stdin     (-> Unit Channel))
  (io-read-file  (-> String String))
  (io-write-file (-> String String Unit))
  (io-open-file  (-> String String Channel)))

(define stdout     (lambda () (perform io-stdout)))
(define stderr     (lambda () (perform io-stderr)))
(define stdin      (lambda () (perform io-stdin)))
(define read-file  (lambda (path) (perform io-read-file path)))
(define write-file (lambda (path content) (perform io-write-file path content)))
(define open-file  (lambda (path mode) (perform io-open-file path mode)))
```

### 13. `src/vm/index.ts` — Updated Exports

Re-export `runFibers`, `VMPid`, `VMChannel`, `EffectSignal`, `resumeContinuation`.

### 14. `spec/concurrent.test.ts` — Tests

See [Tests](#tests-specconcurrenttestts) section.

---

## Edge Cases and Design Decisions

### Deadlock Detection

If the ready queue is empty AND `pendingIO` is empty but `livePids` is non-empty:
- Some fibers are blocked on channels or receive with no possible wake source and no pending IO that could wake them
- The scheduler breaks the loop and returns the main fiber's result (if it completed) or throws a deadlock error
- Note: if `pendingIO` is non-empty, it's NOT a deadlock — the scheduler awaits the IO and may wake blocked fibers

### Main Fiber Result

The return value of `run-fibers` is the return value of the main fiber (PID 0). If the main fiber crashes, the error propagates. Spawned fibers' return values are discarded unless communicated via channels or messages.

### Fiber Identity

Every fiber (including the main one) gets a PID. The main fiber is PID 0. PIDs are monotonically increasing integers, never reused.

### Predicate Evaluation in Selective Receive

`callPredicate` calls the predicate closure synchronously within the scheduler. The predicate **must not** perform effects — it's a pure function. If a predicate performs an effect, the behavior is undefined (the EffectSignal would propagate to the scheduler's catch, likely causing confusion). This is a documented limitation.

### Channel vs. Mailbox

Channels and mailboxes are independent mechanisms:
- Channels are explicit, typed, shared objects (CSP style)
- Mailboxes are per-actor, implicit, untyped queues (Erlang style)
- Both can be used in the same program

### Nested `run-fibers`

Calling `run-fibers` inside a fiber creates a nested scheduler with its own ready queue, channels, and mailboxes. The outer fiber blocks until the inner `run-fibers` completes. PIDs are not shared between nested schedulers.

---

## Tests (`spec/concurrent.test.ts`)

All tests run on **both backends** (VM and tree-walker). Test helper:

```typescript
const backends = [
  { name: 'VM', run: (source: string) => runWithVM(source) },
  { name: 'Tree-walker', run: (source: string) => runWithTreeWalker(source) },
];

for (const backend of backends) {
  describe(`Concurrency (${backend.name})`, () => {
    // ... all test cases ...
  });
}
```

### Fiber Tests

| Test | Description |
|------|-------------|
| `spawn returns pid` | Spawn a fiber, verify the return is a PID value |
| `yield interleaves` | Two fibers yield back and forth, verify interleaved execution order |
| `self returns correct pid` | Fiber checks its own PID matches what spawn returned to parent |
| `multiple fibers run to completion` | Spawn N fibers, all complete, main fiber collects results via channel |
| `fiber crash doesn't kill main` | Spawned fiber throws, main fiber continues (no links) |

### Channel Tests

| Test | Description |
|------|-------------|
| `unbuffered send/recv synchronize` | Send blocks until recv, recv blocks until send |
| `buffered channel` | Send doesn't block until buffer full |
| `close wakes receivers` | Blocked receivers get nil on close |
| `blocked sender wakes on recv` | Sender blocked on full channel resumes when receiver dequeues |
| `multiple producers, one consumer` | Fan-in pattern works correctly |

### Actor Tests

| Test | Description |
|------|-------------|
| `send/receive basic` | Send message, receive it |
| `selective receive` | `receive-match` skips non-matching messages, returns first match |
| `selective receive preserves order` | Skipped messages remain in mailbox in original order |
| `receive blocks until message` | Fiber blocks on empty mailbox, wakes when message arrives |

### Supervision Tests

| Test | Description |
|------|-------------|
| `monitor delivers Down` | Monitored actor crashes, monitor gets `(Down pid reason)` |
| `monitor normal exit` | Monitored actor exits normally, monitor gets `(Down pid "normal")` |
| `link propagates crash` | Linked actor crashes, other actor also dies |
| `trap-exit converts crash to message` | With trap-exit, linked crash delivers `(Exit pid reason)` |
| `unlink prevents propagation` | After unlink, crash doesn't propagate |
| `supervisor restart pattern` | Supervisor with trap-exit restarts crashed worker |

### Async IO Tests

| Test | Description |
|------|-------------|
| `print inside run-fibers` | `println` works inside fibers, output is flushed |
| `read-file async` | File read parks fiber, other fibers continue running, reader wakes with content |
| `write-file async` | File write parks fiber, completes asynchronously |
| `concurrent file reads` | 3 fibers read different files concurrently, all complete |
| `stdout channel` | `io/stdout` returns a channel, `chan-send` writes to stdout |
| `stdin channel` | `io/stdin` returns a channel, `chan-recv` reads a line |
| `open-file streaming` | `io/open-file` returns a channel that yields lines, closes at EOF |
| `existing IO effects in fibers` | `println`/`read-file` effects work inside `run-fibers` with async behavior |

### Edge Case Tests

| Test | Description |
|------|-------------|
| `deadlock detection` | Two fibers blocked on each other with no pending IO, scheduler detects and terminates |
| `main fiber result returned` | Return value of main fiber is the return value of run-fibers |
| `nested run-fibers` | Inner run-fibers completes independently |
| `IO outside run-fibers` | `println`/`read-file` still work synchronously via `tryDefaultHandler` |

---

## Verification

```bash
# Run concurrency tests (both backends)
bun test spec/concurrent.test.ts

# Run all tests (ensure nothing broke)
bun test

# Manual test — VM
bun index.ts run examples/concurrent.mar --vm

# Manual test — tree-walker
bun index.ts run examples/concurrent.mar
```

Example verification program:

```lisp
;; examples/concurrent.mar
(import "std:concurrent" as conc)
(import "std:actor" as actor)

;; Test 1: Basic fiber + channel
(conc/run (lambda ()
  (let ((ch (conc/chan-new 0)))
    (conc/spawn (lambda ()
      (conc/chan-send ch 42)))
    (println (conc/chan-recv ch)))))
;; Expected: 42

;; Test 2: Actor send/receive
(conc/run (lambda ()
  (let ((me (conc/self)))
    (conc/spawn (lambda ()
      (actor/send me "hello")))
    (println (actor/receive)))))
;; Expected: hello

;; Test 3: Supervision
(conc/run (lambda ()
  (actor/trap-exit #t)
  (let ((w (conc/spawn (lambda ()
              (error "boom")))))
    (actor/link w)
    (conc/yield)
    (match (actor/receive)
      ((Exit pid reason) (println (+ "caught: " reason)))))))
;; Expected: caught: boom

;; Test 4: Concurrent async file reads
(conc/run (lambda ()
  (let ((ch (conc/chan-new 3)))
    (conc/spawn (lambda () (conc/chan-send ch (io/read-file "a.txt"))))
    (conc/spawn (lambda () (conc/chan-send ch (io/read-file "b.txt"))))
    (conc/spawn (lambda () (conc/chan-send ch (io/read-file "c.txt"))))
    (println (conc/chan-recv ch))
    (println (conc/chan-recv ch))
    (println (conc/chan-recv ch)))))
;; Expected: contents of a.txt, b.txt, c.txt (order depends on which read finishes first)

;; Test 5: IO channels
(conc/run (lambda ()
  (let ((out (io/stdout)))
    (conc/chan-send out "hello from channel\n"))))
;; Expected: hello from channel

;; Test 6: println inside run-fibers (backward compat)
(conc/run (lambda ()
  (conc/spawn (lambda () (println "fiber 1")))
  (conc/spawn (lambda () (println "fiber 2")))
  (println "main")))
;; Expected: main, fiber 1, fiber 2 (order may vary)
```

---

## Risks & Mitigations

| Risk | Likelihood | Mitigation |
|------|------------|------------|
| EffectSignal interaction with handler stack | High | Careful handler frame management; restore handler stack after each fiber resume |
| One-shot continuation reuse bugs | Medium | Verify `cont.resumed` flag is always checked; test double-resume throws |
| Selective receive performance | Low | Linear scan is O(n) per receive; acceptable for initial impl, optimize later if needed |
| Deadlock in nested run-fibers | Medium | Document that nested schedulers are isolated; test nested scenarios |
| Predicate effects in receive-match | Low | Document as undefined behavior; consider runtime check |
| Handler frame leak on fiber crash | High | Ensure handleFiberExit cleans up handler stack to pre-fiber state |
| Async/sync boundary for `run-fibers` | High | `run-fibers` returns a Promise; CLI entry point must `await` it. Test both `--vm` and REPL paths. |
| Promise rejection in IO | Medium | Wrap all Bun async calls in try/catch; deliver errors as fiber crashes, not unhandled rejections |
| Stdout flush ordering | Low | Buffer channel preserves insertion order; flush drains sequentially |

---

## Non-Goals (Explicit)

- **OS thread parallelism**: This is cooperative concurrency on a single thread
- **Distributed actors**: No cross-process messaging
- **Typed channels/mailboxes**: Channels and mailboxes are dynamically typed for now
- **Preemptive scheduling**: All scheduling is cooperative (fibers yield voluntarily)
- **Work-stealing**: Single ready queue, FIFO scheduling
- **Select/alt for channels**: Multiplexing across multiple channels is future work
- **Network IO**: TCP/UDP/HTTP is future work built on the same async IO pattern

---

## Success Criteria

1. All concurrency features work identically on **both** the VM and tree-walker backends
2. Fibers interleave correctly with cooperative scheduling
3. Unbuffered and buffered channels work with proper blocking/waking semantics
4. Actor send/receive works, including selective receive
5. Monitor and link supervision delivers correct signals
6. Async IO: file reads park fibers without blocking others; concurrent reads complete in parallel
7. Existing IO effects (`println`, `read-file`) work both inside and outside `run-fibers`
8. Existing tests pass unchanged on both backends
9. Counter actor example produces correct output
10. Ping-pong example runs 1000 rounds without stack overflow (uses fiber scheduling, not recursion depth)
11. Concurrent file processing example reads 3 files in parallel
12. Stdlib files (`concurrent.mar`, `actor.mar`, `io.mar`) work unmodified on both backends
