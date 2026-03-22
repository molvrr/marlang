# Fullstack Framework — Planning Document

## Context

This document describes a fullstack web framework built on top of Marlang, a statically typed Scheme-like Lisp with:

- **Row-polymorphic algebraic effects** — the type checker enforces which effects a function may perform
- **Algebraic data types (ADTs)** — positional constructors, exhaustive pattern matching
- **Actors and fibers** — lightweight concurrency with message passing
- **HTTP server** — effect-based, backed by hyper, accepts/responds to requests in a loop
- **TCP networking** — low-level sockets, also effect-based
- **WebSocket** — server-side WebSocket via upgrade from HTTP, text and binary messages
- **Macro system** — `defmacro` with hygiene, quasiquote/unquote

### What Marlang does NOT have yet

These are runtime capabilities the framework needs that must be built first:

| Gap | Needed for | Difficulty |
|-----|-----------|------------|
| **File watching** | Live reload | Medium — FFI to notify/kqueue |
| **JavaScript target** | Client-side components with effect proxying | Medium — `emit-js` macro compiles Marlang S-expressions to JS strings at expansion time (no Rust backend needed, proven working). `perform` compiles to generator yields; unhandled effects proxy to server over WebSocket. |

**Resolved:**
- ~~Database driver~~ — `stdlib/pg.mar`, pure Marlang PostgreSQL wire protocol over TCP
- ~~Crypto~~ — `md5`, `sha256`, `hmac-sha256` builtins (ring + md-5 crates), `stdlib/crypto.mar` for hex encoding
- ~~Binary TCP~~ — `tcp-send-bytes` / `tcp-recv-bytes` for binary protocol support

The framework is designed in phases. Phase 1 works with what exists today. Later phases add capabilities as runtime gaps are filled.

### Macro hygiene and type annotations

Marlang's macro system applies hygiene marks to symbols introduced by macros. Type names (`Int`, `String`, effect names) are **not** in the immune set, so macros **cannot synthesize type annotations from scratch** — the type checker sees `Int~1` instead of `Int`.

**Workaround:** Macros that need to emit type signatures must receive type expressions from the call site (passed through, not constructed). Alternatively, a small compiler patch could add common type names to the hygiene immune set.

---

## Core Idea

- Routes declare their data dependencies via a `depends-on` / `combined` form
- The server renders HTML and sends it to the client
- For interactivity, the framework uses **WebSocket + morphdom** — the server pushes HTML fragments over WebSocket, and the client uses morphdom to efficiently patch the DOM
- When a dependency changes, the server re-renders the affected fragment and pushes it
- Later phases add a JS compilation target where **unhandled effects on the client automatically proxy to the server** — the effect system is the client/server boundary

---

## Phase 1: Server-Side Rendered Framework ✅

Everything in this phase uses capabilities Marlang already has.

### Response ADT

Handlers return a `Response` value that the framework dispatches:

```scheme
(data Response
  (HtmlResponse String)
  (RedirectResponse String)
  (TextResponse Int (List (Pair String String)) String))

; Convenience constructors
(define ok            (lambda (content) (HtmlResponse content)))
(define redirect      (lambda (url) (RedirectResponse url)))
(define text-response (lambda (status headers body) (TextResponse status headers body)))
```

### HTML DSL

Pure functions that produce HTML strings from S-expressions:

```scheme
(import "std:web" as w)

(w/ok
  (w/html (list (MkPair "lang" "en"))
    (string-append
      (w/head (w/title "My App"))
      (w/body (list)
        (string-append
          (w/h1 (list) "Hello")
          (w/p (list) (string-append "Count: " (show count))))))))
```

Implementation: element functions that concatenate strings. Each takes attributes (as a list of pairs) and children (as strings). `text` escapes HTML, `raw` passes through.

### Router

Path matching and parameter extraction:

```scheme
(define routes (list
  (w/GET  "/"               handle-home)
  (w/GET  "/users/:id"      handle-user)
  (w/POST "/users"          handle-create-user)
  (w/DELETE "/users/:id"    handle-delete-user)))
```

Dynamic segments use `:param` syntax. `segments-match` walks both pattern and path, extracting parameters into a `(List (Pair String String))`.

### Request Handling Effect

A `Ctx` effect threads request context implicitly:

```scheme
(effect Ctx
  (ctx-param      (-> String (Option String)))
  (ctx-query      (-> String (Option String)))
  (ctx-form       (-> String (Option String)))
  (ctx-header     (-> String (Option String)))
  (ctx-method     (-> Unit String))
  (ctx-path       (-> Unit String)))
```

Route handlers perform `Ctx` effects. The server loop installs a handler that resolves them from the parsed `HttpRequest`.

### Server Loop

Ties it all together. Dispatch wraps handler execution in a `handle` block that catches `throw` for 500 error pages:

```scheme
(define dispatch (lambda (method full-path headers body conn routes)
  ...
  (match (match-routes method clean-path routes)
    ((Some (MkPair handler params))
      (handle
        (let ((response (run-handler method clean-path params query-params form-params hdrs handler)))
          (send-response conn response))
        ((throw msg k) ->
          (begin
            (io/println (string-append "500 error: " msg))
            (let ((ignored (http/respond-text conn 500 ... (error-page-500 msg))))
              nil)))))
    (None
      (http/respond-text! conn 404 ... (not-found-page full-path))))))
```

### Static File Serving

`serve-file` reads files as bytes and responds with the correct MIME type:

```scheme
(w/serve-file conn "./public" "/style.css")  ; => responds with text/css
```

Backed by `io/read-file-bytes` which reads files as raw `Bytes`.

### What this gives us

A working web framework with:
- Declarative HTML rendering with XSS escaping
- Type-safe Response ADT (ok, redirect, text-response)
- Effect-based request context
- Pattern-matched routing with path parameters
- POST form handling
- Automatic 500 error pages with stack traces
- Automatic 404 pages
- Static file serving (text and binary)
- Server-side state via actor-backed cells

No external dependencies. No JavaScript. Works today.

---

## Phase 2: WebSocket + Morphdom Live Updates

WebSocket support exists in the runtime. This phase builds the framework actors and client protocol on top of it.

### How it works

The server maintains a WebSocket connection per client tab. When server state changes, the server re-renders the affected HTML fragment and sends it over the WebSocket. The client uses morphdom (a ~4KB DOM diffing library) to efficiently patch the page.

### Client protocol

The server sends HTML fragments over WebSocket:

```html
<div id="counter-val">42</div>
```

The client JS receives it and uses morphdom to patch the target element:

```javascript
ws.onmessage = (e) => {
  const tmp = document.createElement('div');
  tmp.innerHTML = e.data;
  const el = tmp.firstElementChild;
  if (el && el.id) {
    const target = document.getElementById(el.id);
    if (target) morphdom(target, el);
  }
};
```

### Form/action handling

User actions (button clicks, form submissions) are sent over WebSocket as JSON messages or via fetch() to server endpoints. The server processes them, updates state, and pushes patches to all connected clients.

### ConnectionActor

One actor per connected WebSocket client. Owns the WebSocket, holds the current subscription set, batches updates:

```scheme
(data ConnectionMsg
  (DepUpdate String String)   ; dep-key, new-html-fragment
  (Navigate String)           ; new path
  (Disconnect))
```

### SourceActor

One per unique dep key across all connections:
- Owns the underlying subscription (DB LISTEN, file watcher, timer)
- Fans out value changes to subscriber callbacks
- Reference counted — shuts down when last subscriber disconnects

### RouterActor

One global instance:
- Owns the source registry: a map from dep key to SourceActor pid
- On new connection: matches route, builds Ctx, resolves deps, spawns ConnectionActor

### Dep protocol

A dep is a value that can be fetched and subscribed to:

```scheme
(data DepState (Loading) (Ready a) (Failed String))

(effect Dep
  (dep-fetch     (-> DepKey a))
  (dep-subscribe (-> DepKey (-> a Unit) Unit)))
```

The `combined` form composes deps sequentially:

```scheme
(combined
  (user    (db-row "users" (list (MkPair "id" user-id))))
  (org     (db-row "orgs"  (list (MkPair "id" (user-org user))))))
```

---

## Phase 3: Database Integration ✅

Implemented as a pure Marlang PostgreSQL client (`stdlib/pg.mar`) using the wire protocol over TCP. No Rust dependencies — just TCP + crypto builtins.

### DB effect

Either way, the framework exposes a clean effect interface:

```scheme
(effect DB
  (db-query   (-> String (List String) (List (List String))))
  (db-execute (-> String (List String) Int)))
```

Route handlers that perform `DB` effects are distinguishable from pure handlers in the type system:

```scheme
; This handler performs DB — the type checker tracks it
(define handle-user (lambda ()
  (let ((id (perform ctx-param "id"))
        (rows (perform db-query "SELECT name FROM users WHERE id = $1" (list id))))
    (match rows
      (Nil (w/ok (w/p (list) "User not found")))
      ((Cons row _) (w/ok (w/p (list) (string-append "User: " (car row)))))))))
```

---

## Phase 4: Client Components — Cross-Platform Effects

**Requires: expanding the `emit-js` macro coverage**

JavaScript compilation is achieved via a macro (`emit-js`) that walks Marlang S-expressions at expansion time and produces JS strings — no Rust compiler backend needed. This has been prototyped and proven to work for: literals, lambda, let, if, begin, define, function calls, list/pair constructors, match/ADT destructuring, binary operators, and nested expressions.

### Core idea: effects as the client/server boundary

Instead of requiring explicit RPC stubs (`server-fn`) or closed effect rows, the framework uses **effects themselves** as the natural client/server boundary. The rule is simple:

> If a `perform` has a local (client-side) handler, it runs locally. If it doesn't, the runtime serializes the effect and sends it to the server over WebSocket, suspends, and resumes when the server responds.

This means the same Marlang code can run on either side. Effects are already "I need something from my environment" — on the client, the environment for `DB`, `IO`, etc. is the server.

### How it works

**Client (JS):** `emit-js` compiles `perform` to generator `yield` points. The client runtime wraps each component in a driver that steps the generator. When a `yield` produces an effect the client can't handle, the runtime sends it over WebSocket and parks the generator until the response arrives.

```javascript
// Generated JS (conceptual)
function* counterWidget(userId) {
  // Local effect — handled by client runtime
  const count = yield { effect: "atom", args: [0] };

  // Remote effect — no local handler, auto-proxied to server
  const user = yield { effect: "db-query",
    args: ["SELECT name FROM users WHERE id = $1", [userId]] };

  return div({},
    span({}, user[0]),
    button({ onclick: () => runtime.perform("atom-swap", count, n => n + 1) }, "+1"));
}
```

**Server:** Receives effect requests over the existing WebSocket connection, runs them in a real handler (with the connection's security context), serializes the result, sends it back.

```scheme
; Server-side effect proxy handler (inside ConnectionActor)
; Receives: { "effect": "db-query", "args": [...], "id": 42 }
; Runs the effect in the server's handler stack
; Responds: { "id": 42, "result": [...] }
```

**Client-handled effects** run entirely in the browser with no round-trip:
- **Dom** — DOM manipulation, morphdom patching
- **Atom** — local reactive state (read/write/swap)
- **Timer** — setTimeout/setInterval wrappers
- **Event** — event listeners, delegation

**Server-proxied effects** are anything the client doesn't have a handler for:
- **DB** — queries, mutations
- **IO** — file reads, writes, print
- **Exception** — thrown errors propagate back to the client
- Any custom effects defined by the application

### Security: effect allowlists

The server can't blindly execute any effect the client requests. Each route/component declares which effects may be proxied:

```scheme
(define-client user-profile (user-id)
  #:allow (DB Exception)    ; only these effects can be proxied
  (let ((user (perform db-query "SELECT * FROM users WHERE id = $1"
                (list (show user-id)))))
    ...))
```

The server validates incoming effect requests against the allowlist. Unauthorized effects are rejected with an error. The allowlist is signed (HMAC) and embedded in the page, so clients can't forge it.

### Latency mitigation

Each proxied effect is a WebSocket round-trip. Strategies to keep this fast:

1. **Prefetching** — the `depends-on`/`combined` system already declares data deps upfront. The server can inline initial data in the page, so the first render is instant.
2. **Batching** — collect multiple `perform`s that don't depend on each other and send them in a single WebSocket message. The server runs them concurrently.
3. **Optimistic updates** — for mutations, update the local atom immediately and reconcile when the server responds.
4. **Selective client components** — most of the app is still server-rendered (Phase 2). Only interactive islands need client components, and most interactions are a single action → one server effect → re-render.

### `define-client`

Defines a component that compiles to both server-side (for initial SSR) and client-side JS (for interactivity). Effect rows are open — unhandled effects proxy to the server automatically.

```scheme
(define-client counter-widget (initial-count)
  (let ((count (atom initial-count)))
    (w/div (list)
      (string-append
        (w/span (list) (show (deref count)))
        (w/button
          (list (MkPair "onclick" (client-fn () (swap! count (lambda (n) (+ n 1))))))
          "+1")))))
```

### `define-shared`

Pure functions emitted to both server and JS. These run identically on both sides with no proxying:

```scheme
(define-shared format-price (cents)
  (string-append "$" (show (/ cents 100)) "." (show (rem cents 100))))
```

### ADT serialization

ADTs need a wire format for effect proxying (args and return values cross the WebSocket boundary). Convention:

```javascript
// Marlang: (Some 42)     → JS: { tag: "Some", fields: [42] }
// Marlang: None           → JS: { tag: "None", fields: [] }
// Marlang: (Cons 1 Nil)   → JS: { tag: "Cons", fields: [1, { tag: "Nil", fields: [] }] }
```

The `emit-js` macro already handles ADT construction and destructuring. The serialization format extends this to JSON for the wire protocol. The existing `stdlib/json.mar` codec handles encoding/decoding.

### Effect proxy protocol

Messages over WebSocket between client runtime and server:

```javascript
// Client → Server (effect request)
{ "type": "effect", "id": 1, "effect": "db-query",
  "args": ["SELECT * FROM users WHERE id = $1", ["42"]] }

// Server → Client (effect response)
{ "type": "result", "id": 1, "value": [["42", "alice", "alice@example.com"]] }

// Server → Client (effect error)
{ "type": "error", "id": 1, "message": "relation \"users\" does not exist" }
```

The `id` field correlates requests with responses, allowing multiple in-flight effects (from batching or concurrent components).

---

## The Actor Topology (Phase 2+)

```
AppActor
  └── RouterActor
        ├── SourceActor (per unique dep key)
        ├── SourceActor (...)
        ├── ConnectionActor (per connected client)
        └── ConnectionActor (...)
```

### AppActor

- Top of supervision tree
- Owns initialized resources (DB pool, config)
- Supervises RouterActor

---

## Key Data Types

```scheme
; --- Responses ---

(data Response
  (HtmlResponse String)
  (RedirectResponse String)
  (TextResponse Int (List (Pair String String)) String))

; --- Deps (Phase 2+) ---

(data DepState (Loading) (Ready a) (Failed String))

; --- Connection (Phase 2+) ---

(data ConnectionMsg
  (DepUpdate String String)
  (Navigate String)
  (EffectProxy EffectRequest)   ; Phase 4: proxied effect from client
  (Disconnect))

(data DisconnectReason
  (ClientLeft)
  (Timeout)
  (Error String))

; --- Effect proxy protocol (Phase 4) ---

(data EffectRequest
  (EffectRequest Int String (List String)))   ; id, effect-name, args (JSON-encoded)

(data EffectResponse
  (EffectResult Int String)                   ; id, value (JSON-encoded)
  (EffectError Int String))                   ; id, error message

; --- Client effects (Phase 4) ---

(data ClientEffect
  (AtomNew a)
  (AtomRead a)
  (AtomSwap a (-> a a))
  (DomPatch String String))                   ; target-id, html-fragment
```

---

## Checklist

### Phase 1 — Server-Side Framework ✅
- [x] HTML DSL — element functions, attribute rendering, escaping
- [x] Router — path matching, `:param` extraction, method dispatch (GET/POST/PUT/DELETE/PATCH)
- [x] Response ADT — `ok`, `redirect`, `text-response`
- [x] `Ctx` effect — param, query, form, header, method, path operations
- [x] Server loop — accept, route, handle, respond
- [x] Error pages — 404 (HTML), 500 (HTML with error message)
- [x] Static file serving — `serve-file` with MIME type detection, binary support via `read-file-bytes`
- [x] State cells — actor-backed `cell-new`, `cell-get`, `cell-set`, `cell-update`
- [x] Example app — `examples/notes-app.mar` (CRUD with forms + redirects)

### Phase 2 — WebSocket + Morphdom Live Updates
- [x] WebSocket runtime — accept, send, recv, close (already built)
- [x] Working demo — `examples/web-mvp.mar` (counter + message board with push)
- [x] morphdom integration — client JS uses morphdom for DOM patching
- [x] LiveHandle ADT — typed handle wrapping value cell + subscribers + render function
- [x] `live` / `live-get` / `live-update` / `live-set` — declarative reactive state API
- [x] Auto-subscribe — `run-live!` auto-subscribes new WebSocket clients to all live sources
- [x] `live-scripts` — includes morphdom CDN + WebSocket client JS
- [x] Effect-polymorphic lambdas — open effect rows so handlers with different effects coexist in route lists
- [x] ConnectionActor — per-client actor (`conn-loop`) with WS reader fiber, processes both live updates and client messages
- [x] Navigation over WebSocket — link clicks intercepted by event delegation, rendered server-side, morphdom patches entire body, pushState for history
- [x] POST over WebSocket — forms/actions sent over WS, redirect responses trigger navigation
- [x] `ConnMsg` ADT — `ConnUpdate`, `ConnWsText`, `ConnWsClosed` message protocol
- [x] SourceActor — one per live source, holds value + render + subscribers, atomic update + notify
- [x] RouterActor — global singleton, manages source registry + connection lifecycle (subscribe/unsubscribe all sources on connect/disconnect)
- [x] Batching — natural message queueing: ConnUpdate messages queue in actor mailbox during handler execution, processed sequentially after
- [x] Dep protocol — `live-map` (derive from one upstream), `live-map2` (derive from two), watcher fibers with `SourceWatch`/`SourceUnwatch`

### Phase 3 — Database ✅ (complete)
- [x] PostgreSQL wire protocol client — pure Marlang over TCP, `stdlib/pg.mar`
- [x] Parameterized queries — `$1`, `$2`, etc. via extended query protocol
- [x] Buffered binary TCP reads — `tcp-send-bytes` / `tcp-recv-bytes` runtime support
- [x] Authentication — trust, cleartext password, MD5
- [x] Typed param serializers — `pg/text`, `pg/int`, `pg/float`, `pg/bool`
- [x] Typed row deserializers — `col`/`col!`, `col-int`/`as-int`, `col-float`/`as-float`, `col-bool`/`as-bool`
- [x] Convenience helpers — `each`, `map-rows`, `scalar`, `scalar!`
- [x] Crypto builtins — `md5`, `sha256`, `hmac-sha256` (ring + md-5 crates, portable)
- [x] VM global cache fix — replaced dangling pointer keys with string keys
- [x] Connection pooling — actor-based pool with checkout/checkin, `pool-new`, `pool-query`, `pool-execute`
- [x] DB-backed live deps — `db-live` creates live sources driven by LISTEN/NOTIFY + auto-requery
- [x] LISTEN/NOTIFY integration — `listener-new`, `listener-subscribe`/`unsubscribe`, `listen`/`unlisten`/`notify`/`wait-notification`
- [x] SCRAM-SHA-256 authentication — full RFC 5802 implementation with PBKDF2, new builtins: `pbkdf2-sha256`, `bytes-xor`, `base64-encode`, `base64-decode`, `random-bytes`

### Phase 4 — Client Components (cross-platform effects)
- [x] `emit-js` macro — compiles Marlang to JS strings at expansion time (literals, lambda, let, if, begin, define, match, ADTs, operators)
- [x] `emit-js` — `perform` compiled to generator `yield` points
- [x] ADT wire format — `{ tag, fields }` JSON serialization for effect args/results
- [x] Client effect runtime — generator driver that steps yields, routes to local or remote handlers
- [x] Local effect handlers — Atom (reactive state), Dom (morphdom), Timer, Event in `stdlib/js.mar` client runtime
- [x] Trampolining for tail calls in JS — `recur` form + `marlang.trampoline/bounce` in runtime
- [x] Effect proxy — unhandled effects auto-proxied to server over WebSocket in client runtime
- [x] Server-side effect dispatcher — `web.mar` parses JSON effect requests, dispatches to handler map, sends results back
- [x] `define-client` macro — server renders mount-point div + mount script; JS is a generator component
- [x] `define-shared` — dual emit for pure functions (server + JS), JS code collected via `shared-js`
- [x] Event delegation — `event_delegate` handler in client runtime; `dom_morph` for morphdom patches
- [x] Effect allowlists — `define-client-guarded` macro with HMAC-signed allowlist, server-validated via `EffectMsg` ADT dispatch
- [x] Batching — `{"type":"batch"}` WebSocket protocol, concurrent fiber dispatch on server, `batchEffects()` in client runtime
- [x] Prefetching — `prefetch` / `prefetch-effect` inline data as `window.__prefetch`, client runtime checks cache before proxying

---

## Open Questions

1. **Macro hygiene and types** — Should common type names (`Int`, `String`, `Bool`, `Unit`, `List`, etc.) and `__effect_row` be added to the hygiene immune set? This is a one-line compiler change that would let macros generate type annotations freely. Without it, macros must receive type expressions from the call site.

2. ~~Database driver strategy~~ — Resolved: pure Marlang wire protocol over TCP (`stdlib/pg.mar`).

3. **State management** — Actors work but are verbose. Should there be a simpler `(define-state name initial-value)` macro that generates get/set message handlers?

4. **Template escaping** — The HTML DSL must escape user content to prevent XSS. Where is the boundary? All `text` calls auto-escape; `raw` for trusted HTML. ✅ Implemented.

5. **Session handling** — Cookie-based sessions are simplest. Need a signed cookie (HMAC) to prevent forgery. Crypto builtins (`hmac-sha256`) are now available.

6. **CSS** — Out of scope for the framework. Serve static CSS files. Consider Tailwind-like utility classes as a convention.

7. **Hot reload** — File-watching the .mar source and restarting the server loop. Requires file watching (Phase 2+) or an external tool like `watchexec`.
