# Autoresearch Ideas

## Completed (effect optimization session)
- Tail-resumptive handler detection: strict (resume k expr) form + mutable box ParseState
- Eliminate String allocations per Perform (use &str refs)
- Combined handler+entry search in Perform, avoid Vec alloc for tail-resume args
- Aborting handler fast path (skip continuation capture when k not used)
- Rewrite with-output to use mutable box + tail-resumptive handler (-10.5% json)
- Cache parsed handler descriptors (avoid JSON re-parsing per PushHandler) (-11.7% json)
- Wrap FunctionPrototype in Arc (closure creation = refcount bump) (-5.5% json)
- string-char-at O(1) ASCII fast path + make_some/make_none helpers
- Avoid Vec heap alloc for builtin args (≤4 args)
- Optimize json-string-char: peek first to avoid try-parse for non-escaped chars (-7.5% json)

## Still potentially promising (bigger refactors)
- Handle body upvalue capture from grandparent scopes (compiler bug blocks inlining try-parse into many-parse loop)
- NaN-boxing: reduce Value size from 16 to 8 bytes — halves cache pressure for ALL interpreter code
- Copy-on-write locals (for cheaper continuation capture in non-tail-resumptive handlers)
- Stack-based locals (contiguous array instead of per-frame Vec — single memcpy for captures)
- Direct threading / computed goto for opcode dispatch (requires unsafe Rust or codegen)
- Change ADT tag from String to interned u32 ID — eliminates string comparison in MatchTag
- Indexed globals table instead of HashMap lookup in PushGlobal

## Dead ends (don't retry)
- Global PushGlobal cache: caused infinite loops (stale cache, complex invalidation)
- Mutex→UnsafeCell for MutableBox: <1ms impact
- Arc<str> for HandlerEntry names: no measurable improvement  
- Constant-return tail-resume ultra-fast path: JSON closures have upvalues
- Pre-split effect op name at compile time: rsplit_once is ~5ns
- Effect dispatch HashMap: too many handler stack modification sites, stale indices
- Perform inline cache (single entry): thrashes with alternating ops
- Wrap Closure in Arc (CallFrame + VMObject): Arc allocation overhead > clone savings
- Share HandlerEntries via Arc: entries are small, Arc overhead offsets savings
- Cached ADT tag strings (LazyLock): Rust SSO makes short string clones cheap
- Stack-array for Construct fields: allocator already efficient for small sizes
