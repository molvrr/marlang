# Autoresearch Ideas

## Completed
- Enable JIT for self-recursive Op::Call (49x improvement)
- Skip dead PushGlobal for self-recursive callees
- Fuse comparison+JumpIfFalse in JIT peephole
- Cache local SSA values within blocks
- Specialized fast inner function for integer-pure recursive functions (2x more)
- JUMP-to-RETURN peephole enables Cranelift shrink-wrapping
- Extended integer-pure to Eq/Not + fixed TailCall SSA (loop header block params)
- Cranelift flags: disabled frame pointers + stack probes

## Effect Optimization Ideas (current focus)

### Tail-resumptive handler improvements
- Generalized tail-resumptive detection: allow if/let/begin/match before (resume k expr), not just bare (resume k expr)
- Pre-compute handler entry index at compile time (avoid linear scan in handler_entries on each Perform)
- Cache effect name → handler index mapping to avoid string comparison per Perform
- Intern effect op names as integers instead of strings for O(1) matching

### Handler stack walk optimization
- Effect name interning: store effect/op as integer IDs instead of Strings in HandlerEntry — O(1) match vs string comparison
- Flat handler table: instead of walking handler_stack linearly, maintain a flat HashMap<(effect_id, op_id), handler_idx> updated on push/pop
- Handler stack locality: keep handler frames inline in a flat Vec rather than separate heap allocations

### Continuation capture avoidance
- More aggressive tail-resumptive detection covering complex handler bodies
- "Prompt-less" handlers: for handlers that always resume exactly once, skip building VMContinuation entirely
- Handler inlining: for simple handlers, inline the handler logic at the perform site during compilation

### Interpreter Improvements
- NaN-boxing: reduce Value size from 16 to 8 bytes (halves cache pressure for non-JIT code)
- Stack-based locals: eliminate per-call Vec allocation
- Direct threading / computed goto for opcode dispatch
- Indexed globals table instead of HashMap lookup in PushGlobal

### JIT Improvements
- Extend integer-pure detection to include DivInt (with trap handling)
- Float-pure specialization (similar to integer-pure but for float arithmetic)
- Type specialization: track types through the JIT and eliminate tag checks for known types
- Support non-self-recursive Call in JIT with proper side-effect tracking
- Inline small functions at call sites (especially base cases)

### Compiler Improvements
- Peephole optimizer in bytecode compiler (dead PushConst nil + Pop elimination)
- Combine PushGlobal + Call into a single CallGlobal opcode
