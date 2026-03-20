# Autoresearch: Optimize effect-heavy Marlang code

## Objective
Optimize the Marlang VM's effect handling so that effect-heavy code (JSON parsing, codec operations, etc.) runs significantly faster. The JSON benchmark uses the codec library which performs thousands of `perform`/`resume` cycles per parse — every character peek/consume goes through the effect system.

The goal is to make the effect optimization **generic** so every effect-heavy function benefits, not just specific effects.

## Metrics
- **Primary**: `total_ms` (effects_ms + json_ms, lower is better)
- **Secondary**: `effects_ms`, `json_ms`, `fib_ms`, `pattern_ms`

## How to Run
`./autoresearch.sh` — outputs `METRIC name=number` lines.

## Architecture: How Effects Work

### Effect lifecycle (Perform → Handler → Resume)
1. `Op::Perform` walks the handler stack to find a matching handler (string comparison per entry)
2. For **tail-resumptive** handlers: calls the resume-value closure inline (no continuation capture)
3. For **non-tail-resumptive** handlers: captures full continuation (clones frames, stack, handlers), unwinds, calls handler clause, then `Op::Resume` restores everything

### Key files
- `port/crates/vm/src/vm.rs` — VM execution loop, Perform/Resume/Handle opcodes, handler stack
- `port/crates/vm/src/compiler.rs` — Compiles `handle` forms, tail-resumptive analysis, handler descriptors
- `port/crates/vm/src/bytecode.rs` — Opcode definitions
- `port/crates/vm/src/object.rs` — HandlerEntry, VMHandlerFrame, VMContinuation, CallFrame
- `port/crates/vm/src/value.rs` — Value representation
- `stdlib/codec.mar` — Codec library (ParseState/PrintState effects, combinators)
- `stdlib/json.mar` — JSON codec built on codec library

### Hot path for JSON parsing
Every character consumed during parsing does:
```
(perform consume) → walk handler stack → find matching handler →
  if tail-resumptive: call resume-value closure inline → push result
  else: capture continuation → unwind → call clause → resume → restore
```
With ~100 JSON parse iterations × ~150 chars each = ~15,000 perform/resume cycles.

### Tail-resumptive optimization
Handler clauses of the form `((op args k) -> ... (resume k <expr>))` where `k` is only used in tail-position `(resume k expr)` calls can skip continuation capture entirely. The compiler detects these and generates a separate "tail-resume closure" that just computes `<expr>`.

## Off Limits
- `bench/*.mar` — DO NOT modify the benchmarks
- Type checker, parser, lexer — not relevant to runtime performance
- Correctness — all existing tests must pass

## Constraints
- `cargo test --workspace` must pass
- `./bench/run.sh` results must not regress significantly (fib, pattern, etc.)
- No cheating: optimizations must be general-purpose, not benchmark-specific

## What's Been Tried

### Starting point (commit 90b1854)
- effects: ~33ms, json: ~259ms, fib: ~20ms, pattern: ~30ms
- JSON dominates at 88% of total_ms (~292ms)

### WIP (commit f41a978)
- Generalized tail-resumptive analysis (allows if/let/begin/match before resume)
- Replaced VM-level ParseState builtins with pure Marlang `handle` block
- **BUG**: JSON benchmark hangs — likely tail-resumptive detection or handler dispatch issue
