# Network Stack Code Review — `e4cffb3`

Review of the TCP/HTTP networking commit. The happy path works; error paths crash the scheduler.

---

## CRITICAL

### 1. `tcp-connect` and `http-fetch` failures crash the entire scheduler

**Files:** `src/vm/scheduler.ts:636-684`, `src/vm/scheduler.ts:886-908`

When `Bun.connect` fails (connection refused, DNS failure) or `fetch()` throws, the
promise rejects. The drain phase does `await Promise.race(promises)` with no `.catch()`
— the rejection propagates as an unhandled error, crashing the scheduler and killing
**every** fiber, not just the one that attempted the connection.

```typescript
// tcp-connect — rejection propagates unhandled:
const promise = new Promise<VMValue>((resolve, reject) => {
  Bun.connect({ ... }).catch(reject);
});

// http-fetch — same problem:
const promise = fetch(urlArg.value, fetchOpts).then(async (resp) => { ... });
// no .catch()
```

**Fix:** Catch rejections inside the promise and resolve to an error value (NIL or an
error ADT), or add `.catch()` wrapping in the drain phase that routes the error to
`handleFiberExit` for the specific pid.

---

### 2. `activeServers` counter can go negative on double-close

**Files:** `src/vm/scheduler.ts:830-834`, `src/vm/scheduler.ts:1037-1039`

Neither `tcp-close` (listener branch) nor `http-close-server` nulls out the server
handle after stopping. A second close passes the guard again, decrements
`activeServers` below zero. The loop condition `state.activeServers > 0` becomes false
even when other servers are still running, causing premature scheduler termination.

```typescript
if (ls && ls.server) {
  ls.server.stop();
  state.activeServers--;  // no guard against double-decrement
}
```

**Fix:** Set `ls.server = null` / `serverState.server = null` after `stop()`.

---

### 3. Single-waiter slots silently drop continuations on concurrent access

**Files:** `src/vm/scheduler.ts:779-780` (tcp-accept), `:983` (http-accept), `:815` (tcp-recv)

`waitingAccept` and `waitingRecv` are single slots, not arrays. If two fibers both
call accept/recv on the same resource, the second silently overwrites the first's
continuation. The first fiber is permanently stuck — its continuation is leaked, never
resumed. The channel implementation (Layer 1) correctly uses arrays, making this
inconsistency especially glaring.

```typescript
listenerState.waitingAccept = { pid: state.currentPid, cont: continuation };
// overwrites any existing waiter
```

**Fix:** Change to arrays with queue semantics (consistent with channels), or throw an
explicit error on concurrent access.

---

## MAJOR

### 4. `tcp-send` on a closed socket has no `closed` check

**File:** `src/vm/scheduler.ts:794-796`

`tcp-recv` checks `ss.closed` and returns NIL. `tcp-send` doesn't check at all.
Writing to a Bun socket after `close`/`error` callback has fired is undefined behavior.

**Fix:** Add `if (ss.closed) throw new Error('tcp-send: socket is closed');` before write.

---

### 5. `http-serve` fetch handler has no `.catch()` on `req.text()`

**File:** `src/vm/scheduler.ts:940-959`

If `req.text()` rejects (client disconnects mid-body), the `Promise<Response>` returned
to Bun is never resolved. The HTTP connection hangs indefinitely.

**Fix:** Add `.catch()` that resolves with a 400 or 500 response.

---

### 6. Socket/listener/server state Maps are never cleaned up after close

**File:** `src/vm/scheduler.ts` (tcp-close, http-close-server)

Closing a socket sets `ss.closed = true` but never calls
`state.tcpSockets.delete(val.id)`. Same for `tcpListeners` and `httpServers`. Over a
long-running server these Maps grow without bound.

**Fix:** Delete Map entries on close.

---

### 7. Unbounded queue growth with no backpressure

**Files:** `src/vm/scheduler.ts:656` (recvQueue), `:722` (acceptQueue), `:956` (requestQueue)

A fast peer or connection flood exhausts memory. No high-water mark, no
`socket.pause()`, no connection limit.

**Fix:** Implement high-water marks. Cap HTTP `requestQueue` and respond 503 on
overflow. Limit `acceptQueue` and close excess connections.

---

### 8. ~75 lines of socket callbacks duplicated between `tcp-connect` and `tcp-listen`

**File:** `src/vm/scheduler.ts:646-680` vs `:725-759`

The `data`, `close`, and `error` callbacks are character-for-character identical.

**Fix:** Extract a `makeSocketHandlers(state)` factory function.

---

### 9. Stdlib type signatures are incorrect

**File:** `stdlib/net.mar:22-23`

```lisp
(tcp-recv    (-> TcpSocket String))     ; actually returns nil on close
(tcp-close   (-> TcpSocket Unit)))      ; actually accepts TcpListener too
```

**Fix:** `tcp-recv` return type should be `(Option String)`. Either add a separate
`close-listener` or make `tcp-close` polymorphic.

---

### 10. `http-close-server` abandons queued requests — clients hang forever

**File:** `src/vm/scheduler.ts:1031-1047`

Requests already in `requestQueue` are orphaned when the server closes. Their HTTP
response promises never resolve until scheduler exit (line 1294).

**Fix:** On server close, drain `requestQueue` and resolve each with 503.

---

## MINOR

### 11. `removeHandlerFrame` at end of `runFibers` may pop the wrong frame

**File:** `src/vm/scheduler.ts:1302`

After the main loop exits, `removeHandlerFrame(state)` pops from `vm.handlerStack`.
But the handler frame for the last fiber was already removed. This may pop an
unrelated handler frame.

**Fix:** Track whether a handler frame is currently installed, or guard with a check.

---

### 12. Spec uses `Pair` type but implementation uses `List`

**File:** `specs/net.md:152-154` vs `stdlib/http.mar:28-29`

Spec says `List (Pair String String)` for headers; implementation uses
`List (List String)`. Out of date at commit time.

**Fix:** Update `specs/net.md` to match implementation.

---

### 13. `http-fetch` silently ignores malformed options

**File:** `src/vm/scheduler.ts:861-880`

Passing `(Method 42)` silently defaults to GET. `(Header "key" 123)` silently drops
the header.

**Fix:** Throw descriptive errors for type mismatches inside option parsing.

---

### 14. `toRuntimeValue` for network types silently erases to `nil`

**File:** `src/vm/vm.ts:1099-1102`

Creates a fresh `{ kind: 'nil' }` object (not the shared `NIL` sentinel) and silently
erases the handle. Should either throw or use the shared `NIL` constant.

---

### 15. Tests use random ports and hardcoded 100ms sleeps

**Files:** `spec/net.test.ts:37-39`, `spec/http.test.ts:37-39,132,164,193`

Random ports from 10000-59999 can collide. The 100ms sleep for server startup is a
race condition that will fail on slow CI.

**Fix:** Use port 0 and read back assigned port. Replace sleeps with retry loops.

---

## NIT

### 16. `any` types for all Bun socket/server handles

**File:** `src/vm/scheduler.ts:31,38,44`

All socket and server handles are typed as `any`. Bun exports proper types.
`__marlangId` monkey-patching uses `(socket as any)` everywhere.

**Fix:** Use Bun's types and a `WeakMap<Socket, number>` for ID association.

---

### 17. Commit bundles unrelated changes

Networking implementation, 2303-line Rust port design doc, and benchmark parameter
changes in one commit with the vague message "net stack and rust port spec".

---

### 18. Test helpers duplicated across three test files

`runAsync`, `captureOutput`, and `randomPort` are copy-pasted across `net.test.ts`,
`http.test.ts`, and `concurrent.test.ts`.

**Fix:** Extract to a shared `spec/helpers.ts`.

---

## Summary

| Severity | Count | Key Theme |
|----------|-------|-----------|
| CRITICAL | 3 | Error paths crash scheduler; double-close corrupts state; concurrent access drops continuations |
| MAJOR | 7 | Missing closed checks, resource leaks, unbounded queues, type signature lies |
| MINOR | 5 | Handler frame lifecycle, spec drift, silent failures |
| NIT | 3 | Type safety, commit hygiene, test duplication |
