// Bytecode IR and Instruction Set for Marlang VM

// Numeric opcodes for fast switch dispatch
export const Op = {
  // Stack manipulation
  PUSH_CONST:    0,   // [idx] Push constant from pool
  PUSH_LOCAL:    1,   // [slot] Push local variable from frame
  SET_LOCAL:     2,   // [slot] Pop TOS, store in local slot
  PUSH_UPVALUE:  3,   // [idx] Push captured variable
  PUSH_GLOBAL:   4,   // [name_idx] Push global (name is string constant)
  SET_GLOBAL:    5,   // [name_idx] Pop TOS, store as global
  POP:           6,   // Discard top of stack
  DUP:           7,   // Duplicate top of stack

  // Arithmetic (binary: pop 2, push 1)
  ADD:           10,
  SUB:           11,
  MUL:           12,
  DIV:           13,
  NEG:           14,  // Unary: pop 1, push 1

  // Comparison (binary: pop 2, push boolean)
  EQ:            20,
  LT:            21,
  GT:            22,
  LE:            23,
  GE:            24,
  NOT:           25,  // Unary boolean not

  // Control flow
  JUMP:          30,  // [target] Unconditional jump to absolute position
  JUMP_IF_FALSE: 31,  // [target] Pop TOS, jump if falsy
  RETURN:        33,  // Return TOS from current frame

  // Functions
  CALL:          40,  // [arity] Call function on stack
  TAIL_CALL:     41,  // [arity] Tail call (reuse frame)
  CLOSURE:       42,  // [proto_idx] Create closure from prototype constant

  // ADTs
  CONSTRUCT:     50,  // [name_idx, arity] Pop arity args, push ADT value
  GET_FIELD:     51,  // [field_idx] Peek ADT on TOS, push field value
  MATCH_TAG:     52,  // [name_idx, target] Peek TOS, jump if tag mismatch

  // Effects (Phase 2)
  PUSH_HANDLER:  60,  // [handler_info_idx] Install handler frame
  POP_HANDLER:   61,  // Remove handler frame
  PERFORM:       62,  // [name_idx, arity] Perform effect operation
  RESUME:        63,  // Resume continuation with TOS value

  HALT:          99,  // Stop execution
} as const;

export type OpCode = (typeof Op)[keyof typeof Op];

// Reverse lookup for disassembly
export const OpName: Record<number, string> = {};
for (const [name, code] of Object.entries(Op)) {
  OpName[code as number] = name;
}

export type Instruction = {
  op: OpCode;
  operand?: number;
  operand2?: number;
  line: number;
};

export type ConstantValue =
  | { kind: 'number'; value: number }
  | { kind: 'string'; value: string }
  | { kind: 'boolean'; value: boolean }
  | { kind: 'nil' }
  | { kind: 'prototype'; proto: FunctionPrototype };

export type Chunk = {
  code: Instruction[];
  constants: ConstantValue[];
  localCount: number;
  sourceFile?: string;
  vmConstants?: VMValue[];  // Pre-converted constants for fast PUSH_CONST
};

export type FunctionPrototype = {
  name: string;
  arity: number;
  upvalues: UpvalueDescriptor[];
  chunk: Chunk;
};

export type UpvalueDescriptor = {
  isLocal: boolean;  // true = from enclosing local, false = from enclosing upvalue
  index: number;
};

// --- VM Runtime Value Types ---

export type CallFrame = {
  closure: VMClosure;
  ip: number;
  locals: VMValue[];
};

export type VMClosure = {
  kind: 'vm-closure';
  proto: FunctionPrototype;
  upvalues: VMValue[];
};

export type VMBuiltin = {
  kind: 'vm-builtin';
  name: string;
  fn: (args: VMValue[], vm?: any) => VMValue;
};

export type VMPid = {
  kind: 'pid';
  id: number;
};

export type VMChannel = {
  kind: 'channel';
  id: number;
};

export type VMContinuation = {
  kind: 'vm-continuation';
  frames: CallFrame[];
  stack: VMValue[];
  handlerStack: unknown[];
  resumed: boolean;
  frameBase: number;  // Frame index that frames[0] was captured from
  stackBase: number;  // Stack index that stack[0] was captured from
};

// Unboxed numbers: raw JS numbers avoid heap allocation.
// JS engines (V8, JSC) optimize `typeof x === 'number'` into a single type-tag check.
export type VMObject =
  | { kind: 'boolean'; value: boolean }
  | { kind: 'nil' }
  | { kind: 'string'; value: string }
  | { kind: 'list'; elements: VMValue[] }
  | { kind: 'adt-value'; constructor: string; args: VMValue[] }
  | { kind: 'adt-constructor'; name: string; arity: number }
  | VMClosure
  | VMBuiltin
  | VMContinuation
  | VMPid
  | VMChannel
  | VMFFILibrary
  | VMFFIPointer
  | VMFFIStructDef
  | VMFFIStruct;

// FFI types — reuse StructDef/FFISignature from core/types.ts
import type { FFISignature, StructDef } from '../core/types';

export type VMFFILibrary = {
  kind: 'ffi-library';
  path: string;
  lib: any;
  signatures: Map<string, FFISignature>;
};

export type VMFFIPointer = {
  kind: 'ffi-pointer';
  ptr: number;
};

export type VMFFIStructDef = {
  kind: 'ffi-struct-def';
  def: StructDef;
};

export type VMFFIStruct = {
  kind: 'ffi-struct';
  def: StructDef;
  buffer: Uint8Array;
};

export type VMValue = number | VMObject;

export const NIL: VMObject = { kind: 'nil' };
export const TRUE: VMObject = { kind: 'boolean', value: true };
export const FALSE: VMObject = { kind: 'boolean', value: false };

/** Get a displayable kind string for error messages */
export function vmKind(v: VMValue): string {
  return typeof v === 'number' ? 'number' : v.kind;
}

/** Pre-convert a constant pool to VMValues (numbers become raw JS numbers) */
export function preconvertConstants(constants: ConstantValue[]): VMValue[] {
  const out: VMValue[] = new Array(constants.length);
  for (let i = 0; i < constants.length; i++) {
    const c = constants[i];
    switch (c.kind) {
      case 'number': out[i] = c.value; break;
      case 'string': out[i] = { kind: 'string', value: c.value }; break;
      case 'boolean': out[i] = c.value ? TRUE : FALSE; break;
      case 'nil': out[i] = NIL; break;
      case 'prototype': out[i] = NIL; break; // placeholder — prototypes accessed via constants[]
    }
  }
  return out;
}
