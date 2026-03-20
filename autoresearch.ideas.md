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
- Eliminate String allocations per Perform (use &str refs)
- Combined handler+entry search in Perform, avoid Vec alloc for tail-resume args
- Aborting handler fast path (skip continuation capture when k not used)
- Rewrite with-output to use mutable box + tail-resumptive handler
- Cache parsed handler descriptors (avoid JSON re-parsing per PushHandler)

## Effect Optimization Ideas (current focus)

### Still promising
- Store effect_name/op_name as Arc<str> or constant pool indices in HandlerEntry (avoid String clone per PushHandler)
- Cache handler entries (closures) across repeated PushHandler calls for the same handler
- Pre-compute handler lookup table (HashMap<op_name, handler_idx>) maintained on push/pop
- Inline tail-resume closures directly (avoid frame push/pop for simple expressions)
- Pre-split effect op name at compile time (avoid rsplit_once per Perform)

### Bigger refactors
- Copy-on-write locals (Arc<Vec<Value>>) to make continuation capture cheaper
- Stack-based locals (contiguous array) for faster continuation capture (single memcpy)
- NaN-boxing: reduce Value size from 16 to 8 bytes

### Dead ends
- Global PushGlobal cache: caused infinite loops in JSON (stale cache, complex invalidation)
- Mutex→UnsafeCell for MutableBox: negligible impact (<1ms)
