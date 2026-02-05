// Stack-based VM for Marlang

import { Op, OpName, type Chunk, type FunctionPrototype, type VMValue, type VMClosure, type VMBuiltin, NIL } from './bytecode';
import type { RuntimeValue } from '../core/types';
import { createBuiltinGlobals } from './builtins';

// --- VM Types ---

export type CallFrame = {
  closure: VMClosure;
  ip: number;
  locals: VMValue[];
};

export type VM = {
  stack: VMValue[];
  stackTop: number;
  frames: CallFrame[];
  frameCount: number;
  globals: Map<string, VMValue>;
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
  if (value.kind === 'nil') return false;
  if (value.kind === 'boolean' && !value.value) return false;
  return true;
}

// --- Equality ---

function valuesEqual(a: VMValue, b: VMValue): boolean {
  if (a.kind === 'nil' && b.kind === 'nil') return true;
  if (a.kind === 'boolean' && b.kind === 'boolean') return a.value === b.value;
  if (a.kind === 'number' && b.kind === 'number') return a.value === b.value;
  if (a.kind === 'string' && b.kind === 'string') return a.value === b.value;
  if (a.kind === 'adt-value' && b.kind === 'adt-value') {
    if (a.constructor !== b.constructor) return false;
    if (a.args.length !== b.args.length) return false;
    return a.args.every((arg, i) => valuesEqual(arg, b.args[i]));
  }
  // Lists
  if (a.kind === 'list' && b.kind === 'list') {
    if (a.elements.length !== b.elements.length) return false;
    return a.elements.every((el, i) => valuesEqual(el, b.elements[i]));
  }
  // Nil list match
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
  };
}

// --- Interpret ---

export function interpret(vm: VM, chunk: Chunk): RuntimeValue {
  // Wrap the chunk in a script closure
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

  // Push initial frame
  const frame: CallFrame = {
    closure: scriptClosure,
    ip: 0,
    locals: new Array(chunk.localCount).fill(NIL),
  };

  vm.frames[0] = frame;
  vm.frameCount = 1;

  const result = run(vm);
  return toRuntimeValue(result);
}

// --- Main Execution Loop ---

function run(vm: VM): VMValue {
  let frame = vm.frames[vm.frameCount - 1];

  while (true) {
    const inst = frame.closure.proto.chunk.code[frame.ip++];

    switch (inst.op) {
      case Op.PUSH_CONST: {
        const constant = frame.closure.proto.chunk.constants[inst.operand!];
        switch (constant.kind) {
          case 'number': push(vm, { kind: 'number', value: constant.value }); break;
          case 'string': push(vm, { kind: 'string', value: constant.value }); break;
          case 'boolean': push(vm, { kind: 'boolean', value: constant.value }); break;
          case 'nil': push(vm, NIL); break;
          case 'prototype':
            // Shouldn't happen directly, prototypes are used via CLOSURE
            throw new VMError('Cannot push prototype as value', getStackTrace(vm, frame));
        }
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

        // Handle special constructor encoding from compiler
        if (value.kind === 'string' && value.value.startsWith('\0ctor:')) {
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
        if (a.kind === 'number' && b.kind === 'number') {
          push(vm, { kind: 'number', value: a.value + b.value });
        } else if (a.kind === 'string' && b.kind === 'string') {
          push(vm, { kind: 'string', value: a.value + b.value });
        } else {
          throw new VMError(`Cannot add ${a.kind} and ${b.kind}`, getStackTrace(vm, frame));
        }
        break;
      }

      case Op.SUB: {
        const b = pop(vm);
        const a = pop(vm);
        if (a.kind !== 'number' || b.kind !== 'number') {
          throw new VMError('Subtraction requires numbers', getStackTrace(vm, frame));
        }
        push(vm, { kind: 'number', value: a.value - b.value });
        break;
      }

      case Op.MUL: {
        const b = pop(vm);
        const a = pop(vm);
        if (a.kind !== 'number' || b.kind !== 'number') {
          throw new VMError('Multiplication requires numbers', getStackTrace(vm, frame));
        }
        push(vm, { kind: 'number', value: a.value * b.value });
        break;
      }

      case Op.DIV: {
        const b = pop(vm);
        const a = pop(vm);
        if (a.kind !== 'number' || b.kind !== 'number') {
          throw new VMError('Division requires numbers', getStackTrace(vm, frame));
        }
        push(vm, { kind: 'number', value: a.value / b.value });
        break;
      }

      case Op.NEG: {
        const a = pop(vm);
        if (a.kind !== 'number') throw new VMError('Negate requires number', getStackTrace(vm, frame));
        push(vm, { kind: 'number', value: -a.value });
        break;
      }

      // --- Comparison ---

      case Op.EQ: {
        const b = pop(vm);
        const a = pop(vm);
        push(vm, { kind: 'boolean', value: valuesEqual(a, b) });
        break;
      }

      case Op.LT: {
        const b = pop(vm);
        const a = pop(vm);
        if (a.kind !== 'number' || b.kind !== 'number') {
          throw new VMError('< requires numbers', getStackTrace(vm, frame));
        }
        push(vm, { kind: 'boolean', value: a.value < b.value });
        break;
      }

      case Op.GT: {
        const b = pop(vm);
        const a = pop(vm);
        if (a.kind !== 'number' || b.kind !== 'number') {
          throw new VMError('> requires numbers', getStackTrace(vm, frame));
        }
        push(vm, { kind: 'boolean', value: a.value > b.value });
        break;
      }

      case Op.LE: {
        const b = pop(vm);
        const a = pop(vm);
        if (a.kind !== 'number' || b.kind !== 'number') {
          throw new VMError('<= requires numbers', getStackTrace(vm, frame));
        }
        push(vm, { kind: 'boolean', value: a.value <= b.value });
        break;
      }

      case Op.GE: {
        const b = pop(vm);
        const a = pop(vm);
        if (a.kind !== 'number' || b.kind !== 'number') {
          throw new VMError('>= requires numbers', getStackTrace(vm, frame));
        }
        push(vm, { kind: 'boolean', value: a.value >= b.value });
        break;
      }

      case Op.NOT: {
        const a = pop(vm);
        push(vm, { kind: 'boolean', value: !isTruthy(a) });
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

        if (vm.frameCount === 0) {
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

        if (callee.kind === 'vm-closure') {
          if (arity !== callee.proto.arity) {
            throw new VMError(
              `Expected ${callee.proto.arity} arguments, got ${arity}`,
              getStackTrace(vm, frame)
            );
          }

          // Collect arguments
          const locals = new Array(callee.proto.chunk.localCount).fill(NIL);
          for (let i = arity - 1; i >= 0; i--) {
            locals[i] = pop(vm);
          }
          pop(vm); // pop callee

          // Push new frame
          const newFrame: CallFrame = {
            closure: callee,
            ip: 0,
            locals,
          };
          vm.frames[vm.frameCount++] = newFrame;
          frame = newFrame;

        } else if (callee.kind === 'vm-builtin') {
          const args: VMValue[] = [];
          for (let i = arity - 1; i >= 0; i--) {
            args[i] = pop(vm);
          }
          pop(vm); // pop callee
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
          pop(vm); // pop callee
          push(vm, { kind: 'adt-value', constructor: callee.name, args });

        } else {
          throw new VMError(`Cannot call ${callee.kind}`, getStackTrace(vm, frame));
        }
        break;
      }

      case Op.TAIL_CALL: {
        const arity = inst.operand!;
        const callee = peek(vm, arity);

        if (callee.kind === 'vm-closure') {
          if (arity !== callee.proto.arity) {
            throw new VMError(
              `Expected ${callee.proto.arity} arguments, got ${arity}`,
              getStackTrace(vm, frame)
            );
          }

          // Collect arguments
          const locals = new Array(callee.proto.chunk.localCount).fill(NIL);
          for (let i = arity - 1; i >= 0; i--) {
            locals[i] = pop(vm);
          }
          pop(vm); // pop callee

          // Reuse current frame
          frame.closure = callee;
          frame.ip = 0;
          frame.locals = locals;

        } else if (callee.kind === 'vm-builtin') {
          // Can't tail-call a builtin in the traditional sense, just call it
          const args: VMValue[] = [];
          for (let i = arity - 1; i >= 0; i--) {
            args[i] = pop(vm);
          }
          pop(vm);
          const result = callee.fn(args);
          push(vm, result);
          // Now we need to return this result
          // Since this was a tail call, we should return from the current frame
          vm.frameCount--;
          if (vm.frameCount === 0) return result;
          frame = vm.frames[vm.frameCount - 1];
          // result is already on stack from push above... actually no.
          // The push went to the current frame's stack. After popping the frame,
          // we need to push to the parent. Let me fix this.
          // Actually, stackTop is global, so the push is already visible to the parent frame.
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
          // Return from current frame with this result
          vm.frameCount--;
          if (vm.frameCount === 0) return result;
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

        // Capture upvalues
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

        if (value.kind === 'adt-value') {
          if (fieldIdx >= value.args.length) {
            throw new VMError(`Field index ${fieldIdx} out of bounds for ${value.constructor}`, getStackTrace(vm, frame));
          }
          push(vm, value.args[fieldIdx]);
        } else if (value.kind === 'list') {
          // Support Cons-style field access on lists
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

        let matches = false;
        if (value.kind === 'adt-value' && value.constructor === nameConst.value) {
          matches = true;
        }
        // Match list against Cons/Nil
        if (value.kind === 'list') {
          if (nameConst.value === 'Cons' && value.elements.length > 0) matches = true;
          if (nameConst.value === 'Nil' && value.elements.length === 0) matches = true;
        }
        // Match nil against Nil
        if (value.kind === 'nil' && nameConst.value === 'Nil') matches = true;

        if (!matches) {
          frame.ip = target;
        }
        break;
      }

      // --- Effects (Phase 2 stubs) ---

      case Op.PERFORM: {
        const nameConst = frame.closure.proto.chunk.constants[inst.operand!];
        if (nameConst.kind !== 'string') throw new VMError('PERFORM name must be string', getStackTrace(vm, frame));
        const arity = inst.operand2!;
        const opName = nameConst.value;

        // Collect arguments
        const args: VMValue[] = [];
        for (let i = arity - 1; i >= 0; i--) {
          args[i] = pop(vm);
        }

        // Check for default handlers (IO operations)
        const defaultResult = tryDefaultHandler(opName, args);
        if (defaultResult !== undefined) {
          push(vm, defaultResult);
          break;
        }

        throw new VMError(`Unhandled effect: ${opName}`, getStackTrace(vm, frame));
      }

      case Op.PUSH_HANDLER:
      case Op.POP_HANDLER:
      case Op.RESUME: {
        throw new VMError('Effects not yet implemented in VM', getStackTrace(vm, frame));
      }

      case Op.HALT: {
        // Check if there's an error message on the stack
        const msg = pop(vm);
        if (msg.kind === 'string') {
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
      if (msg?.kind === 'string') {
        process.stdout.write(msg.value);
      } else {
        process.stdout.write(formatVMValue(msg ?? NIL));
      }
      return NIL;
    }
    case 'println': {
      const msg = args[0];
      if (msg?.kind === 'string') {
        console.log(msg.value);
      } else {
        console.log(formatVMValue(msg ?? NIL));
      }
      return NIL;
    }
    case 'read-line': {
      // Synchronous readline not easily available; return empty string for now
      return { kind: 'string', value: '' };
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
  switch (value.kind) {
    case 'number': return String(value.value);
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
  }
}

// --- Value Conversion ---

function toRuntimeValue(v: VMValue): RuntimeValue {
  switch (v.kind) {
    case 'number': return v;
    case 'boolean': return v;
    case 'nil': return v;
    case 'string': return v;
    case 'list': return { kind: 'list', elements: v.elements.map(toRuntimeValue) };
    case 'adt-value': return { kind: 'adt-value', constructor: v.constructor, args: v.args.map(toRuntimeValue) };
    case 'adt-constructor': return v;
    case 'vm-closure': return { kind: 'function', params: [], body: [], closure: { bindings: new Map(), parent: null } };
    case 'vm-builtin': return { kind: 'builtin', name: v.name, fn: () => ({ kind: 'nil' }) };
  }
}
