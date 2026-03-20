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
| **Database driver** | Persistent state, dep system | Hard — FFI to libpq or pure-Marlang postgres wire protocol |
| **File watching** | Live reload | Medium — FFI to notify/kqueue |
| **JavaScript target** | Client-side components | Medium — `emit-js` macro compiles Marlang S-expressions to JS strings at expansion time (no Rust backend needed, proven working) |

**Resolved:**
- ~~Crypto (HMAC-SHA256)~~ — `stdlib/crypto.mar` via FFI to CommonCrypto (macOS)

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
- Later phases add a JS compilation target for rich client components

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

## Phase 3: Database Integration

**Requires: Database driver (FFI or wire protocol)**

### Approach A: FFI to libpq

Use Marlang's existing FFI system to call libpq.

### Approach B: Pure Marlang postgres wire protocol

Implement the Postgres wire protocol over TCP. More work, but no FFI dependency.

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

## Phase 4: Client Components (future)

**Requires: expanding the `emit-js` macro coverage**

JavaScript compilation is achieved via a macro (`emit-js`) that walks Marlang S-expressions at expansion time and produces JS strings — no Rust compiler backend needed. This has been prototyped and proven to work for: literals, lambda, let, if, begin, define, function calls, list/pair constructors, match/ADT destructuring, binary operators, and nested expressions.

Remaining JS coverage needed: `perform`/effects (compile to generator yields), tail-call trampolining, multi-statement bodies, and the client-side runtime (atom, event delegation, morphdom glue).

### `define-client`

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

Pure functions emitted to both server and JS:

```scheme
(define-shared format-price (cents)
  (string-append "$" (show (/ cents 100)) "." (show (rem cents 100))))
```

### `server-fn`

Inline server-side callbacks callable from client code:

```scheme
(server-fn (item-id)
  (perform db-execute "DELETE FROM items WHERE id = $1" (list item-id)))
```

Compiles to:
- Server: an RPC handler keyed by a signed token
- Client: a stub that sends the token + args over WebSocket

### Effect boundary enforcement

The type checker enforces boundaries via closed effect rows:

```scheme
; (define (counter-widget : (-> Int {Dom} Html)) ...)
; If the body tries to perform DB or IO, the type checker rejects it.
```

**Note:** Due to macro hygiene (see above), the macro must either receive the effect row from the user or the compiler's immune set must be extended to include type/effect names.

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
  (RpcCall String Bytes)
  (Disconnect))

(data DisconnectReason
  (ClientLeft)
  (Timeout)
  (Error String))

; --- Render tree analysis (Phase 4) ---

(data RenderNode
  (Static String)
  (Dynamic (List String) String)
  (ClientBoundary String (List String)))
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
- [ ] SourceActor — subscription fan-out with ref counting
- [ ] RouterActor — source registry, connection lifecycle
- [ ] Dep protocol — fetch, subscribe, combined
- [ ] Batching — defer flush to end of actor turn

### Phase 3 — Database (requires new runtime: DB driver)
- [ ] DB effect — query, execute
- [ ] Connection pooling
- [ ] DB-backed deps — `db-row`, `db-table`, `db-query`
- [ ] LISTEN/NOTIFY integration for live dep updates

### Phase 4 — Client Components (extends `emit-js` macro)
- [x] `emit-js` macro — compiles Marlang to JS strings at expansion time (literals, lambda, let, if, begin, define, match, ADTs, operators)
- [ ] `emit-js` — effects via generator yields
- [ ] ADT representation in JS
- [ ] Trampolining for tail calls
- [ ] `define-client` macro — closed effect row, JS emit
- [ ] `define-shared` — dual emit (server + JS)
- [ ] `server-fn` — RPC stubs + signed tokens
- [ ] Atom implementation in JS
- [ ] morphdom integration
- [ ] Event delegation
- [ ] Bundle manifest per route

---

## Open Questions

1. **Macro hygiene and types** — Should common type names (`Int`, `String`, `Bool`, `Unit`, `List`, etc.) and `__effect_row` be added to the hygiene immune set? This is a one-line compiler change that would let macros generate type annotations freely. Without it, macros must receive type expressions from the call site.

2. **Database driver strategy** — FFI to libpq is fast to build. A pure Marlang postgres wire protocol over TCP is more portable and educational. Start with FFI?

3. **State management** — Actors work but are verbose. Should there be a simpler `(define-state name initial-value)` macro that generates get/set message handlers?

4. **Template escaping** — The HTML DSL must escape user content to prevent XSS. Where is the boundary? All `text` calls auto-escape; `raw` for trusted HTML. ✅ Implemented.

5. **Session handling** — Cookie-based sessions are simplest. Need a signed cookie (HMAC) to prevent forgery. This requires the crypto capability from Phase 3+.

6. **CSS** — Out of scope for the framework. Serve static CSS files. Consider Tailwind-like utility classes as a convention.

7. **Hot reload** — File-watching the .mar source and restarting the server loop. Requires file watching (Phase 2+) or an external tool like `watchexec`.
