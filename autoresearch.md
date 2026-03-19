# Autoresearch: Optimize marlang fib(40) to be comparable to bun

## Objective
Optimize the Marlang VM to run `bench/fib40.mar` (recursive fibonacci of 40) at a comparable speed to `bun bench/fib40.ts`. Currently marlang takes ~38s vs bun's ~0.37s (100x slower).

The fib40 benchmark is a pure recursive function — no effects, no ADTs, no closures with captures. The hot path is: integer comparison, integer arithmetic, function calls (non-tail recursive), and returns.

## Metrics
- **Primary**: `fib40_s` (seconds, lower is better)
- **Secondary**: `bun_s` (bun reference time)

## How to Run
`./autoresearch.sh` — outputs `METRIC name=number` lines.

## Files in Scope
- `port/crates/vm/src/vm.rs` — Main VM execution loop (the `run()` function)
- `port/crates/vm/src/compiler.rs` — Bytecode compiler
- `port/crates/vm/src/bytecode.rs` — Opcode definitions and Instruction struct
- `port/crates/vm/src/value.rs` — Value representation (16-byte tagged union)
- `port/crates/vm/src/object.rs` — VMObject, Closure, CallFrame definitions
- `port/crates/vm/src/jit/` — Cranelift JIT compiler (mod.rs, engine.rs, translate.rs, runtime.rs)
- `bench/fib40.mar` — The benchmark program
- `bench/fib40.ts` — Bun reference

## Off Limits
- `bench/fib40.mar` and `bench/fib40.ts` — DO NOT modify the benchmarks
- Type checker, parser, lexer — not relevant to runtime performance
- Correctness — all existing tests must pass

## Constraints
- `cargo test --workspace` must pass
- `./bench/run.sh` results must not regress significantly
- No cheating: optimizations must be general-purpose, not fib-specific
- The fib function uses `Op::Call` (non-tail recursive), not `Op::TailCall`

## Architecture Notes
- The JIT currently REJECTS `Op::Call` in `can_jit()` — only self-recursive TailCalls work
- This means fib runs entirely in the interpreter (~38s)
- The JIT's translator already has `emit_self_call` and `emit_call` for Call opcodes
- Enabling JIT for self-recursive Calls would give native recursion with direct function calls
- The interpreter's hot path: PUSH_GLOBAL "fib" does HashMap lookup every call (~400M lookups)
- Value representation is 16 bytes (tag u8 + 7 padding + u64 payload)
- Each Call clones the closure (Arc increment) and allocates/reuses a locals Vec

## What's Been Tried
(Nothing yet — starting fresh)
