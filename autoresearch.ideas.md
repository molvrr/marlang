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

## Future Optimizations (not pursued yet)

### JIT Improvements
- Extend integer-pure detection to include DivInt (with trap handling)
- Float-pure specialization (similar to integer-pure but for float arithmetic)
- Type specialization: track types through the JIT and eliminate tag checks for known types
- Support non-self-recursive Call in JIT with proper side-effect tracking
- Inline small functions at call sites (especially base cases)

### Interpreter Improvements
- NaN-boxing: reduce Value size from 16 to 8 bytes (halves cache pressure for non-JIT code)
- Stack-based locals: eliminate per-call Vec allocation
- Direct threading / computed goto for opcode dispatch
- Indexed globals table instead of HashMap lookup in PushGlobal

### Compiler Improvements
- Emit RETURN directly after base case instead of JUMP to RETURN (bytecode level)
- Peephole optimizer in bytecode compiler (dead PushConst nil + Pop elimination)
- Combine PushGlobal + Call into a single CallGlobal opcode
