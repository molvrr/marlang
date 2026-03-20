# Fullstack Framework — Planning Document

## Context

This document describes a fullstack web framework built on top of Marlang, a statically typed Scheme-like Lisp with:

- **Row-polymorphic algebraic effects** — the type checker enforces which effects a function may perform
- **Algebraic data types (ADTs)** — positional constructors, exhaustive pattern matching
- **Actors and fibers** — lightweight concurrency with message passing
- **HTTP server** — effect-based, backed by hyper, accepts/responds to requests in a loop
- **TCP networking** — low-level sockets, also effect-based
- **Macro system** — `defmacro` with hygiene, quasiquote/unquote

### What Marlang does NOT have yet

These are runtime capabilities the framework needs that must be built first:

| Gap | Needed for | Difficulty |
|-----|-----------|------------|
| **WebSocket** | Live updates, server push | Medium — extend TCP/HTTP layer, hyper supports upgrades |
| **JSON codec** | API responses, client-server data exchange | Easy — pure Marlang on top of the codec library |
| **Path routing / URL parsing** | Request dispatch | Easy — pure Marlang string matching |
| **Database driver** | Persistent state, dep system | Hard — FFI to libpq or pure-Marlang postgres wire protocol |
| **File watching** | `fs/file` dep | Medium — FFI to notify/kqueue |
| **Crypto (HMAC-SHA256)** | `server-fn` token signing | Easy — FFI or ring via Rust |
| **JavaScript target** | Client-side components | Medium — `emit-js` macro compiles Marlang S-expressions to JS strings at expansion time (no Rust backend needed, proven working) |

The framework is designed in phases. Phase 1 works with what exists today. Later phases add capabilities as runtime gaps are filled.

### Macro hygiene and type annotations

Marlang's macro system applies hygiene marks to symbols introduced by macros. Type names (`Int`, `String`, effect names) are **not** in the immune set, so macros **cannot synthesize type annotations from scratch** — the type checker sees `Int~1` instead of `Int`.

**Workaround:** Macros that need to emit type signatures must receive type expressions from the call site (passed through, not constructed). Alternatively, a small compiler patch could add common type names to the hygiene immune set.

---

## Core Idea

- Routes declare their data dependencies via a `depends-on` / `combined` form
- The server renders HTML and sends it to the client
- For interactivity without JavaScript compilation, the framework uses **HTMX** — a small JS library that lets HTML attributes drive server requests
- When a dependency changes, the server re-renders the affected fragment and sends it as an HTMX partial
- Later phases add WebSocket push for live updates and eventually a JS compilation target for rich client components

---

## Phase 1: Server-Side Rendered Framework (works today)

Everything in this phase uses capabilities Marlang already has.

### HTML DSL

An `html` macro that produces HTML strings from S-expressions:

```scheme
(import "std:http" as http)
(import "framework:html" as h)

(h/render
  (h/html
    (h/head (h/title "My App"))
    (h/body
      (h/h1 "Hello")
      (h/p (string-append "Count: " (show count))))))
```

Implementation: pure functions that concatenate strings. Each element function takes attributes (as a list of pairs) and children (as strings). No special runtime support needed.

```scheme
; html.mar — core of the HTML DSL

(define element (lambda (tag attrs children)
  (let ((attr-str (attrs->string attrs)))
    (string-append "<" tag attr-str ">" children "</" tag ">"))))

(define attrs->string (lambda (attrs)
  (match attrs
    (Nil "")
    ((Cons (MkPair k v) rest)
      (string-append " " k "=\"" v "\"" (attrs->string rest))))))

(define html  (lambda (head body) (string-append "<!DOCTYPE html>" (element "html" (list) (string-append head body)))))
(define head  (lambda (child) (element "head" (list) child)))
(define body  (lambda (child) (element "body" (list) child)))
(define title (lambda (text)  (element "title" (list) text)))
(define h1    (lambda (text)  (element "h1" (list) text)))
(define p     (lambda (text)  (element "p" (list) text)))
(define div   (lambda (child) (element "div" (list) child)))
(define a     (lambda (attrs child) (element "a" attrs child)))
```

### Router

Path matching and parameter extraction, implemented as pure Marlang functions:

```scheme
(data RouteMatch
  (Matched Handler Params)
  (NotFound)
  (MethodNotAllowed))

; Params is (List (Pair String String))
; Handler is (-> Params HttpConnection Unit)

(data Route (Route String String (List (Pair String String)) HttpConnection))
;            method  path   params                           conn

(define match-route (lambda (method path routes)
  ; Walk the route table, match path patterns, extract params
  ...))
```

Routes are simple path-prefix or exact matches. Dynamic segments use `:param` syntax parsed at startup:

```scheme
(define routes (list
  (MkPair "GET"  (MkPair "/"           handle-home))
  (MkPair "GET"  (MkPair "/users/:id"  handle-user))
  (MkPair "POST" (MkPair "/users"      handle-create-user))))
```

### Request Handling Effect

A `Ctx` effect threads request context implicitly:

```scheme
(effect Ctx
  (ctx-param  (-> String (Option String)))
  (ctx-header (-> String (Option String)))
  (ctx-method (-> Unit String))
  (ctx-path   (-> Unit String)))
```

Route handlers perform `Ctx` effects. The server loop installs a handler that resolves them from the parsed `HttpRequest`:

```scheme
(define handle-home (lambda ()
  (let ((method (perform ctx-method))
        (path   (perform ctx-path)))
    (h/html
      (h/head (h/title "Home"))
      (h/body (h/h1 (string-append "Welcome — " method " " path)))))))
```

### Server Loop

Ties it all together using the HTTP server we just built:

```scheme
(define run-server (lambda (port routes)
  (let ((server (http/serve! port)))
    (io/println (string-append "Listening on http://127.0.0.1:" (show port)))
    (accept-loop server routes))))

(define accept-loop (lambda (server routes)
  (match (http/accept server)
    (None nil)
    ((Some (HttpRequest method path headers body conn))
      (begin
        (handle-request method path headers body conn routes)
        (accept-loop server routes))))))

(define handle-request (lambda (method path headers body conn routes)
  (match (match-route method path routes)
    ((Matched handler params)
      (let ((html (run-with-ctx method path headers params handler)))
        (http/respond-text! conn 200
          (list (MkPair "content-type" "text/html"))
          html)))
    (NotFound
      (http/respond-text! conn 404
        (list (MkPair "content-type" "text/plain"))
        "Not Found"))
    (MethodNotAllowed
      (http/respond-text! conn 405
        (list (MkPair "content-type" "text/plain"))
        "Method Not Allowed")))))

; Install the Ctx effect handler and run the route handler
(define run-with-ctx (lambda (method path headers params handler)
  (handle (handler)
    ((ctx-param key k) ->
      (resume k (lookup key params)))
    ((ctx-header key k) ->
      (resume k (lookup key headers)))
    ((ctx-method k) ->
      (resume k method))
    ((ctx-path k) ->
      (resume k path)))))
```

### What this gives us

A working web framework with:
- Declarative HTML rendering
- Effect-based request context (type-safe — handlers that need DB access are statically distinguishable from pure handlers)
- Pattern-matched routing
- Standard request/response cycle

No external dependencies. No JavaScript. Works today.

---

## Phase 2: HTMX Integration

HTMX lets the server drive interactivity via HTML attributes. The client includes a single `<script>` tag; all logic stays on the server.

### How it works

```scheme
; A button that triggers a server request and swaps the result into #counter
(h/button
  (list (MkPair "hx-post" "/increment")
        (MkPair "hx-target" "#counter")
        (MkPair "hx-swap" "innerHTML"))
  "Click me")

(h/div-with-id "counter" (show count))
```

When clicked, HTMX sends `POST /increment` to the server. The server returns an HTML fragment (not a full page). HTMX swaps it into the target element. No WebSocket needed.

### Partial rendering

Route handlers can return fragments instead of full pages:

```scheme
(define handle-increment (lambda ()
  ; update state, return just the counter fragment
  (h/span (show new-count))))
```

### HTMX attributes as DSL

```scheme
; Helper: attach hx-* attributes
(define hx-get    (lambda (url)    (MkPair "hx-get" url)))
(define hx-post   (lambda (url)    (MkPair "hx-post" url)))
(define hx-target (lambda (sel)    (MkPair "hx-target" sel)))
(define hx-swap   (lambda (strat)  (MkPair "hx-swap" strat)))
(define hx-trigger (lambda (evt)   (MkPair "hx-trigger" evt)))
```

### State management

Server-side state lives in actors. Each piece of shared state is an actor that handles get/set messages:

```scheme
(define make-counter (lambda ()
  (let ((pid (conc/spawn (lambda () (counter-loop 0)))))
    pid)))

(define counter-loop (lambda (n)
  (let ((msg (actor/receive)))
    (match msg
      ((MkPair "get" sender)
        (begin (actor/send sender n) (counter-loop n)))
      ((MkPair "inc" sender)
        (begin (actor/send sender (+ n 1)) (counter-loop (+ n 1))))))))
```

Route handlers send messages to state actors and render the result.

---

## Phase 3: WebSocket + Live Updates

**Requires: WebSocket runtime support (not yet built)**

Once Marlang has WebSocket support, the framework can push updates:

### Connection upgrade

The HTTP server accepts a WebSocket upgrade request, returning a bidirectional channel:

```scheme
(effect WebSocket
  (ws-accept   (-> HttpConnection (Result String WsConnection)))
  (ws-send     (-> WsConnection String (Result String Unit)))
  (ws-recv     (-> WsConnection (Option String)))
  (ws-close    (-> WsConnection Unit)))
```

### ConnectionActor

One actor per connected client. Owns the WebSocket, holds the current subscription set, batches updates:

```scheme
(data ConnectionMsg
  (DepUpdate String String)   ; dep-key, new-html-fragment
  (Navigate String)           ; new path
  (Disconnect))

(define connection-actor (lambda (ws ctx subscriptions)
  (let ((msg (actor/receive)))
    (match msg
      ((DepUpdate key html)
        (begin
          (ws/send! ws (patch-message key html))
          (connection-actor ws ctx subscriptions)))
      ((Navigate path)
        ; Tear down old subs, set up new ones
        ...)
      (Disconnect
        (ws/close ws))))))
```

### Server-sent patches

The server sends small JSON messages over the WebSocket:

```json
{"type": "patch", "selector": "#user-name", "html": "<span>Alice</span>"}
```

The client applies them with a tiny JS shim (morphdom or innerHTML replacement). This is ~20 lines of JavaScript, not a full client runtime.

### Dep protocol

A dep is a value that can be fetched and subscribed to:

```scheme
(data DepState (Loading) (Ready a) (Failed String))

; Dep operations as an effect
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

`combined` desugars into nested `dep-fetch` calls with dependency tracking. When an upstream value changes, downstream subscriptions are torn down and rebuilt.

---

## Phase 4: Database Integration

**Requires: Database driver (FFI or wire protocol)**

### Approach A: FFI to libpq

Use Marlang's existing FFI system to call libpq:

```scheme
(foreign-lib "libpq" "pq"
  (pq-connectdb (-> String PgConn))
  (pq-exec      (-> PgConn String PgResult))
  (pq-ntuples   (-> PgResult Int))
  (pq-getvalue  (-> PgResult Int Int String)))
```

### Approach B: Pure Marlang postgres wire protocol

Implement the Postgres wire protocol over TCP. More work, but no FFI dependency:

```scheme
; Connect via TCP, send startup message, handle auth, execute queries
(define pg-connect (lambda (host port db user password)
  (let ((sock (net/connect! host port)))
    (pg-startup sock db user)
    (pg-auth sock password)
    sock)))
```

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
      (Nil (h/p "User not found"))
      ((Cons row _) (h/p (string-append "User: " (car row))))))))
```

---

## Phase 5: Client Components (future)

**Requires: expanding the `emit-js` macro coverage**

JavaScript compilation is achieved via a macro (`emit-js`) that walks Marlang S-expressions at expansion time and produces JS strings — no Rust compiler backend needed. This has been prototyped and proven to work for: literals, lambda, let, if, begin, define, function calls, list/pair constructors, match/ADT destructuring, binary operators, and nested expressions.

Remaining JS coverage needed: `perform`/effects (compile to generator yields), tail-call trampolining, multi-statement bodies, and the client-side runtime (atom, event delegation, morphdom glue).

### `define-client`

```scheme
(define-client counter-widget (initial-count)
  ; This body compiles to JavaScript
  ; Only Dom and Actor effects allowed
  (let ((count (atom initial-count)))
    (h/div
      (h/span (show (deref count)))
      (h/button
        (list (MkPair "onclick" (client-fn () (swap! count (lambda (n) (+ n 1))))))
        "+1"))))
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
; Macro emits:
;   (define (counter-widget : (-> Int {Dom} Html)) ...)
; If the body tries to perform DB or IO, the type checker rejects it.
```

**Note:** Due to macro hygiene (see above), the macro must either receive the effect row from the user or the compiler's immune set must be extended to include type/effect names.

---

## The Actor Topology (Phases 3+)

```
AppActor
  └── RouterActor
        ├── SourceActor (per unique dep key)
        ├── SourceActor (...)
        ├── ConnectionActor (per connected client)
        └── ConnectionActor (...)
```

### SourceActor

- One per unique dep key across all connections
- Owns the underlying subscription (DB LISTEN, file watcher, timer)
- Fans out value changes to subscriber callbacks
- Reference counted — shuts down when last subscriber disconnects

### ConnectionActor

- One per connected WebSocket client
- Receives `DepUpdate` messages, batches by deferring flush
- Re-renders affected HTML fragments, sends patches over WebSocket
- Handles navigation (tear down old subs, set up new route)

### RouterActor

- One global instance
- Owns the source registry: a map from dep key to SourceActor pid
- On new connection: matches route, builds Ctx, resolves deps, spawns ConnectionActor

### AppActor

- Top of supervision tree
- Owns initialized resources (DB pool, config)
- Supervises RouterActor

---

## Key Data Types

```scheme
; --- Routing ---

(data RouteMatch
  (Matched Handler (List (Pair String String)))
  (NotFound)
  (MethodNotAllowed))

; --- Deps (Phase 3+) ---

(data DepState (Loading) (Ready a) (Failed String))

; --- Connection (Phase 3+) ---

(data ConnectionMsg
  (DepUpdate String String)
  (Navigate String)
  (RpcCall String Bytes)
  (Disconnect))

(data DisconnectReason
  (ClientLeft)
  (Timeout)
  (Error String))

; --- Render tree analysis (Phase 5) ---

(data RenderNode
  (Static String)
  (Dynamic (List String) String)
  (ClientBoundary String (List String)))
```

---

## Checklist

### Phase 1 — Server-Side Framework (no new runtime needed)
- [ ] HTML DSL — element functions, attribute rendering, escaping
- [ ] Router — path matching, `:param` extraction, method dispatch
- [ ] `Ctx` effect — param, header, method, path operations
- [ ] Server loop — accept, route, handle, respond
- [ ] Error pages — 404, 500
- [ ] Static file serving
- [ ] Example app

### Phase 2 — HTMX Integration (no new runtime needed)
- [ ] HTMX attribute helpers
- [ ] Fragment rendering (partial responses)
- [ ] State actors (server-side mutable state)
- [ ] Form handling
- [ ] Example: counter, todo list

### Phase 3 — Live Updates (requires new runtime: WebSocket)
- [ ] WebSocket effect — accept, send, recv, close
- [ ] ConnectionActor — per-client state + WebSocket
- [ ] Server-push patches — JSON message format
- [ ] Client JS shim — patch application (~20 lines)
- [ ] SourceActor — subscription fan-out
- [ ] RouterActor — source registry, connection lifecycle
- [ ] Navigation over WebSocket (no full reload)
- [ ] Dep protocol — fetch, subscribe, combined
- [ ] Batching — defer flush to end of actor turn

### Phase 4 — Database (requires new runtime: DB driver)
- [ ] DB effect — query, execute
- [ ] Connection pooling
- [ ] DB-backed deps — `db-row`, `db-table`, `db-query`
- [ ] LISTEN/NOTIFY integration for live dep updates

### Phase 5 — Client Components (extends `emit-js` macro)
- [x] `emit-js` macro — compiles Marlang to JS strings at expansion time (literals, lambda, let, if, begin, define, match, ADTs, operators)
- [ ] `emit-js` — effects via generator yields
- [ ] ADT representation in JS
- [ ] Trampolining for tail calls
- [ ] `define-client` macro — closed effect row, JS emit
- [ ] `define-shared` — dual emit (server + JS)
- [ ] `server-fn` — RPC stubs + signed tokens
- [ ] Atom implementation in JS
- [ ] Morphdom integration
- [ ] Event delegation
- [ ] Bundle manifest per route

---

## Open Questions

1. **Macro hygiene and types** — Should common type names (`Int`, `String`, `Bool`, `Unit`, `List`, etc.) and `__effect_row` be added to the hygiene immune set? This is a one-line compiler change that would let macros generate type annotations freely. Without it, macros must receive type expressions from the call site.

2. **WebSocket implementation** — Build on top of hyper's upgrade support? Or on raw TCP with a Marlang-level WebSocket frame parser? Hyper upgrade is the pragmatic choice.

3. **Database driver strategy** — FFI to libpq is fast to build. A pure Marlang postgres wire protocol over TCP is more portable and educational. Start with FFI?

4. **State management in Phase 1** — Actors work but are verbose. Should there be a simpler `(define-state name initial-value)` macro that generates get/set message handlers?

5. **Template escaping** — The HTML DSL must escape user content to prevent XSS. Where is the boundary? All `h/text` calls auto-escape; `h/raw` for trusted HTML.

6. **Session handling** — Cookie-based sessions are simplest. Need a signed cookie (HMAC) to prevent forgery. This requires the crypto capability from Phase 3+.

7. **CSS** — Out of scope for the framework. Serve static CSS files. Consider Tailwind-like utility classes as a convention.

8. **Hot reload** — File-watching the .mar source and restarting the server loop. Requires file watching (Phase 3+) or an external tool like `watchexec`.
