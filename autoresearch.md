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

### Wins
1. **Enable JIT for self-recursive Op::Call** (37.58s → 0.76s, 49x): Changed `can_jit()` to allow Call for self-recursive call sites, using `emit_self_call` for direct native recursion.
2. **Skip dead PushGlobal for self-callees**: Replaced runtime HashMap lookup with dummy nil for self-references that will be discarded by Call.
3. **Fuse comparison+JumpIfFalse peephole**: Merged LtInt+JumpIfFalse into single `icmp+brif` in JIT translator.
4. **Cache local SSA values within blocks**: Avoid redundant loads from locals_ptr after function calls.
5. **Specialized fast inner function** (0.76s → 0.40s, 1.9x): For integer-pure recursive functions, generate a lean inner function with `(i64,...) -> i64` signature — no tags, no bail checks, no vm/closure pointers.
6. **Restrict JIT Call to integer-pure only**: Reverted general Call support to avoid regressions on ADT-heavy benchmarks (MatchTag/GetField runtime calls are slower than interpreter inline handling).
7. **Cranelift flags**: Disabled frame pointer preservation and stack probes for tighter native code.
8. **Lower JIT threshold**: 500 → 100 calls for faster compilation trigger.
9. **JUMP-to-RETURN peephole** (0.40s → 0.38s): Direct return in base case enables Cranelift shrink-wrapping (base case skips function prologue).

### Dead ends
- Pre-allocating VM stack/frames: negligible impact
- Cranelift alias analysis: no effect (fast function has no memory ops)
- Very low JIT threshold (10): marginal improvement, not worth the risk
