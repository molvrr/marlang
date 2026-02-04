// Shared AST types
export type LispValue =
  | { kind: 'symbol', value: string }
  | { kind: 'number', value: number }
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

// Runtime types
export type RuntimeValue =
  | { kind: 'number', value: number }
  | { kind: 'boolean', value: boolean }
  | { kind: 'nil' }
  | { kind: 'string', value: string }
  | { kind: 'list', elements: RuntimeValue[] }
  | { kind: 'function', params: string[], body: Expr, closure: Env }
  | { kind: 'builtin', name: string, fn: (args: RuntimeValue[]) => RuntimeValue }
  | { kind: 'ffi-library', path: string, lib: any, signatures: Map<string, FFISignature> }
  | { kind: 'ffi-pointer', ptr: number }
  | { kind: 'environment', env: Env }

export type Env = {
  bindings: Map<string, RuntimeValue>;
  parent: Env | null;
}
