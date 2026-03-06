//! Cranelift-based method JIT compiler for the Marlang VM.
//!
//! The JIT compiles hot bytecode functions to native code via Cranelift.
//! Functions are compiled when they cross a configurable call-count threshold.
//! The compiled code uses the same `Value` representation as the interpreter,
//! calling back into runtime helpers for complex operations (ADTs, effects,
//! builtins, globals).
//!
//! **Architecture:**
//! - The `JitEngine` owns a Cranelift `JITModule` and compiles functions on demand.
//! - Compiled functions have signature: `(vm: *mut u8, closure: *mut u8) -> [u64; 2]`
//!   where the return is `(tag, payload)` — the raw components of a `Value`.
//! - The VM checks `FunctionPrototype.jit_code` before entering the interpreter loop.
//! - Effects, mutable upvalues, and variadic functions bail out to the interpreter.

mod engine;
pub(crate) mod runtime;
mod translate;

pub use crate::jit::engine::{JitEngine, JitFn};

/// Call count threshold before a function is JIT-compiled.
/// This balances compilation cost against execution savings.
pub const JIT_THRESHOLD: u32 = 500;
