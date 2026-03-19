# Autoresearch Ideas

## Completed
- Enable JIT for self-recursive Op::Call (49x improvement)
- Skip dead PushGlobal for self-recursive callees
- Fuse comparison+JumpIfFalse in JIT peephole
- Cache local SSA values within blocks
- Specialized fast inner function for integer-pure recursive functions (2x more)

## Future Optimizations (not pursued yet)

### JIT Improvements
- Extend integer-pure detection to include DivInt (with trap handling)
- Support non-self-recursive Call in JIT for functions with only safe ops
- Inline small functions at call sites (especially base cases)
- Float-pure specialization (similar to integer-pure but for float arithmetic)
- Type specialization: track types through the JIT and eliminate tag checks for known types

### Interpreter Improvements
- NaN-boxing: reduce Value size from 16 to 8 bytes (halves cache pressure)
- Stack-based locals: eliminate per-call Vec allocation
- Direct threading / computed goto for opcode dispatch
- Indexed globals table instead of HashMap lookup in PushGlobal
- Inline cache for PushGlobal (cache last lookup result)

### Compiler Improvements
- Emit RETURN directly after base case instead of JUMP to RETURN
- Peephole optimizer in bytecode compiler (dead PushConst nil + Pop elimination)
- Combine PushGlobal + Call into a single CallGlobal opcode
