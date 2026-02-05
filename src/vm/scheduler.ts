// Async trampoline scheduler for Marlang concurrency (VM backend)
//
// Implements fibers (green threads), channels (CSP), actors (mailboxes,
// selective receive, supervision), and async IO — all powered by the
// existing effect/continuation machinery.

import type { VM, VMHandlerFrame } from './vm';
import type { VMValue, VMClosure, VMContinuation, VMPid, VMChannel, Chunk, FunctionPrototype } from './bytecode';
import { NIL, TRUE, FALSE } from './bytecode';
import { EffectSignal, callClosure, resumeContinuation, push, isTruthy, formatVMValue } from './vm';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

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
    predicate?: VMClosure;
  }>;

  // Supervision (Layer 2)
  monitors: Map<FiberId, Set<FiberId>>;
  links: Map<FiberId, Set<FiberId>>;
  trapExit: Set<FiberId>;
  livePids: Set<FiberId>;

  // Async IO (Layer 3)
  pendingIO: Map<FiberId, { promise: Promise<VMValue>; cont: VMContinuation }>;

  // IO channels
  stdoutBuffer: string[];
  stderrBuffer: string[];

  // Collected output (for testing)
  output: string[];

  // Options
  onOutput?: (text: string) => void;
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function enqueueResumed(
  state: SchedulerState,
  pid: FiberId,
  cont: VMContinuation,
  value: VMValue,
): void {
  state.readyQueue.push({ state: 'suspended', cont, resumeValue: value, pid });
}

function deliverMessage(state: SchedulerState, targetPid: FiberId, msg: VMValue): void {
  const mailbox = state.mailboxes.get(targetPid);
  if (!mailbox) return;
  mailbox.push(msg);

  // Check if target is blocked on receive
  const blocked = state.blockedOnReceive.get(targetPid);
  if (blocked) {
    if (blocked.predicate) {
      // Selective receive: test predicate against new message
      const matches = callPredicateSafe(state, blocked.predicate, msg);
      if (matches) {
        const idx = mailbox.indexOf(msg);
        if (idx !== -1) mailbox.splice(idx, 1);
        state.blockedOnReceive.delete(targetPid);
        enqueueResumed(state, targetPid, blocked.cont, msg);
      }
    } else {
      // Non-selective: dequeue first message, wake receiver
      const first = mailbox.shift()!;
      state.blockedOnReceive.delete(targetPid);
      enqueueResumed(state, targetPid, blocked.cont, first);
    }
  }
}

/** Call a predicate closure synchronously. Returns true if the predicate
 *  returns a truthy value. The predicate must be pure (no effects). */
function callPredicateSafe(state: SchedulerState, predicate: VMClosure, value: VMValue): boolean {
  try {
    const result = callClosure(state.vm, predicate, [value]);
    return isTruthy(result);
  } catch {
    return false;
  }
}

function killFiber(state: SchedulerState, pid: FiberId, reason: string): void {
  // Remove from ready queue
  state.readyQueue = state.readyQueue.filter(f => f.pid !== pid);
  // Remove from blocked
  state.blockedOnReceive.delete(pid);
  // Remove from pending IO
  state.pendingIO.delete(pid);
  // Process exit
  handleFiberExit(state, pid, 'crash', reason);
}

function handleFiberExit(
  state: SchedulerState,
  pid: FiberId,
  exitType: 'normal' | 'crash',
  error: any,
): void {
  state.livePids.delete(pid);
  const reason = exitType === 'normal' ? 'normal' : String(error?.message ?? error);

  // Notify monitors: deliver (Down pid reason)
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
      if (exitType === 'normal') continue;

      if (state.trapExit.has(linkedPid)) {
        const exitMsg: VMValue = {
          kind: 'adt-value',
          constructor: 'Exit',
          args: [{ kind: 'pid', id: pid }, { kind: 'string', value: reason }],
        };
        deliverMessage(state, linkedPid, exitMsg);
      } else {
        killFiber(state, linkedPid, reason);
      }
    }
    state.links.delete(pid);
  }

  // Clean up
  state.mailboxes.delete(pid);
  state.blockedOnReceive.delete(pid);
}

// Flush stdout buffer
async function flushStdout(state: SchedulerState): Promise<void> {
  if (state.stdoutBuffer.length > 0) {
    const text = state.stdoutBuffer.join('');
    state.stdoutBuffer.length = 0;
    if (state.onOutput) {
      state.onOutput(text);
    } else {
      process.stdout.write(text);
    }
  }
}

// Flush stderr buffer
async function flushStderr(state: SchedulerState): Promise<void> {
  if (state.stderrBuffer.length > 0) {
    const text = state.stderrBuffer.join('');
    state.stderrBuffer.length = 0;
    process.stderr.write(text);
  }
}

// ---------------------------------------------------------------------------
// Effect Dispatch
// ---------------------------------------------------------------------------

function handleEffect(state: SchedulerState, signal: EffectSignal): void {
  const { opName, args, continuation } = signal;

  switch (opName) {
    // --- Layer 1: Fibers ---

    case 'fiber-spawn': {
      const thunk = args[0] as VMClosure;
      const pid = state.nextPid++;
      state.livePids.add(pid);
      state.mailboxes.set(pid, []);
      // Resume parent first (so it can set up monitors/links before child runs)
      enqueueResumed(state, state.currentPid, continuation, { kind: 'pid', id: pid });
      state.readyQueue.push({ state: 'new', closure: thunk, pid });
      break;
    }

    case 'fiber-yield': {
      enqueueResumed(state, state.currentPid, continuation, NIL);
      break;
    }

    case 'fiber-self': {
      enqueueResumed(state, state.currentPid, continuation, { kind: 'pid', id: state.currentPid });
      break;
    }

    // --- Layer 1: Channels ---

    case 'chan-new': {
      const capacity = args[0] as number;
      const id = state.nextChannelId++;
      const ch: ChannelState = {
        buffer: [], capacity, closed: false,
        waitingSenders: [], waitingReceivers: [],
      };
      state.channels.set(id, ch);
      enqueueResumed(state, state.currentPid, continuation, { kind: 'channel', id });
      break;
    }

    case 'chan-send': {
      const chVal = args[0];
      const value = args[1];
      if (typeof chVal === 'number' || chVal.kind !== 'channel') {
        throw new Error('chan-send: first argument must be a channel');
      }
      const chanState = state.channels.get(chVal.id);
      if (!chanState) throw new Error('chan-send: invalid channel');

      if (chanState.closed) {
        throw new Error('Cannot send on closed channel');
      }

      if (chanState.waitingReceivers.length > 0) {
        const receiver = chanState.waitingReceivers.shift()!;
        enqueueResumed(state, receiver.pid, receiver.cont, value);
        enqueueResumed(state, state.currentPid, continuation, NIL);
      } else if (chanState.buffer.length < chanState.capacity) {
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
      const chVal = args[0];
      if (typeof chVal === 'number' || chVal.kind !== 'channel') {
        throw new Error('chan-recv: first argument must be a channel');
      }
      const chanState = state.channels.get(chVal.id);
      if (!chanState) throw new Error('chan-recv: invalid channel');

      if (chanState.buffer.length > 0) {
        const value = chanState.buffer.shift()!;
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
      const chVal = args[0];
      if (typeof chVal === 'number' || chVal.kind !== 'channel') {
        throw new Error('chan-close: first argument must be a channel');
      }
      const chanState = state.channels.get(chVal.id);
      if (!chanState) throw new Error('chan-close: invalid channel');

      chanState.closed = true;
      // Wake all blocked receivers with nil
      for (const receiver of chanState.waitingReceivers) {
        enqueueResumed(state, receiver.pid, receiver.cont, NIL);
      }
      chanState.waitingReceivers.length = 0;
      enqueueResumed(state, state.currentPid, continuation, NIL);
      break;
    }

    // --- Layer 2: Actors ---

    case 'actor-send': {
      const targetPidVal = args[0];
      const msg = args[1];
      if (typeof targetPidVal === 'number' || targetPidVal.kind !== 'pid') {
        throw new Error('actor-send: first argument must be a pid');
      }
      deliverMessage(state, targetPidVal.id, msg);
      enqueueResumed(state, state.currentPid, continuation, NIL);
      break;
    }

    case 'actor-receive': {
      const mailbox = state.mailboxes.get(state.currentPid);
      if (mailbox && mailbox.length > 0) {
        const msg = mailbox.shift()!;
        enqueueResumed(state, state.currentPid, continuation, msg);
      } else {
        state.blockedOnReceive.set(state.currentPid, { cont: continuation });
      }
      break;
    }

    case 'actor-receive-match': {
      const predicate = args[0] as VMClosure;
      const mailbox = state.mailboxes.get(state.currentPid);

      if (mailbox) {
        for (let i = 0; i < mailbox.length; i++) {
          const matches = callPredicateSafe(state, predicate, mailbox[i]);
          if (matches) {
            const msg = mailbox.splice(i, 1)[0];
            enqueueResumed(state, state.currentPid, continuation, msg);
            return;
          }
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
      const targetPidVal = args[0];
      if (typeof targetPidVal === 'number' || targetPidVal.kind !== 'pid') {
        throw new Error('actor-monitor: argument must be a pid');
      }
      const targetPid = targetPidVal.id;
      if (!state.monitors.has(targetPid)) {
        state.monitors.set(targetPid, new Set());
      }
      state.monitors.get(targetPid)!.add(state.currentPid);
      enqueueResumed(state, state.currentPid, continuation, NIL);
      break;
    }

    case 'actor-link': {
      const targetPidVal = args[0];
      if (typeof targetPidVal === 'number' || targetPidVal.kind !== 'pid') {
        throw new Error('actor-link: argument must be a pid');
      }
      const targetPid = targetPidVal.id;
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
      const targetPidVal = args[0];
      if (typeof targetPidVal === 'number' || targetPidVal.kind !== 'pid') {
        throw new Error('actor-unlink: argument must be a pid');
      }
      const targetPid = targetPidVal.id;
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
      const msg = args[0];
      const text = typeof msg === 'number' ? String(msg) :
                   msg.kind === 'string' ? msg.value : formatVMValue(msg);
      state.stdoutBuffer.push(text);
      state.output.push(text);
      enqueueResumed(state, state.currentPid, continuation, NIL);
      break;
    }

    case 'println': {
      const msg = args[0];
      const text = typeof msg === 'number' ? String(msg) :
                   msg.kind === 'string' ? msg.value : formatVMValue(msg);
      state.stdoutBuffer.push(text + '\n');
      state.output.push(text + '\n');
      enqueueResumed(state, state.currentPid, continuation, NIL);
      break;
    }

    case 'read-line': {
      const promise = new Promise<string>((resolve) => {
        resolve(prompt('') ?? '');
      }).then(line => ({ kind: 'string', value: line }) as VMValue);
      state.pendingIO.set(state.currentPid, { promise, cont: continuation });
      break;
    }

    case 'read-file':
    case 'io-read-file': {
      const pathArg = args[0];
      if (typeof pathArg === 'number' || pathArg.kind !== 'string') {
        throw new Error('read-file: path must be a string');
      }
      const promise = Bun.file(pathArg.value).text()
        .then(content => ({ kind: 'string', value: content }) as VMValue);
      state.pendingIO.set(state.currentPid, { promise, cont: continuation });
      break;
    }

    case 'write-file':
    case 'io-write-file': {
      const pathArg = args[0];
      const content = args[1];
      if (typeof pathArg === 'number' || pathArg.kind !== 'string') {
        throw new Error('write-file: path must be a string');
      }
      if (typeof content === 'number' || content.kind !== 'string') {
        throw new Error('write-file: content must be a string');
      }
      const promise = Bun.write(pathArg.value, content.value)
        .then(() => NIL as VMValue);
      state.pendingIO.set(state.currentPid, { promise, cont: continuation });
      break;
    }

    case 'io-stdout': {
      // Return a special channel id for stdout
      if (!state.channels.has(-1)) {
        state.channels.set(-1, {
          buffer: [], capacity: 64, closed: false,
          waitingSenders: [], waitingReceivers: [],
        });
      }
      enqueueResumed(state, state.currentPid, continuation, { kind: 'channel', id: -1 });
      break;
    }

    case 'io-stderr': {
      if (!state.channels.has(-2)) {
        state.channels.set(-2, {
          buffer: [], capacity: 64, closed: false,
          waitingSenders: [], waitingReceivers: [],
        });
      }
      enqueueResumed(state, state.currentPid, continuation, { kind: 'channel', id: -2 });
      break;
    }

    case 'io-stdin': {
      if (!state.channels.has(-3)) {
        state.channels.set(-3, {
          buffer: [], capacity: 0, closed: false,
          waitingSenders: [], waitingReceivers: [],
        });
      }
      enqueueResumed(state, state.currentPid, continuation, { kind: 'channel', id: -3 });
      break;
    }

    case 'io-open-file': {
      const pathArg = args[0];
      const mode = args[1];
      if (typeof pathArg === 'number' || pathArg.kind !== 'string') {
        throw new Error('open-file: path must be a string');
      }
      const modeStr = (typeof mode !== 'number' && mode.kind === 'string') ? mode.value : 'r';
      const chId = state.nextChannelId++;
      const ch: ChannelState = {
        buffer: [], capacity: 16, closed: false,
        waitingSenders: [], waitingReceivers: [],
      };
      state.channels.set(chId, ch);

      if (modeStr === 'r') {
        // Start async file read streaming in background
        const filePath = pathArg.value;
        (async () => {
          try {
            const content = await Bun.file(filePath).text();
            const lines = content.split('\n');
            for (const line of lines) {
              ch.buffer.push({ kind: 'string', value: line });
              // Wake any waiting receivers
              if (ch.waitingReceivers.length > 0 && ch.buffer.length > 0) {
                const val = ch.buffer.shift()!;
                const recv = ch.waitingReceivers.shift()!;
                enqueueResumed(state, recv.pid, recv.cont, val);
              }
            }
          } catch (e) {
            // On error, close channel
          } finally {
            ch.closed = true;
            for (const recv of ch.waitingReceivers) {
              enqueueResumed(state, recv.pid, recv.cont, NIL);
            }
            ch.waitingReceivers.length = 0;
          }
        })();
      }

      enqueueResumed(state, state.currentPid, continuation, { kind: 'channel', id: chId });
      break;
    }

    default:
      throw new Error(`Scheduler: unhandled effect '${opName}'`);
  }
}

// ---------------------------------------------------------------------------
// Handler frame installation
// ---------------------------------------------------------------------------

const emptyChunk: Chunk = {
  code: [],
  constants: [],
  localCount: 0,
};

const HANDLED_EFFECTS = [
  // Layer 1: Fibers + Channels
  'fiber-spawn', 'fiber-yield', 'fiber-self',
  'chan-new', 'chan-send', 'chan-recv', 'chan-close',
  // Layer 2: Actors
  'actor-send', 'actor-receive', 'actor-receive-match',
  'actor-monitor', 'actor-link', 'actor-unlink', 'actor-trap-exit',
  // Layer 3: IO
  'print', 'println', 'read-line', 'read-file', 'write-file',
  'io-stdout', 'io-stderr', 'io-stdin',
  'io-read-file', 'io-write-file', 'io-open-file',
];

function installHandlerFrame(state: SchedulerState): void {
  const dummyClosure: VMClosure = {
    kind: 'vm-closure',
    proto: { name: '<scheduler>', arity: 0, upvalues: [], chunk: emptyChunk },
    upvalues: [],
  };

  const handlers = new Map<string, { closure: VMClosure; arity: number }>();
  for (const name of HANDLED_EFFECTS) {
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

function removeHandlerFrame(state: SchedulerState): void {
  state.vm.handlerStack.pop();
}

// ---------------------------------------------------------------------------
// Entry Point
// ---------------------------------------------------------------------------

export type RunFibersOptions = {
  onOutput?: (text: string) => void;
};

export async function runFibers(vm: VM, body: VMClosure, options?: RunFibersOptions): Promise<VMValue> {
  const state: SchedulerState = {
    vm,
    readyQueue: [],
    currentPid: 0,
    nextPid: 1,
    channels: new Map(),
    nextChannelId: 0,
    mailboxes: new Map([[0, []]]),
    blockedOnReceive: new Map(),
    monitors: new Map(),
    links: new Map(),
    trapExit: new Set(),
    livePids: new Set([0]),
    pendingIO: new Map(),
    stdoutBuffer: [],
    stderrBuffer: [],
    output: [],
    onOutput: options?.onOutput,
  };

  let mainResult: VMValue = NIL;

  // Start main fiber (PID 0)
  installHandlerFrame(state);
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
  // PERFORM removes the handler frame, so we need to track that.
  // After each EffectSignal or fiber completion, the handler frame may
  // have been removed by PERFORM. We reinstall it before each fiber run.

  // Async trampoline loop
  while (state.readyQueue.length > 0 || state.pendingIO.size > 0) {
    // --- SYNC PHASE: run ready fibers ---
    while (state.readyQueue.length > 0) {
      const fiber = state.readyQueue.shift()!;
      state.currentPid = fiber.pid;

      // Save VM state so we can restore on crash
      const savedFrameCount = vm.frameCount;
      const savedStackTop = vm.stackTop;

      // Reinstall handler frame (PERFORM removes it when catching an effect)
      installHandlerFrame(state);

      try {
        let result: VMValue;
        if (fiber.state === 'new') {
          result = callClosure(vm, fiber.closure, []);
        } else {
          result = resumeContinuation(vm, fiber.cont, fiber.resumeValue);
        }
        // Fiber completed normally - remove our handler frame
        removeHandlerFrame(state);
        if (fiber.pid === 0) mainResult = result;
        handleFiberExit(state, fiber.pid, 'normal', NIL);
      } catch (e) {
        if (e instanceof EffectSignal) {
          // PERFORM already cleaned up VM state
          handleEffect(state, e);
        } else {
          // Fiber crashed - restore VM state and clean up handler frame
          vm.frameCount = savedFrameCount;
          vm.stackTop = savedStackTop;
          if (vm.handlerStack.length > 0) {
            removeHandlerFrame(state);
          }
          handleFiberExit(state, fiber.pid, 'crash', e);
        }
      }
    }

    // --- DRAIN PHASE: await pending IO if no ready fibers ---
    if (state.readyQueue.length === 0 && state.pendingIO.size > 0) {
      await flushStdout(state);
      await flushStderr(state);

      const { pid, value } = await Promise.race(
        [...state.pendingIO.entries()].map(([pid, entry]) =>
          entry.promise.then(value => ({ pid, value }))
        )
      );

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

  // Final flush
  await flushStdout(state);
  await flushStderr(state);

  removeHandlerFrame(state);

  return mainResult;
}

/** Run fibers and collect output (for testing) */
export async function runFibersCollectOutput(
  vm: VM,
  body: VMClosure,
): Promise<{ result: VMValue; output: string[] }> {
  const collected: string[] = [];
  const result = await runFibers(vm, body, {
    onOutput: (text) => collected.push(text),
  });
  return { result, output: collected };
}

/** Get the promise from runFibers for CLI await */
let _lastPromise: Promise<VMValue> | null = null;
export function getLastRunFibersPromise(): Promise<VMValue> | null {
  const p = _lastPromise;
  _lastPromise = null;
  return p;
}

export function setLastRunFibersPromise(p: Promise<VMValue>): void {
  _lastPromise = p;
}
