// Stack-based VM for Marlang

import { Op, OpName, type Chunk, type FunctionPrototype, type VMValue, type VMObject, type VMClosure, type VMBuiltin, type VMContinuation, type CallFrame, NIL, TRUE, FALSE, vmKind, preconvertConstants } from './bytecode';
import type { RuntimeValue } from '../core/types';
import { createBuiltinGlobals } from './builtins';
import * as fs from 'fs';

// --- Effect System Types ---

type VMHandlerFrame = {
  handlers: Map<string, { closure: VMClosure; arity: number }>;
  returnHandler?: VMClosure;
  // Immutable: where to unwind VM state when an effect is caught
  unwindFrameCount: number;
  unwindStackTop: number;
  // Mutable: where to start capturing continuation frames/stack.
  // Initially same as unwind values. Updated by RESUME so that
  // re-performed effects only capture body frames, not handler body frames.
  captureFrameStart: number;
  captureStackStart: number;
};

class EffectSignal {
  opName: string;
  args: VMValue[];
  continuation: VMContinuation;
  targetHandler: VMHandlerFrame;

  constructor(opName: string, args: VMValue[], continuation: VMContinuation, targetHandler: VMHandlerFrame) {
    this.opName = opName;
    this.args = args;
    this.continuation = continuation;
    this.targetHandler = targetHandler;
  }
}

// --- VM Types ---

export type VM = {
  stack: VMValue[];
  stackTop: number;
  frames: CallFrame[];
  frameCount: number;
  globals: Map<string, VMValue>;
  handlerStack: VMHandlerFrame[];
};

// --- Stack Operations ---

function push(vm: VM, value: VMValue): void {
  vm.stack[vm.stackTop++] = value;
}

function pop(vm: VM): VMValue {
  return vm.stack[--vm.stackTop];
}

function peek(vm: VM, distance: number = 0): VMValue {
  return vm.stack[vm.stackTop - 1 - distance];
}

// --- Truthiness ---

function isTruthy(value: VMValue): boolean {
  if (typeof value === 'number') return true;
  if (value.kind === 'nil') return false;
  if (value.kind === 'boolean' && !value.value) return false;
  return true;
}

// --- Equality ---

function valuesEqual(a: VMValue, b: VMValue): boolean {
  if (typeof a === 'number') return typeof b === 'number' && a === b;
  if (typeof b === 'number') return false;
  if (a.kind === 'nil' && b.kind === 'nil') return true;
  if (a.kind === 'boolean' && b.kind === 'boolean') return a.value === b.value;
  if (a.kind === 'string' && b.kind === 'string') return a.value === b.value;
  if (a.kind === 'adt-value' && b.kind === 'adt-value') {
    if (a.constructor !== b.constructor) return false;
    if (a.args.length !== b.args.length) return false;
    return a.args.every((arg, i) => valuesEqual(arg, b.args[i]));
  }
  if (a.kind === 'list' && b.kind === 'list') {
    if (a.elements.length !== b.elements.length) return false;
    return a.elements.every((el, i) => valuesEqual(el, b.elements[i]));
  }
  if (a.kind === 'nil' && b.kind === 'list' && b.elements.length === 0) return true;
  if (b.kind === 'nil' && a.kind === 'list' && a.elements.length === 0) return true;
  if (a.kind === 'nil' && b.kind === 'adt-value' && b.constructor === 'Nil' && b.args.length === 0) return true;
  if (b.kind === 'nil' && a.kind === 'adt-value' && a.constructor === 'Nil' && a.args.length === 0) return true;
  return false;
}

// --- VM Creation ---

export function createVM(): VM {
  return {
    stack: new Array(256),
    stackTop: 0,
    frames: [],
    frameCount: 0,
    globals: createBuiltinGlobals(),
    handlerStack: [],
  };
}

// --- Closure Helpers ---

function createClosureFromProto(frame: CallFrame, proto: FunctionPrototype): VMClosure {
  const upvalues: VMValue[] = new Array(proto.upvalues.length);
  for (let i = 0; i < proto.upvalues.length; i++) {
    const desc = proto.upvalues[i];
    if (desc.isLocal) {
      upvalues[i] = frame.locals[desc.index];
    } else {
      upvalues[i] = frame.closure.upvalues[desc.index];
    }
  }
  return { kind: 'vm-closure', proto, upvalues };
}

function callClosure(vm: VM, closure: VMClosure, args: VMValue[]): VMValue {
  const locals = new Array(closure.proto.chunk.localCount).fill(NIL);
  for (let i = 0; i < args.length; i++) {
    locals[i] = args[i];
  }
  const newFrame: CallFrame = { closure, ip: 0, locals };
  const baseFrameCount = vm.frameCount;
  vm.frames[vm.frameCount++] = newFrame;
  return run(vm, baseFrameCount);
}

// --- Effect Handler Execution ---

function runWithHandler(
  vm: VM,
  handlerDef: { handlers: Map<string, { closure: VMClosure; arity: number }>; returnHandler?: VMClosure },
  closure: VMClosure,
  args: VMValue[],
): VMValue {
  const handlerFrame: VMHandlerFrame = {
    handlers: handlerDef.handlers,
    returnHandler: handlerDef.returnHandler,
    unwindFrameCount: vm.frameCount,
    unwindStackTop: vm.stackTop,
    captureFrameStart: vm.frameCount,
    captureStackStart: vm.stackTop,
  };

  vm.handlerStack.push(handlerFrame);

  try {
    return callClosure(vm, closure, args);
  } catch (e) {
    if (e instanceof EffectSignal && e.targetHandler === handlerFrame) {
      // Handler was already removed from stack by PERFORM's unwind.
      const clause = handlerFrame.handlers.get(e.opName);
      if (!clause) throw e; // shouldn't happen
      // Only pass the operation args the handler declares, then the continuation
      const expectedArgCount = clause.arity - 1;
      const handlerArgs: VMValue[] = [];
      for (let i = 0; i < expectedArgCount && i < e.args.length; i++) {
        handlerArgs.push(e.args[i]);
      }
      handlerArgs.push(e.continuation);
      // Deep handler semantics: recursively run handler body with same handler def
      return runWithHandler(vm, handlerDef, clause.closure, handlerArgs);
    }
    // Not for us — clean up if our handler is still on the stack
    const idx = vm.handlerStack.lastIndexOf(handlerFrame);
    if (idx !== -1) vm.handlerStack.splice(idx, 1);
    throw e;
  }
}

/** Run a restored continuation, catching effects that target restored handlers */
function runRestoredContinuation(
  vm: VM,
  resumeBase: number,
  restoredHandlers: VMHandlerFrame[],
  savedHandlerLen: number,
): VMValue {
  try {
    return run(vm, resumeBase);
  } catch (e) {
    if (e instanceof EffectSignal) {
      // Check if the effect targets one of our restored handlers
      for (const hframe of restoredHandlers) {
        if (e.targetHandler === hframe) {
          const clause = hframe.handlers.get(e.opName);
          if (!clause) throw e;

          const expectedArgCount = clause.arity - 1;
          const handlerArgs: VMValue[] = [];
          for (let i = 0; i < expectedArgCount && i < e.args.length; i++) {
            handlerArgs.push(e.args[i]);
          }
          handlerArgs.push(e.continuation);

          // Clean up restored handlers before dispatching
          vm.handlerStack.length = savedHandlerLen;

          // Deep handler semantics: run handler body with same handler def
          return runWithHandler(vm, {
            handlers: hframe.handlers,
            returnHandler: hframe.returnHandler,
          }, clause.closure, handlerArgs);
        }
      }
    }
    throw e;
  }
}

// --- Interpret ---

/** Recursively pre-convert constant pools to VMValues */
function ensureVmConstants(chunk: Chunk): void {
  if (chunk.vmConstants) return;
  chunk.vmConstants = preconvertConstants(chunk.constants);
  for (const c of chunk.constants) {
    if (c.kind === 'prototype') {
      ensureVmConstants(c.proto.chunk);
    }
  }
}

export function interpret(vm: VM, chunk: Chunk): RuntimeValue {
  ensureVmConstants(chunk);

  const scriptProto: FunctionPrototype = {
    name: '<script>',
    arity: 0,
    upvalues: [],
    chunk,
  };

  const scriptClosure: VMClosure = {
    kind: 'vm-closure',
    proto: scriptProto,
    upvalues: [],
  };

  const frame: CallFrame = {
    closure: scriptClosure,
    ip: 0,
    locals: new Array(chunk.localCount).fill(NIL),
  };

  vm.frames[0] = frame;
  vm.frameCount = 1;

  const result = run(vm, 0);
  return toRuntimeValue(result);
}

// --- Main Execution Loop ---

function run(vm: VM, baseFrameCount: number): VMValue {
  let frame = vm.frames[vm.frameCount - 1];

  while (true) {
    const inst = frame.closure.proto.chunk.code[frame.ip++];

    switch (inst.op) {
      case Op.PUSH_CONST: {
        push(vm, frame.closure.proto.chunk.vmConstants![inst.operand!]);
        break;
      }

      case Op.PUSH_LOCAL: {
        push(vm, frame.locals[inst.operand!]);
        break;
      }

      case Op.SET_LOCAL: {
        frame.locals[inst.operand!] = pop(vm);
        break;
      }

      case Op.PUSH_UPVALUE: {
        push(vm, frame.closure.upvalues[inst.operand!]);
        break;
      }

      case Op.PUSH_GLOBAL: {
        const name = frame.closure.proto.chunk.constants[inst.operand!];
        if (name.kind !== 'string') throw new VMError('Global name must be string', getStackTrace(vm, frame));
        const value = vm.globals.get(name.value);
        if (value === undefined) {
          throw new VMError(`Undefined variable: ${name.value}`, getStackTrace(vm, frame));
        }
        push(vm, value);
        break;
      }

      case Op.SET_GLOBAL: {
        const name = frame.closure.proto.chunk.constants[inst.operand!];
        if (name.kind !== 'string') throw new VMError('Global name must be string', getStackTrace(vm, frame));
        const value = pop(vm);

        if (typeof value !== 'number' && value.kind === 'string' && value.value.startsWith('\0ctor:')) {
          const parts = value.value.substring(6).split(':');
          const ctorName = parts[0];
          const arity = parseInt(parts[1], 10);
          vm.globals.set(name.value, { kind: 'adt-constructor', name: ctorName, arity });
        } else {
          vm.globals.set(name.value, value);
        }
        break;
      }

      case Op.POP: {
        pop(vm);
        break;
      }

      case Op.DUP: {
        const v = peek(vm);
        push(vm, v);
        break;
      }

      // --- Arithmetic ---

      case Op.ADD: {
        const b = pop(vm);
        const a = pop(vm);
        if (typeof a === 'number' && typeof b === 'number') {
          push(vm, a + b);
        } else if (typeof a !== 'number' && typeof b !== 'number' && a.kind === 'string' && b.kind === 'string') {
          push(vm, { kind: 'string', value: a.value + b.value });
        } else {
          throw new VMError(`Cannot add ${vmKind(a)} and ${vmKind(b)}`, getStackTrace(vm, frame));
        }
        break;
      }

      case Op.SUB: {
        const b = pop(vm);
        const a = pop(vm);
        if (typeof a !== 'number' || typeof b !== 'number') {
          throw new VMError('Subtraction requires numbers', getStackTrace(vm, frame));
        }
        push(vm, a - b);
        break;
      }

      case Op.MUL: {
        const b = pop(vm);
        const a = pop(vm);
        if (typeof a !== 'number' || typeof b !== 'number') {
          throw new VMError('Multiplication requires numbers', getStackTrace(vm, frame));
        }
        push(vm, a * b);
        break;
      }

      case Op.DIV: {
        const b = pop(vm);
        const a = pop(vm);
        if (typeof a !== 'number' || typeof b !== 'number') {
          throw new VMError('Division requires numbers', getStackTrace(vm, frame));
        }
        push(vm, a / b);
        break;
      }

      case Op.NEG: {
        const a = pop(vm);
        if (typeof a !== 'number') throw new VMError('Negate requires number', getStackTrace(vm, frame));
        push(vm, -a);
        break;
      }

      // --- Comparison ---

      case Op.EQ: {
        const b = pop(vm);
        const a = pop(vm);
        push(vm, valuesEqual(a, b) ? TRUE : FALSE);
        break;
      }

      case Op.LT: {
        const b = pop(vm);
        const a = pop(vm);
        if (typeof a !== 'number' || typeof b !== 'number') {
          throw new VMError('< requires numbers', getStackTrace(vm, frame));
        }
        push(vm, a < b ? TRUE : FALSE);
        break;
      }

      case Op.GT: {
        const b = pop(vm);
        const a = pop(vm);
        if (typeof a !== 'number' || typeof b !== 'number') {
          throw new VMError('> requires numbers', getStackTrace(vm, frame));
        }
        push(vm, a > b ? TRUE : FALSE);
        break;
      }

      case Op.LE: {
        const b = pop(vm);
        const a = pop(vm);
        if (typeof a !== 'number' || typeof b !== 'number') {
          throw new VMError('<= requires numbers', getStackTrace(vm, frame));
        }
        push(vm, a <= b ? TRUE : FALSE);
        break;
      }

      case Op.GE: {
        const b = pop(vm);
        const a = pop(vm);
        if (typeof a !== 'number' || typeof b !== 'number') {
          throw new VMError('>= requires numbers', getStackTrace(vm, frame));
        }
        push(vm, a >= b ? TRUE : FALSE);
        break;
      }

      case Op.NOT: {
        const a = pop(vm);
        push(vm, isTruthy(a) ? FALSE : TRUE);
        break;
      }

      // --- Control Flow ---

      case Op.JUMP: {
        frame.ip = inst.operand!;
        break;
      }

      case Op.JUMP_IF_FALSE: {
        const cond = pop(vm);
        if (!isTruthy(cond)) {
          frame.ip = inst.operand!;
        }
        break;
      }

      case Op.RETURN: {
        const result = pop(vm);
        vm.frameCount--;

        if (vm.frameCount <= baseFrameCount) {
          return result;
        }

        frame = vm.frames[vm.frameCount - 1];
        push(vm, result);
        break;
      }

      // --- Functions ---

      case Op.CALL: {
        const arity = inst.operand!;
        const callee = peek(vm, arity);

        if (typeof callee === 'number') {
          throw new VMError(`Cannot call number`, getStackTrace(vm, frame));
        }

        if (callee.kind === 'vm-closure') {
          if (arity !== callee.proto.arity) {
            throw new VMError(
              `Expected ${callee.proto.arity} arguments, got ${arity}`,
              getStackTrace(vm, frame)
            );
          }

          const locals = new Array(callee.proto.chunk.localCount).fill(NIL);
          for (let i = arity - 1; i >= 0; i--) {
            locals[i] = pop(vm);
          }
          pop(vm); // pop callee

          const newFrame: CallFrame = { closure: callee, ip: 0, locals };
          vm.frames[vm.frameCount++] = newFrame;
          frame = newFrame;

        } else if (callee.kind === 'vm-builtin') {
          const args: VMValue[] = [];
          for (let i = arity - 1; i >= 0; i--) {
            args[i] = pop(vm);
          }
          pop(vm);
          const result = callee.fn(args);
          push(vm, result);

        } else if (callee.kind === 'adt-constructor') {
          if (arity !== callee.arity) {
            throw new VMError(
              `Constructor ${callee.name} expects ${callee.arity} arguments, got ${arity}`,
              getStackTrace(vm, frame)
            );
          }
          const args: VMValue[] = [];
          for (let i = arity - 1; i >= 0; i--) {
            args[i] = pop(vm);
          }
          pop(vm);
          push(vm, { kind: 'adt-value', constructor: callee.name, args });

        } else {
          throw new VMError(`Cannot call ${callee.kind}`, getStackTrace(vm, frame));
        }
        break;
      }

      case Op.TAIL_CALL: {
        const arity = inst.operand!;
        const callee = peek(vm, arity);

        if (typeof callee === 'number') {
          throw new VMError(`Cannot call number`, getStackTrace(vm, frame));
        }

        if (callee.kind === 'vm-closure') {
          if (arity !== callee.proto.arity) {
            throw new VMError(
              `Expected ${callee.proto.arity} arguments, got ${arity}`,
              getStackTrace(vm, frame)
            );
          }

          const locals = new Array(callee.proto.chunk.localCount).fill(NIL);
          for (let i = arity - 1; i >= 0; i--) {
            locals[i] = pop(vm);
          }
          pop(vm);

          frame.closure = callee;
          frame.ip = 0;
          frame.locals = locals;

        } else if (callee.kind === 'vm-builtin') {
          const args: VMValue[] = [];
          for (let i = arity - 1; i >= 0; i--) {
            args[i] = pop(vm);
          }
          pop(vm);
          const result = callee.fn(args);
          push(vm, result);
          vm.frameCount--;
          if (vm.frameCount <= baseFrameCount) return result;
          frame = vm.frames[vm.frameCount - 1];
          break;

        } else if (callee.kind === 'adt-constructor') {
          if (arity !== callee.arity) {
            throw new VMError(
              `Constructor ${callee.name} expects ${callee.arity} arguments, got ${arity}`,
              getStackTrace(vm, frame)
            );
          }
          const args: VMValue[] = [];
          for (let i = arity - 1; i >= 0; i--) {
            args[i] = pop(vm);
          }
          pop(vm);
          const result: VMValue = { kind: 'adt-value', constructor: callee.name, args };
          vm.frameCount--;
          if (vm.frameCount <= baseFrameCount) return result;
          frame = vm.frames[vm.frameCount - 1];
          push(vm, result);
          break;

        } else {
          throw new VMError(`Cannot call ${callee.kind}`, getStackTrace(vm, frame));
        }
        break;
      }

      case Op.CLOSURE: {
        const constant = frame.closure.proto.chunk.constants[inst.operand!];
        if (constant.kind !== 'prototype') {
          throw new VMError('CLOSURE expects a prototype constant', getStackTrace(vm, frame));
        }
        const proto = constant.proto;

        const upvalues: VMValue[] = new Array(proto.upvalues.length);
        for (let i = 0; i < proto.upvalues.length; i++) {
          const desc = proto.upvalues[i];
          if (desc.isLocal) {
            upvalues[i] = frame.locals[desc.index];
          } else {
            upvalues[i] = frame.closure.upvalues[desc.index];
          }
        }

        push(vm, { kind: 'vm-closure', proto, upvalues });
        break;
      }

      // --- ADTs ---

      case Op.CONSTRUCT: {
        const nameConst = frame.closure.proto.chunk.constants[inst.operand!];
        if (nameConst.kind !== 'string') throw new VMError('CONSTRUCT name must be string', getStackTrace(vm, frame));
        const arity = inst.operand2!;
        const args: VMValue[] = [];
        for (let i = arity - 1; i >= 0; i--) {
          args[i] = pop(vm);
        }
        push(vm, { kind: 'adt-value', constructor: nameConst.value, args });
        break;
      }

      case Op.GET_FIELD: {
        const fieldIdx = inst.operand!;
        const value = peek(vm);

        if (typeof value === 'number') {
          throw new VMError(`GET_FIELD on number`, getStackTrace(vm, frame));
        }

        if (value.kind === 'adt-value') {
          if (fieldIdx >= value.args.length) {
            throw new VMError(`Field index ${fieldIdx} out of bounds for ${value.constructor}`, getStackTrace(vm, frame));
          }
          push(vm, value.args[fieldIdx]);
        } else if (value.kind === 'list') {
          if (fieldIdx === 0) {
            if (value.elements.length === 0) {
              throw new VMError('Cannot get head of empty list', getStackTrace(vm, frame));
            }
            push(vm, value.elements[0]);
          } else if (fieldIdx === 1) {
            push(vm, { kind: 'list', elements: value.elements.slice(1) });
          } else {
            throw new VMError(`Invalid field index ${fieldIdx} for list`, getStackTrace(vm, frame));
          }
        } else {
          throw new VMError(`GET_FIELD on non-ADT value: ${value.kind}`, getStackTrace(vm, frame));
        }
        break;
      }

      case Op.MATCH_TAG: {
        const nameConst = frame.closure.proto.chunk.constants[inst.operand!];
        if (nameConst.kind !== 'string') throw new VMError('MATCH_TAG name must be string', getStackTrace(vm, frame));
        const target = inst.operand2!;
        const value = peek(vm);

        if (typeof value === 'number') {
          frame.ip = target; // numbers never match a tag
          break;
        }

        let matches = false;
        if (value.kind === 'adt-value' && value.constructor === nameConst.value) {
          matches = true;
        }
        if (value.kind === 'list') {
          if (nameConst.value === 'Cons' && value.elements.length > 0) matches = true;
          if (nameConst.value === 'Nil' && value.elements.length === 0) matches = true;
        }
        if (value.kind === 'nil' && nameConst.value === 'Nil') matches = true;

        if (!matches) {
          frame.ip = target;
        }
        break;
      }

      // --- Effects ---

      case Op.PUSH_HANDLER: {
        const descriptorConst = frame.closure.proto.chunk.constants[inst.operand!];
        if (descriptorConst.kind !== 'string' || !descriptorConst.value.startsWith('\0handler:')) {
          throw new VMError('Invalid handler descriptor', getStackTrace(vm, frame));
        }

        const descriptorJson = descriptorConst.value.substring('\0handler:'.length);
        const descriptor = JSON.parse(descriptorJson);

        // Create body closure from prototype
        const bodyProtoConst = frame.closure.proto.chunk.constants[descriptor.bodyProtoIdx];
        if (bodyProtoConst.kind !== 'prototype') throw new VMError('Expected body prototype', getStackTrace(vm, frame));
        const bodyClosure = createClosureFromProto(frame, bodyProtoConst.proto);

        // Create handler clause closures
        const handlers = new Map<string, { closure: VMClosure; arity: number }>();
        for (const h of descriptor.handlers) {
          const opNameConst = frame.closure.proto.chunk.constants[h.opNameIdx];
          const protoConst = frame.closure.proto.chunk.constants[h.protoIdx];
          if (opNameConst.kind !== 'string' || protoConst.kind !== 'prototype') {
            throw new VMError('Invalid handler clause', getStackTrace(vm, frame));
          }
          handlers.set(opNameConst.value, {
            closure: createClosureFromProto(frame, protoConst.proto),
            arity: h.arity,
          });
        }

        // Create return handler closure
        let returnClosure: VMClosure | undefined;
        if (descriptor.returnProtoIdx >= 0) {
          const returnProtoConst = frame.closure.proto.chunk.constants[descriptor.returnProtoIdx];
          if (returnProtoConst.kind !== 'prototype') throw new VMError('Expected return handler prototype', getStackTrace(vm, frame));
          returnClosure = createClosureFromProto(frame, returnProtoConst.proto);
        }

        // Run body with handler installed
        let result = runWithHandler(vm, { handlers, returnHandler: returnClosure }, bodyClosure, []);

        // Apply return handler to the final result
        if (returnClosure) {
          result = callClosure(vm, returnClosure, [result]);
        }

        push(vm, result);
        break;
      }

      case Op.POP_HANDLER: {
        // Currently unused — handler cleanup is managed by runWithHandler
        break;
      }

      case Op.PERFORM: {
        const nameConst = frame.closure.proto.chunk.constants[inst.operand!];
        if (nameConst.kind !== 'string') throw new VMError('PERFORM name must be string', getStackTrace(vm, frame));
        const arity = inst.operand2!;
        const opName = nameConst.value;

        const args: VMValue[] = [];
        for (let i = arity - 1; i >= 0; i--) {
          args[i] = pop(vm);
        }

        // Walk handler stack to find a matching handler
        for (let i = vm.handlerStack.length - 1; i >= 0; i--) {
          const hframe = vm.handlerStack[i];
          if (hframe.handlers.has(opName)) {
            // Capture continuation: frames and stack from handler's capture point to current
            const capturedFrames: CallFrame[] = [];
            for (let f = hframe.captureFrameStart; f < vm.frameCount; f++) {
              const fr = vm.frames[f];
              capturedFrames.push({ closure: fr.closure, ip: fr.ip, locals: [...fr.locals] });
            }
            const capturedStack = vm.stack.slice(hframe.captureStackStart, vm.stackTop);

            // Capture nested handler frames (between this handler and top of stack)
            const capturedHandlers = vm.handlerStack.slice(i + 1);

            const continuation: VMContinuation = {
              kind: 'vm-continuation',
              frames: capturedFrames,
              stack: capturedStack,
              handlerStack: capturedHandlers,
              resumed: false,
              frameBase: hframe.captureFrameStart,
              stackBase: hframe.captureStackStart,
            };

            // Unwind VM state to handler's installation point
            vm.frameCount = hframe.unwindFrameCount;
            vm.stackTop = hframe.unwindStackTop;
            vm.handlerStack.length = i; // remove this handler and everything above

            throw new EffectSignal(opName, args, continuation, hframe);
          }
        }

        // No user handler found — try default handlers
        const defaultResult = tryDefaultHandler(opName, args);
        if (defaultResult !== undefined) {
          push(vm, defaultResult);
          break;
        }

        throw new VMError(`Unhandled effect: ${opName}`, getStackTrace(vm, frame));
      }

      case Op.RESUME: {
        const value = pop(vm);
        const cont = pop(vm);

        if (typeof cont === 'number' || cont.kind !== 'vm-continuation') {
          throw new VMError('resume expects a continuation', getStackTrace(vm, frame));
        }

        if (cont.resumed) {
          throw new VMError('Continuation already resumed (one-shot)', getStackTrace(vm, frame));
        }
        cont.resumed = true;

        // Update the current (top) handler frame so that:
        // - captureStart: re-performed effects only capture body frames
        // - unwind: protects the current handler body's frame from being unwound
        const topHandler = vm.handlerStack[vm.handlerStack.length - 1];
        if (topHandler) {
          topHandler.captureFrameStart = vm.frameCount;
          topHandler.captureStackStart = vm.stackTop;
          topHandler.unwindFrameCount = vm.frameCount;
          topHandler.unwindStackTop = vm.stackTop;
        }

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

        // Push the resume value (this is what PERFORM "returns" to the body)
        push(vm, value);

        // Restore nested handlers from the continuation, rebasing their
        // frame/stack indices to the current restoration context
        const savedHandlerLen = vm.handlerStack.length;
        const restoredHandlers: VMHandlerFrame[] = [];
        const frameOffset = resumeBase - cont.frameBase;
        const stackOffset = (vm.stackTop - cont.stack.length - 1) - cont.stackBase;
        for (const h of cont.handlerStack as VMHandlerFrame[]) {
          const rebased: VMHandlerFrame = {
            handlers: h.handlers,
            returnHandler: h.returnHandler,
            unwindFrameCount: h.unwindFrameCount + frameOffset,
            unwindStackTop: h.unwindStackTop + stackOffset,
            captureFrameStart: h.captureFrameStart + frameOffset,
            captureStackStart: h.captureStackStart + stackOffset,
          };
          restoredHandlers.push(rebased);
          vm.handlerStack.push(rebased);
        }

        // Continue execution of the body, catching effects that target
        // restored handlers or the enclosing deep handler
        let result: VMValue;
        try {
          if (restoredHandlers.length === 0) {
            result = run(vm, resumeBase);
          } else {
            result = runRestoredContinuation(vm, resumeBase, restoredHandlers, savedHandlerLen);
          }
        } catch (e) {
          if (e instanceof EffectSignal && topHandler && e.targetHandler === topHandler) {
            // Body re-performed an effect handled by the enclosing handler.
            // Dispatch to handler clause with deep handler semantics so that
            // the result flows back to this RESUME (preserving the handler
            // body's pending computation like `(+ n (resume k nil))`).
            const clause = topHandler.handlers.get(e.opName);
            if (!clause) throw e;
            const expectedArgCount = clause.arity - 1;
            const handlerArgs: VMValue[] = [];
            for (let i = 0; i < expectedArgCount && i < e.args.length; i++) {
              handlerArgs.push(e.args[i]);
            }
            handlerArgs.push(e.continuation);
            result = runWithHandler(vm, {
              handlers: topHandler.handlers,
              returnHandler: topHandler.returnHandler,
            }, clause.closure, handlerArgs);
          } else {
            throw e;
          }
        }

        // Restore VM state: frame count and handler stack may have been
        // modified by handler dispatch within the restored continuation
        vm.frameCount = resumeBase;
        vm.handlerStack.length = savedHandlerLen;

        push(vm, result);
        break;
      }

      case Op.HALT: {
        const msg = pop(vm);
        if (typeof msg !== 'number' && msg.kind === 'string') {
          throw new VMError(msg.value, getStackTrace(vm, frame));
        }
        throw new VMError('HALT', getStackTrace(vm, frame));
      }

      default:
        throw new VMError(`Unknown opcode: ${inst.op} (${OpName[inst.op] ?? '?'})`, getStackTrace(vm, frame));
    }
  }
}

// --- Default Effect Handlers ---

function tryDefaultHandler(opName: string, args: VMValue[]): VMValue | undefined {
  switch (opName) {
    case 'print': {
      const msg = args[0];
      if (msg !== undefined && typeof msg !== 'number' && msg.kind === 'string') {
        process.stdout.write(msg.value);
      } else {
        process.stdout.write(formatVMValue(msg ?? NIL));
      }
      return NIL;
    }
    case 'println': {
      const msg = args[0];
      if (msg !== undefined && typeof msg !== 'number' && msg.kind === 'string') {
        console.log(msg.value);
      } else {
        console.log(formatVMValue(msg ?? NIL));
      }
      return NIL;
    }
    case 'read-line': {
      return { kind: 'string', value: prompt('') ?? '' };
    }
    case 'read-file': {
      const path = args[0];
      if (path === undefined || typeof path === 'number' || path.kind !== 'string') {
        throw new Error('read-file: path must be a string');
      }
      return { kind: 'string', value: fs.readFileSync(path.value, 'utf-8') };
    }
    case 'write-file': {
      const path = args[0];
      const content = args[1];
      if (path === undefined || typeof path === 'number' || path.kind !== 'string') {
        throw new Error('write-file: path must be a string');
      }
      if (content === undefined || typeof content === 'number' || content.kind !== 'string') {
        throw new Error('write-file: content must be a string');
      }
      fs.writeFileSync(path.value, content.value);
      return NIL;
    }
    default:
      return undefined;
  }
}

// --- Error Handling ---

export type StackFrame = {
  functionName: string;
  line: number;
  file?: string;
};

function getStackTrace(vm: VM, currentFrame: CallFrame): StackFrame[] {
  const trace: StackFrame[] = [];
  for (let i = vm.frameCount - 1; i >= 0; i--) {
    const f = i === vm.frameCount - 1 ? currentFrame : vm.frames[i];
    const ip = Math.max(0, f.ip - 1);
    const chunk = f.closure.proto.chunk;
    const line = chunk.code[ip]?.line ?? 0;
    trace.push({
      functionName: f.closure.proto.name,
      line,
      file: chunk.sourceFile,
    });
  }
  return trace;
}

export class VMError extends Error {
  stackTrace: StackFrame[];

  constructor(message: string, stackTrace: StackFrame[]) {
    super(message);
    this.name = 'VMError';
    this.stackTrace = stackTrace;
  }

  formatTrace(): string {
    return this.stackTrace
      .map(f => `  at ${f.functionName} (${f.file ?? '<unknown>'}:${f.line})`)
      .join('\n');
  }
}

// --- Value Formatting ---

export function formatVMValue(value: VMValue): string {
  if (typeof value === 'number') return String(value);
  switch (value.kind) {
    case 'boolean': return value.value ? '#t' : '#f';
    case 'nil': return 'nil';
    case 'string': return `"${value.value}"`;
    case 'list': return `(${value.elements.map(formatVMValue).join(' ')})`;
    case 'adt-value':
      if (value.args.length === 0) return value.constructor;
      return `(${value.constructor} ${value.args.map(formatVMValue).join(' ')})`;
    case 'adt-constructor': return `#<constructor ${value.name}/${value.arity}>`;
    case 'vm-closure': return `#<vm-closure ${value.proto.name}>`;
    case 'vm-builtin': return `#<vm-builtin ${value.name}>`;
    case 'vm-continuation': return '#<continuation>';
    case 'ffi-library': return `#<ffi-library ${value.path}>`;
    case 'ffi-pointer': return `#<ffi-pointer 0x${value.ptr.toString(16)}>`;
    case 'ffi-struct-def': return `#<ffi-struct-def ${value.def.name} (${value.def.size} bytes)>`;
    case 'ffi-struct': return `#<ffi-struct ${value.def.name}>`;
  }
}

// --- Value Conversion ---

function toRuntimeValue(v: VMValue): RuntimeValue {
  if (typeof v === 'number') return { kind: 'number', value: v };
  switch (v.kind) {
    case 'boolean': return v;
    case 'nil': return v;
    case 'string': return v;
    case 'list': return { kind: 'list', elements: v.elements.map(toRuntimeValue) };
    case 'adt-value': return { kind: 'adt-value', constructor: v.constructor, args: v.args.map(toRuntimeValue) };
    case 'adt-constructor': return v;
    case 'vm-closure': return { kind: 'function', params: [], body: [], closure: { bindings: new Map(), parent: null } };
    case 'vm-builtin': return { kind: 'builtin', name: v.name, fn: () => ({ kind: 'nil' }) };
    case 'vm-continuation': return { kind: 'nil' };
    case 'ffi-library': return { kind: 'ffi-library', path: v.path, lib: v.lib, signatures: v.signatures };
    case 'ffi-pointer': return { kind: 'ffi-pointer', ptr: v.ptr };
    case 'ffi-struct-def': return { kind: 'ffi-struct-def', def: v.def };
    case 'ffi-struct': return { kind: 'ffi-struct', def: v.def, buffer: v.buffer };
  }
}
