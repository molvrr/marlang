// Shared AST types

// Source location for error reporting and LSP
export type SourceLocation = {
  start: { line: number; column: number; offset: number };
  end: { line: number; column: number; offset: number };
};

export type LispValue =
  | { kind: 'symbol', value: string }
  | { kind: 'number', value: number, isFloat?: boolean }
  | { kind: 'string', value: string }

export type Expr =
  | { kind: 'atom', value: LispValue, loc?: SourceLocation }
  | { kind: 'list', head: Expr, tail: Expr, loc?: SourceLocation }
  | { kind: 'nil', loc?: SourceLocation }

// FFI types
export type FFISignature = {
  args: string[];
  returns: string;
}

// Struct field definition
export type StructField = {
  name: string;
  type: string;
  offset: number;
  size: number;
}

// Struct definition
export type StructDef = {
  name: string;
  fields: StructField[];
  size: number;
}

// Runtime types
export type RuntimeValue =
  | { kind: 'number', value: number }
  | { kind: 'boolean', value: boolean }
  | { kind: 'nil' }
  | { kind: 'string', value: string }
  | { kind: 'list', elements: RuntimeValue[] }
  | { kind: 'function', params: string[], body: Expr[], closure: Env }
  | { kind: 'builtin', name: string, fn: (args: RuntimeValue[]) => RuntimeValue }
  | { kind: 'ffi-library', path: string, lib: any, signatures: Map<string, FFISignature> }
  | { kind: 'ffi-pointer', ptr: number }
  | { kind: 'ffi-struct-def', def: StructDef }
  | { kind: 'ffi-struct', def: StructDef, buffer: Uint8Array }
  | { kind: 'environment', env: Env }
  | { kind: 'adt-value', constructor: string, args: RuntimeValue[] }
  | { kind: 'adt-constructor', name: string, arity: number }
  | { kind: 'continuation', cont: Continuation }
  | { kind: 'pid', id: number }
  | { kind: 'channel', id: number }

export type Env = {
  bindings: Map<string, RuntimeValue>;
  parent: Env | null;
}

// Tail call thunk for trampolining
export type TailCall = {
  kind: 'tail-call';
  fn: RuntimeValue;
  args: RuntimeValue[];
}

// Evaluation can return a value or a tail call thunk
export type EvalOutcome = RuntimeValue | TailCall;

// Effect system types

// An effect suspension represents a performed effect waiting for a handler
export type EffectSuspension = {
  kind: 'effect-suspension';
  effectName: string;
  opName: string;
  args: RuntimeValue[];
  continuation: Continuation;
}

// A continuation captures the computation up to a handler prompt
export type Continuation = {
  kind: 'continuation';
  // The continuation is represented as a function that takes a value
  // and returns an EvalOutcome (possibly suspending again)
  resume: (value: RuntimeValue) => Generator<EffectSuspension, RuntimeValue, RuntimeValue>;
}
