# Concurrency System for Marlang

**Type:** Feature Spec
**Status:** Implemented (refactored to always-on scheduler)

> **Note:** This spec was originally written for the two-tier scheduler model
> (`run-fibers` / `run-fibers-parallel`). The architecture has been refactored
> to an **always-on per-core scheduler** that starts automatically at program
> startup. All effects (IO, TCP, HTTP, fibers, channels, actors) now work at
> the top level — no wrapper functions needed. The `run-fibers` and
> `run-fibers-parallel` builtins have been removed. Code examples in this
> document may still reference the old wrappers; the actual behavior is that
> effect operations work directly at the top level.

## Problem

Marlang needs concurrency primitives. Programs running single-threaded with no way to express concurrent computations, communicate between independent tasks, or build fault-tolerant systems cannot support network servers, interactive programs, or parallel workloads.

## Solution

Effect-based concurrency built on the existing `PERFORM`/continuation machinery. Fibers are lightweight green threads whose suspension/resumption is powered by the VM's one-shot continuations. Channels provide CSP-style communication. The actor model (PIDs, mailboxes, supervision, selective receive) is layered on top. Two scheduler modes are provided:

| Mode | Entry Point | Threads | IO | TCP |
|------|------------|---------|-----|-----|
| Single-threaded | `run-fibers` | 1 | Synchronous (blocking) | Not supported |
| Multi-threaded | `run-fibers-parallel` | N (configurable) | Async (tokio) | Supported |

No new VM opcodes are needed. The stdlib files (`concurrent.mar`, `actor.mar`, `io.mar`, `net.mar`) use `perform`, which dispatches to the scheduler's handler frame.

---

## Architecture Overview

```
┌────────────────────────────────────────────────────────────────┐
│                       User Code (.mar)                         │
├────────────────┬─────────────────┬──────────────┬──────────────┤
│  stdlib/       │  stdlib/        │  stdlib/     │  stdlib/     │
│  concurrent    │  actor.mar      │  io.mar      │  net.mar     │
│  .mar          │  (send, receive │  (print,     │  (connect,   │
│  (spawn, yield │   monitor, link │   read-file  │   listen,    │
│   chan-*)      │   trap-exit)    │   write-file)│   send, recv)│
├────────────────┴─────────────────┴──────────────┴──────────────┤
│                     Scheduler effect handlers                   │
├───────────────────────────────┬─────────────────────────────────┤
│  scheduler.rs                 │  runtime.rs                     │
│  Single-threaded              │  Multi-threaded                 │
│  Synchronous trampoline       │  Work-stealing + tokio          │
│  Blocking IO                  │  Async IO + TCP                 │
├───────────────────────────────┴─────────────────────────────────┤
│  vm.rs — VM core                                                │
│  PERFORM → EffectSignal, resume_continuation                    │
│  try_default_handler (IO fallback outside schedulers)           │
├─────────────────────────────────────────────────────────────────┤
│  object.rs — Pid(u64), Continuation, Closure, Port              │
│  value.rs — NaN-boxed Value                                     │
└─────────────────────────────────────────────────────────────────┘
```

### How the Schedulers Work

Both schedulers share the same approach:

1. Install a handler frame on `vm.handler_stack` with entries for all concurrency/IO effect names
2. Call the body closure to start the main fiber (PID 0)
3. When a fiber performs an effect (e.g., `fiber-spawn`, `chan-send`, `read-file`), `PERFORM` finds the handler frame, captures a one-shot continuation, and returns an `EffectSignal`
4. The scheduler processes the effect: updates queues/channels/mailboxes, enqueues fibers to resume
5. Pick the next ready fiber and resume via `resume_continuation`
6. Repeat until all fibers complete or deadlock is detected

**Single-threaded** (`scheduler.rs`): A simple `while let Some(fiber) = ready_queue.pop_front()` loop. All state is local. IO blocks the entire scheduler.

**Multi-threaded** (`runtime.rs`): N OS threads, each owning a VM instance. Fibers are distributed across threads at spawn time. Cross-thread communication uses `crossbeam-channel`. Work stealing uses `crossbeam-deque`. A `tokio` runtime handles async IO and TCP — fibers are parked while IO is pending, and other fibers (on any thread) keep running.

```
Scheduler                      VM
   │                            │
   │── call_closure(body) ─────►│
   │                            │── runs fiber code...
   │                            │── PERFORM "fiber-spawn"
   │◄── EffectSignal ──────────│   (continuation captured)
   │                            │
   │   [create fiber, enqueue]  │
   │── resume_continuation ────►│
   │   (resume parent w/ PID)   │── runs fiber code...
   │                            │── PERFORM "chan-recv"
   │◄── EffectSignal ──────────│
   │                            │
   │   [check buffer/waiters,   │
   │    block or hand off]      │
   │── resume_continuation ────►│   (run next fiber)
   ...                          ...
```

---

## Layer 1: Green Threads (Fibers) + Channels

### Fiber API (`stdlib/concurrent.mar`)

```lisp
(effect Concurrent
  (fiber-spawn (-> (-> Unit Unit) Pid))
  (fiber-yield (-> Unit Unit))
  (fiber-self  (-> Unit Pid)))

(define spawn   (lambda (f) (perform fiber-spawn f)))
(define yield   (lambda () (perform fiber-yield ())))
(define self    (lambda () (perform fiber-self ())))
(define run     (lambda (f) (run-fibers f)))
```

### Fiber Semantics

| Operation | Behavior |
|-----------|----------|
| `spawn f` | Create a new fiber running thunk `f`. Assign a fresh PID. Add to ready queue. Resume parent with the PID. |
| `yield` | Save current fiber's continuation to the **end** of the ready queue. Switch to next ready fiber. Resume with `nil`. |
| `self` | Resume immediately with current fiber's PID. No context switch. |

Fibers are cooperatively scheduled. Every fiber gets a PID (monotonically increasing `u64`, never reused). The main fiber is PID 0.

### Channel API (`stdlib/concurrent.mar`)

```lisp
(effect Channel
  (chan-new   (-> Int Channel))
  (chan-send  (-> Channel a (Result String Unit)))
  (chan-recv  (-> Channel (Option a)))
  (chan-close (-> Channel Unit)))

(define chan-new   (lambda (cap) (perform chan-new cap)))
(define chan-send  (lambda (ch v) (perform chan-send ch v)))
(define chan-recv  (lambda (ch) (perform chan-recv ch)))
(define chan-close (lambda (ch) (perform chan-close ch)))
```

A throwing wrapper is also provided:

```lisp
;; chan-send! : throws on closed channel instead of returning (Err msg)
(define chan-send! (lambda (ch v)
  (match (perform chan-send ch v)
    ((Ok _) nil)
    ((Err msg) (perform throw (string-append "chan-send: " msg))))))
```

### Channel Semantics

Channels are typed communication pipes with optional buffering.

| Operation | Behavior |
|-----------|----------|
| `chan-new cap` | Create a channel with buffer capacity `cap`. `cap=0` means unbuffered (rendezvous). |
| `chan-send ch val` | Returns `(Ok nil)` on success, `(Err msg)` if channel is closed. If a receiver is waiting, hand off directly and wake receiver. If buffer has space, enqueue value. Otherwise, block sender. |
| `chan-recv ch` | Returns `(Some value)` on success, `None` if channel is closed and empty. If buffer non-empty, dequeue and return. If a sender is blocked, accept their value, wake sender. Otherwise, block receiver. |
| `chan-close ch` | Mark channel as closed. Wake all blocked receivers with `None`. |

**Unbuffered channel (rendezvous) protocol:**
- `chan-send` on empty channel with no waiting receiver → block sender
- `chan-recv` on empty channel with no waiting sender → block receiver
- When both a sender and receiver are waiting on the same channel → hand off value directly, wake both

### Example: Producer-Consumer

```lisp
(define consume (lambda (ch)
  (match (perform chan-recv ch)
    ((None) (perform println "done"))
    ((Some v)
      (begin
        (perform println v)
        (consume ch))))))

(run-fibers (lambda ()
  (let ((ch (perform chan-new 0)))
    (perform fiber-spawn (lambda ()
      (perform chan-send ch 10)
      (perform chan-send ch 20)
      (perform chan-send ch 30)
      (perform chan-close ch)))
    (consume ch))))
;; Output: 10 20 30 done
```

### Example: Fan-Out

```lisp
(define recv! (lambda (ch)
  (match (perform chan-recv ch)
    ((Some v) v)
    ((None) (perform throw "channel closed unexpectedly")))))

(run-fibers (lambda ()
  (let ((jobs (perform chan-new 10))
        (results (perform chan-new 10)))
    (perform fiber-spawn (lambda ()
      (let ((j (recv! jobs)))
        (perform chan-send results (* j j)))))
    (perform fiber-spawn (lambda ()
      (let ((j (recv! jobs)))
        (perform chan-send results (* j j)))))
    (perform fiber-spawn (lambda ()
      (let ((j (recv! jobs)))
        (perform chan-send results (* j j)))))
    (perform chan-send jobs 2)
    (perform chan-send jobs 3)
    (perform chan-send jobs 4)
    (let ((a (recv! results))
          (b (recv! results))
          (c (recv! results)))
      (perform println (+ a (+ b c)))))))
;; Output: 29
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

Selective receive allows an actor to process messages **out of order** based on a predicate:

```lisp
(perform actor-receive-match
  (lambda (m) (match m ((High _) #t) (_ #f))))
```

The scheduler scans the mailbox linearly. When blocking with a predicate, incoming messages are tested against the predicate before deciding to wake the actor. The predicate **must not** perform effects — it is called synchronously within the scheduler.

### Supervision

#### Monitors (unidirectional, non-destructive)

```lisp
(let ((worker (perform fiber-spawn (lambda () (do-work)))))
  (perform actor-monitor worker)
  ;; ... later ...
  (match (perform actor-receive)
    ((Down pid reason) (perform println (string-append "Worker died: " reason)))))
```

- Monitors are unidirectional: monitor watches target, not vice versa
- Target's death delivers `(Down pid reason)` to monitor's mailbox
- `reason` is `"normal"` for clean exit, or the error message for crashes

#### Links (bidirectional, destructive by default)

```lisp
(let ((worker (perform fiber-spawn (lambda () (do-risky-work)))))
  (perform actor-link worker)
  ;; If worker crashes, this actor crashes too
  (do-more-work))
```

- Links are bidirectional: if either side dies, the other is killed
- `trap-exit #t` converts death signals into messages:

```lisp
;; Supervisor pattern
(perform actor-trap-exit #t)
(let ((worker (perform fiber-spawn do-work)))
  (perform actor-link worker)
  (match (perform actor-receive)
    ((Exit pid reason)
      (perform println (string-append "Restarting worker, died with: " reason))
      ;; ... spawn new worker ...
      )))
```

### Example: Counter Actor

```lisp
(data CounterMsg
  (Increment Int)
  (GetCount Pid))
(data CounterReply
  (Count Int))

(define counter (lambda (count)
  (let ((msg (perform actor-receive)))
    (match msg
      ((Increment n) (counter (+ count n)))
      ((GetCount sender)
        (begin
          (perform actor-send sender (Count count))
          (counter count)))))))

(run-fibers (lambda ()
  (let ((c (perform fiber-spawn (lambda () (counter 0))))
        (me (perform fiber-self)))
    (perform actor-send c (Increment 5))
    (perform actor-send c (Increment 3))
    (perform actor-send c (Increment 10))
    (perform actor-send c (GetCount me))
    (let ((reply (perform actor-receive)))
      (match reply
        ((Count n) (perform println n)))))))
;; Output: 18
```

### Example: Ping-Pong

```lisp
(data PingPong (Ping Pid) (Pong Pid))

(define pong (lambda ()
  (let ((msg (perform actor-receive)))
    (match msg
      ((Ping sender)
        (let ((me (perform fiber-self)))
          (perform actor-send sender (Pong me))
          (pong)))
      (_ (pong))))))

(define ping (lambda (partner n)
  (if (<= n 0)
    (perform println "ping-pong done!")
    (let ((me (perform fiber-self)))
      (perform actor-send partner (Ping me))
      (let ((msg (perform actor-receive)))
        (match msg
          ((Pong _) (ping partner (- n 1)))
          (_ nil)))))))

(run-fibers (lambda ()
  (let ((p (perform fiber-spawn pong)))
    (ping p 5))))
;; Output: ping-pong done!
```

---

## Layer 3: IO

IO effects work both inside and outside schedulers, with different behavior:

### IO Effect (`stdlib/io.mar`)

```lisp
(effect IO
  (print (=> (Show a) (-> a Unit)))
  (println (=> (Show a) (-> a Unit)))
  (read-line (-> Unit String))
  (read-file (-> String (Result String String)))
  (write-file (-> String String (Result String Unit))))
```

File operations return `Result` types. Throwing wrappers are provided:

```lisp
(define read-file! (lambda (path)
  (match (perform read-file path)
    ((Ok content) content)
    ((Err msg) (perform throw msg)))))

(define write-file! (lambda (path content)
  (match (perform write-file path content)
    ((Ok _) nil)
    ((Err msg) (perform throw msg)))))
```

### IO Behavior by Context

| Effect | Outside schedulers | Inside `run-fibers` | Inside `run-fibers-parallel` |
|--------|-------------------|--------------------|-----------------------------|
| `print` | `try_default_handler` → sync `print!()` | Scheduler handles → sync `print!()` | Scheduler handles → tokio task |
| `println` | `try_default_handler` → sync `println!()` | Scheduler handles → sync `println!()` | Scheduler handles → tokio task |
| `read-line` | `try_default_handler` → blocking `stdin().read_line()` | Scheduler handles → blocking `stdin().read_line()` | Scheduler handles → tokio `spawn_blocking` |
| `read-file` | `try_default_handler` → blocking `fs::read_to_string()` | Scheduler handles → blocking `fs::read_to_string()` | Scheduler handles → `tokio::fs::read_to_string()`, fiber is parked |
| `write-file` | `try_default_handler` → blocking `fs::write()` | Scheduler handles → blocking `fs::write()` | Scheduler handles → `tokio::fs::write()`, fiber is parked |

Key point: existing programs that use `println` and `read-file` effects get async behavior for free when wrapped in `run-fibers-parallel`, with zero code changes.

---

## Layer 4: TCP Networking

TCP operations are effects handled only by the multi-threaded scheduler. Attempting TCP effects inside `run-fibers` returns an error directing the user to `run-fibers-parallel`.

### TCP API (`stdlib/net.mar`)

```lisp
(effect Tcp
  (tcp-connect (-> String Int (Result String TcpSocket)))
  (tcp-listen  (-> Int (Result String TcpListener)))
  (tcp-accept  (-> TcpListener (Option TcpSocket)))
  (tcp-send    (-> TcpSocket String (Result String Unit)))
  (tcp-recv    (-> TcpSocket (Option String)))
  (tcp-close   (-> TcpSocket Unit))
  (tcp-close-listener (-> TcpListener Unit)))
```

Following the failable operations convention, `stdlib/net.mar` provides two wrapper styles:

| Operation | Safe wrapper | Throwing wrapper |
|-----------|-------------|-----------------|
| `tcp-connect` | `connect` — returns `(Result String TcpSocket)` | `connect!` — throws on failure |
| `tcp-listen` | `listen` — returns `(Result String TcpListener)` | `listen!` — throws on failure |
| `tcp-send` | `send` — returns `(Result String Unit)` | `send!` — throws on failure |
| `tcp-accept` | `accept` — returns `(Option TcpSocket)` | Same (not failable) |
| `tcp-recv` | `recv` — returns `(Option String)` | Same (not failable) |
| `tcp-close` | `close` — always succeeds | Same |
| `tcp-close-listener` | `close-listener` — always succeeds | Same |

### TCP Semantics

| Operation | Behavior |
|-----------|----------|
| `connect host port` | Async TCP connect via `tokio::net::TcpStream`. Fiber is parked until connection completes. A background task is spawned to read incoming data into a buffer. |
| `listen port` | Bind a `tokio::net::TcpListener`. A background accept loop is spawned that pushes accepted connections into a queue. |
| `accept listener` | Returns the next accepted connection from the queue. If none available, parks the fiber until one arrives. Returns `None` if the listener has been closed. |
| `send sock data` | Async write via `tokio::io::AsyncWriteExt::write_all()`. Fiber is parked until write completes. |
| `recv sock` | Returns buffered data if available. If no data, parks the fiber until data arrives from the background read task. Returns `None` on EOF or remote close. |
| `close sock` | Close the socket. |
| `close-listener listener` | Shut down the listener and its accept loop. |

TCP socket state is tracked per-scheduler-thread. Cross-thread TCP operations (e.g., a fiber on thread 1 sending on a socket owned by thread 0) are routed via `TcpOp` messages between scheduler threads. Cross-thread `recv`/`accept` when no data is buffered is not yet supported.

### Example: Echo Server

```lisp
(import "std:net" as net)

(run-fibers-parallel (lambda ()
  (let ((listener (net/listen 9001)))
    (perform fiber-spawn (lambda ()
      (match (net/accept listener)
        (None nil)
        ((Some client)
          (begin
            (match (net/recv client)
              (None nil)
              ((Some data)
                (net/send client (string-append "echo: " data))))
            (net/close client))))))
    (perform fiber-yield)
    (perform fiber-yield)
    (let ((sock (net/connect "127.0.0.1" 9001)))
      (net/send sock "hello from marlang!")
      (match (net/recv sock)
        (None nil)
        ((Some resp) (perform println resp)))
      (net/close sock))
    (net/close-listener listener)))
  2)
;; Output: echo: hello from marlang!
```

---

## Implementation Details

### File Structure

```
port/crates/vm/src/
  scheduler.rs    — Single-threaded synchronous scheduler (824 lines)
  runtime.rs      — Multi-threaded work-stealing scheduler (2557 lines)
  builtins.rs     — run-fibers and run-fibers-parallel builtins
  vm.rs           — VM core: EffectSignal, try_default_handler, resume_continuation
  object.rs       — VMObject enum: Pid(u64), Port(u64), Continuation(Box<VMContinuation>)
  value.rs        — NaN-boxed Value type

stdlib/
  concurrent.mar  — Fiber + Channel effects and wrappers
  actor.mar       — Actor effects and wrappers
  io.mar          — IO effects (print, println, read-line, read-file, write-file)
  net.mar         — TCP networking effects and wrappers (throwing + try)
  http.mar        — HTTP API (unimplemented design sketch)
```

### Handled Effects

Both schedulers install a handler frame for the same set of effect names:

```
Fibers:   fiber-spawn, fiber-yield, fiber-self
Channels: chan-new, chan-send, chan-recv, chan-close
Actors:   actor-send, actor-receive, actor-receive-match,
          actor-monitor, actor-link, actor-unlink, actor-trap-exit
IO:       print, println, read-line, read-file, write-file
TCP:      tcp-connect, tcp-listen, tcp-accept, tcp-send, tcp-recv,
          tcp-close, tcp-close-listener
```

The single-threaded scheduler handles TCP effects by returning an error:
`"TCP effects require the multi-threaded scheduler (run-fibers-parallel)"`.

### Single-Threaded Scheduler (`scheduler.rs`)

- Entry: `run_fibers(vm, body)` — synchronous, returns `Result<Value, VMError>`
- All state is local (no `Mutex`, no atomics)
- Simple trampoline loop: `while let Some(fiber) = ready_queue.pop_front()`
- IO is blocking — when a fiber does `read-file`, the entire scheduler blocks
- Deadlock: if the ready queue is empty but live PIDs remain with blocked receives or channel waits, the scheduler exits (no fibers can make progress)

### Multi-Threaded Scheduler (`runtime.rs`)

- Entry: `run_fibers_parallel(vm, body, num_threads)` — creates a `tokio` runtime, spawns N OS threads via `std::thread::scope`, returns `Result<Value, VMError>`
- Each thread owns a `PerScheduler` (local ready queue, parked fibers, TCP state) and a `VM` instance cloned from the parent's globals
- Shared state (`SharedState`) behind `Mutex`: mailboxes, channels, supervision (monitors, links, trap-exit), PID→scheduler mapping
- **Work stealing**: `crossbeam_deque::Worker`/`Stealer` — idle threads steal fibers from busy threads
- **Spawn distribution**: round-robin with local bias (keep fiber local if queue is small)
- **Async IO**: operations spawn `tokio` tasks that send `SchedulerMessage::IoComplete` back to the owning scheduler thread
- **TCP**: each socket/listener tracked per-scheduler; cross-scheduler operations routed via `TcpOp` messages
- **Termination**: idle counting + alive/parked comparison — all threads exit when no fibers are alive and no IO is pending

### Handler Frame Mechanics

The scheduler installs a `VMHandlerFrame` with dummy closures for all effect names. `PERFORM` only checks `handlers.has(op_name)` to decide whether to capture a continuation and return an `EffectSignal` — the closures themselves are never invoked because the scheduler catches the `EffectSignal` before dispatch would reach them.

Each fiber resume reinstalls the handler frame (since `PERFORM` removes it when catching an effect). On fiber crash, the scheduler restores `frame_count` and `stack_top` and removes the handler frame.

---

## Edge Cases and Design Decisions

### Deadlock Detection

**Single-threaded**: If the ready queue is empty but live PIDs remain blocked on receives or channel waits, the scheduler returns the main fiber's result (if it completed) without error. Blocked fibers are silently abandoned.

**Multi-threaded**: Termination detection compares alive PIDs against parked PIDs plus idle schedulers. If no fibers can make progress and no IO is pending, all threads exit.

### Main Fiber Result

The return value of `run-fibers` / `run-fibers-parallel` is the return value of the main fiber (PID 0). If the main fiber crashes, the error propagates. Spawned fibers' return values are discarded unless communicated via channels or messages.

### Channel vs. Mailbox

Channels and mailboxes are independent mechanisms:
- Channels are explicit, typed, shared objects (CSP style)
- Mailboxes are per-actor, implicit, untyped queues (Erlang style)
- Both can be used in the same program

### Nested Schedulers

Calling `run-fibers` inside a fiber creates a nested scheduler with its own ready queue, channels, and mailboxes. The outer fiber blocks until the inner scheduler completes. PIDs are not shared between nested schedulers.

### Predicate Effects in Selective Receive

The predicate passed to `actor-receive-match` is called synchronously within the scheduler. If a predicate performs an effect, behavior is undefined. This is a documented limitation.

---

## Verification

```bash
# Run all tests (includes concurrency tests)
cargo test --workspace

# Run VM tests specifically (concurrency, TCP, scheduling)
cargo test -p marlang-vm

# Run example programs
cargo run -p marlang-cli -- run examples/fibers.mar
cargo run -p marlang-cli -- run examples/channels.mar
cargo run -p marlang-cli -- run examples/actors.mar
cargo run -p marlang-cli -- run examples/tcp.mar

# Run with and without type checking
cargo run -p marlang-cli -- run examples/fibers.mar
cargo run -p marlang-cli -- run examples/fibers.mar --no-typecheck
```

---

## Risks & Mitigations

| Risk | Likelihood | Mitigation |
|------|------------|------------|
| Handler frame leak on fiber crash | High | Scheduler restores `frame_count`/`stack_top` and removes handler frame on every crash path |
| One-shot continuation reuse bugs | Medium | `cont.resumed` flag is checked; double-resume returns an error |
| Mutex contention in multi-threaded scheduler | Medium | Hot paths (channel send/recv, mailbox delivery) hold locks briefly; per-scheduler local state reduces contention |
| Work-stealing fairness | Low | FIFO stealing provides reasonable fairness; no priority scheduling |
| Cross-scheduler TCP routing | Medium | Socket/listener ownership tracked per-scheduler; cross-thread operations routed via messages; cross-thread recv when no data is buffered is not yet supported |
| Selective receive performance | Low | Linear scan is O(n) per receive; acceptable for initial impl |
| Recursive define + TCP effects hangs | Medium | Known issue documented in CLAUDE.md; workaround: unroll recursion |

---

## Non-Goals (Explicit)

- **Distributed actors**: No cross-process messaging
- **Typed channels/mailboxes**: Channels and mailboxes are dynamically typed
- **Preemptive scheduling**: All scheduling is cooperative (fibers yield voluntarily)
- **Select/alt for channels**: Multiplexing across multiple channels is future work
- **HTTP**: Design sketch exists in `stdlib/http.mar` but no runtime handlers are implemented
- **UDP**: Not implemented

---

## Success Criteria

1. Fibers interleave correctly with cooperative scheduling
2. Unbuffered and buffered channels work with proper blocking/waking semantics
3. Channel operations return `Result`/`Option` types correctly
4. Actor send/receive works, including selective receive
5. Monitor and link supervision delivers correct signals
6. IO effects work both inside and outside schedulers
7. Multi-threaded scheduler distributes fibers across threads with work stealing
8. Async IO in `run-fibers-parallel`: file reads park fibers without blocking other threads
9. TCP networking works in `run-fibers-parallel`: connect, listen, accept, send, recv, close
10. Existing tests pass (`cargo test --workspace`)
11. Counter actor example produces correct output
12. Ping-pong example runs without stack overflow
13. Echo server example works end-to-end
