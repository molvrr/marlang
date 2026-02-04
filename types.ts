// Shared AST types
export type LispValue =
  | { kind: 'symbol', value: string }
  | { kind: 'number', value: number, isFloat?: boolean }
  | { kind: 'string', value: string }

export type Expr =
  | { kind: 'atom', value: LispValue }
  | { kind: 'list', head: Expr, tail: Expr }
  | { kind: 'nil' }

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
