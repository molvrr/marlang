// Public API for the Marlang bytecode VM

export { compile, compileProgram } from './compiler';
export { createVM, interpret, VMError, formatVMValue } from './vm';
export type { VM, CallFrame, StackFrame } from './vm';
export { disassemble } from './debug';
export { Op, OpName } from './bytecode';
export type { Chunk, FunctionPrototype, VMValue, VMClosure, VMBuiltin, Instruction, OpCode } from './bytecode';
