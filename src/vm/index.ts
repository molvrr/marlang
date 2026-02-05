// Public API for the Marlang bytecode VM

export { compile, compileProgram } from './compiler';
export { createVM, interpret, VMError, formatVMValue, EffectSignal, callClosure, resumeContinuation, push, pop, isTruthy, run } from './vm';
export type { VM, StackFrame, VMHandlerFrame } from './vm';
export { disassemble } from './debug';
export { Op, OpName } from './bytecode';
export type { Chunk, FunctionPrototype, VMValue, VMObject, VMClosure, VMBuiltin, VMContinuation, VMPid, VMChannel, VMFFILibrary, VMFFIPointer, VMFFIStructDef, VMFFIStruct, CallFrame, Instruction, OpCode } from './bytecode';
export { NIL, TRUE, FALSE } from './bytecode';
export { runFibers } from './scheduler';
