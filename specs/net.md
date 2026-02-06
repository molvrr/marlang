# Network Stack Specification

## Overview

TCP, HTTP client, and HTTP server support for the Marlang VM, integrated with
the existing algebraic effect / fiber scheduler. All networking operations are
effects handled natively by `scheduler.ts`.

## Architecture

```
Layer 3 — HTTP Server   Bun.serve()   → http-serve, http-accept, http-respond, http-close-server
Layer 2 — HTTP Client   fetch()       → http-fetch
Layer 1 — TCP           Bun sockets   → tcp-connect, tcp-listen, tcp-accept, tcp-send, tcp-recv, tcp-close
```

## VMValue Types (bytecode.ts)

```typescript
VMTcpSocket   = { kind: 'tcp-socket';   id: number }
VMTcpListener = { kind: 'tcp-listener'; id: number }
VMHttpServer  = { kind: 'http-server';  id: number }
VMHttpRequest = { kind: 'http-request'; id: number }
```

Added to `VMObject` union and handled in `formatVMValue`, `valuesEqual`,
`toRuntimeValue`.

## Scheduler State

```typescript
// TCP
tcpSockets:    Map<number, TcpSocketState>
tcpListeners:  Map<number, TcpListenerState>
nextTcpId:     number

// HTTP
httpServers:      Map<number, HttpServerState>
httpRequests:     Map<number, HttpRequestState>
nextHttpServerId: number
nextHttpReqId:    number

// Server lifecycle
activeServers: number          // live TCP listeners + HTTP servers
serverWaker:   { resolve: () => void } | null
```

### Internal state types

```typescript
type TcpSocketState = {
  socket: any;               // Bun socket handle
  recvQueue: string[];       // buffered incoming data
  waitingRecv: { pid: FiberId; cont: VMContinuation } | null;
  closed: boolean;
};

type TcpListenerState = {
  server: any;               // Bun.listen() handle
  acceptQueue: number[];     // queued socket IDs
  waitingAccept: { pid: FiberId; cont: VMContinuation } | null;
};

type HttpServerState = {
  server: any;               // Bun.serve() handle
  requestQueue: number[];    // queued request IDs
  waitingAccept: { pid: FiberId; cont: VMContinuation } | null;
};

type HttpRequestState = {
  resolve: (resp: Response) => void;
  method: string;
  url: string;
  headers: [string, string][];
  body: string;
};
```

## Waker Mechanism

When a Bun callback fires from outside the scheduler loop (new connection,
new HTTP request) and enqueues work, it calls `wakeScheduler(state)`:

```typescript
function wakeScheduler(state: SchedulerState): void {
  if (state.serverWaker) {
    state.serverWaker.resolve();
    state.serverWaker = null;
  }
}
```

Loop condition becomes:
```
while (readyQueue.length > 0 || pendingIO.size > 0 || activeServers > 0)
```

Drain phase: if nothing is pending but `activeServers > 0`, await a waker
promise (with 100ms timeout to avoid deadlock).

## Effect Handlers

### TCP

| Effect | Args | Async? | Resumes with |
|--------|------|--------|-------------|
| `tcp-connect` | host port | Yes (pendingIO) | VMTcpSocket |
| `tcp-listen` | port | Sync | VMTcpListener |
| `tcp-accept` | listener | Parks | VMTcpSocket |
| `tcp-send` | socket data | Sync | nil |
| `tcp-recv` | socket | Parks | String or nil on close |
| `tcp-close` | socket/listener | Sync | nil |

Bun socket callbacks route events via `(socket as any).__marlangId`.

### HTTP Client

| Effect | Args | Async? | Resumes with |
|--------|------|--------|-------------|
| `http-fetch` | url options | Yes (pendingIO) | HttpResponse ADT |

Options: Marlang list of `(Method str)`, `(Header k v)`, `(Body str)`.
Response: `(HttpResponse status headers body)`.

### HTTP Server

| Effect | Args | Async? | Resumes with |
|--------|------|--------|-------------|
| `http-serve` | port | Sync | VMHttpServer |
| `http-accept` | server | Parks | HttpReq ADT |
| `http-respond` | handle status headers body | Sync | nil |
| `http-close-server` | server | Sync | nil |

Request ADT: `(HttpReq handle method url headers body)`.

## Stdlib

### stdlib/net.mar
```lisp
(effect Tcp
  (tcp-connect (-> String Int TcpSocket))
  (tcp-listen  (-> Int TcpListener))
  (tcp-accept  (-> TcpListener TcpSocket))
  (tcp-send    (-> TcpSocket String Unit))
  (tcp-recv    (-> TcpSocket String))
  (tcp-close   (-> TcpSocket Unit)))
```
Plus wrapper functions.

### stdlib/http.mar
```lisp
(data HttpResponse (HttpResponse Int (List (Pair String String)) String))
(data HttpReq (HttpReq HttpRequest String String (List (Pair String String)) String))
(data HttpOption (Method String) (Header String String) (Body String))

(effect HttpClient (http-fetch (-> String (List HttpOption) HttpResponse)))
(effect HttpServ
  (http-serve        (-> Int HttpServer))
  (http-accept       (-> HttpServer HttpReq))
  (http-respond      (-> HttpRequest Int (List (Pair String String)) String Unit))
  (http-close-server (-> HttpServer Unit)))
```
Plus wrappers and convenience functions (http-get, http-post, respond-text, respond-json).

## Cleanup

At end of `runFibers`, stop all TCP listeners and HTTP servers, close all
TCP sockets to prevent resource leaks.

## Testing

- **spec/net.test.ts** — TCP echo server, connect/send/recv, close-returns-nil
- **spec/http.test.ts** — HTTP fetch (start Bun server from TS, fetch from Marlang),
  HTTP server (start Marlang server, send request from TS)
- Use random high ports to avoid conflicts
- Use `captureOutput` pattern from concurrent.test.ts
