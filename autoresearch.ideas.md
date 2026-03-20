# Autoresearch Ideas

## Completed
- Eliminate String allocations per Perform (use &str refs)
- Combined handler+entry search in Perform, avoid Vec alloc for tail-resume args
- Aborting handler fast path (skip continuation capture when k not used)
- Rewrite with-output to use mutable box + tail-resumptive handler
- Cache parsed handler descriptors (avoid JSON re-parsing per PushHandler)
- Wrap FunctionPrototype in Arc (closure creation = refcount bump)
- string-char-at O(1) ASCII fast path + make_some/make_none helpers

## Still promising
- Avoid handler clause closure string cloning in PushHandler (use constant pool refs)
- Pre-compute which handler entries match specific ops (during PushHandler, not Perform)
- Reduce closure creation cost in PushHandler: cache closures when upvalues haven't changed

## Dead ends
- Global PushGlobal cache: caused infinite loops in JSON (stale cache, complex invalidation)
- Mutex→UnsafeCell for MutableBox: negligible impact (<1ms)
- Arc<str> for HandlerEntry names: no measurable improvement
- Constant-return tail-resume ultra-fast path: JSON closures have upvalues, doesn't apply
- Pre-split effect op name at compile time: rsplit_once is ~5ns, negligible
- Effect dispatch HashMap: too many handler stack modification sites, stale indices crash
