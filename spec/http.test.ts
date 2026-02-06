import { describe, test, expect } from 'bun:test';
import * as Lexer from '../src/core/lexer';
import * as Parser from '../src/core/parser';
import { compileProgram } from '../src/vm/compiler';
import { createVM, interpret } from '../src/vm/vm';
import { getLastRunFibersPromise } from '../src/vm/scheduler';
import type { VMValue } from '../src/vm/bytecode';

async function runAsync(code: string): Promise<VMValue> {
  const tokens = Lexer.lex(Lexer.empty(code));
  const parsed = Parser.parse(Parser.empty(tokens));
  const chunk = compileProgram(parsed.exprs);
  const vm = createVM();
  const result = interpret(vm, chunk);
  const promise = getLastRunFibersPromise();
  if (promise) {
    return await promise;
  }
  return result;
}

async function captureOutput(code: string): Promise<string[]> {
  const lines: string[] = [];
  const origWrite = process.stdout.write;
  process.stdout.write = ((chunk: any) => {
    lines.push(String(chunk));
    return true;
  }) as any;
  try {
    await runAsync(code);
  } finally {
    process.stdout.write = origWrite;
  }
  return lines;
}

function randomPort(): number {
  return 10000 + Math.floor(Math.random() * 50000);
}

// ============================================================================
// HTTP Client Tests
// ============================================================================

describe('Networking - HTTP Client', () => {
  test('http-fetch GET from Bun server', async () => {
    const port = randomPort();
    // Start a Bun HTTP server from TS side
    const server = Bun.serve({
      port,
      hostname: '127.0.0.1',
      fetch() {
        return new Response('hello from bun');
      },
    });

    try {
      const output = await captureOutput(`
        (data HttpResponse (HttpResponse Int (List (List String)) String))
        (data HttpOption (Method String) (Header String String) (Body String))
        (run-fibers (lambda ()
          (let ((resp (perform http-fetch "http://127.0.0.1:${port}" (list))))
            (match resp
              ((HttpResponse status headers body)
                (begin
                  (perform println status)
                  (perform println body)))))))
      `);
      const text = output.join('');
      expect(text).toContain('200');
      expect(text).toContain('hello from bun');
    } finally {
      server.stop();
    }
  });

  test('http-fetch POST', async () => {
    const port = randomPort();
    const server = Bun.serve({
      port,
      hostname: '127.0.0.1',
      async fetch(req) {
        const body = await req.text();
        return new Response(`got: ${body}`);
      },
    });

    try {
      const output = await captureOutput(`
        (data HttpResponse (HttpResponse Int (List (List String)) String))
        (data HttpOption (Method String) (Header String String) (Body String))
        (run-fibers (lambda ()
          (let ((resp (perform http-fetch "http://127.0.0.1:${port}"
                        (list (Method "POST") (Body "test-data")))))
            (match resp
              ((HttpResponse status headers body)
                (perform println body))))))
      `);
      expect(output.join('').trim()).toBe('got: test-data');
    } finally {
      server.stop();
    }
  });
});

// ============================================================================
// HTTP Server Tests
// ============================================================================

describe('Networking - HTTP Server', () => {
  test('http server handles request from TS', async () => {
    const port = randomPort();

    // Run Marlang HTTP server in background
    const serverPromise = runAsync(`
      (data HttpResponse (HttpResponse Int (List (List String)) String))
      (data HttpReq (HttpReq HttpRequest String String (List (List String)) String))
      (data HttpOption (Method String) (Header String String) (Body String))
      (run-fibers (lambda ()
        (let ((server (perform http-serve ${port})))
          ;; Handle one request then close
          (let ((req (perform http-accept server)))
            (match req
              ((HttpReq handle method url headers body)
                (perform http-respond handle 200
                  (list (list "content-type" "text/plain"))
                  (+ "method=" method)))))
          (perform http-close-server server))))
    `);

    // Give the server time to start
    await new Promise(r => setTimeout(r, 100));

    // Send a request from TS
    const resp = await fetch(`http://127.0.0.1:${port}/test`);
    const body = await resp.text();
    expect(resp.status).toBe(200);
    expect(body).toBe('method=GET');

    // Wait for server to shut down
    await serverPromise;
  });

  test('http server handles multiple requests', async () => {
    const port = randomPort();

    const serverPromise = runAsync(`
      (data HttpResponse (HttpResponse Int (List (List String)) String))
      (data HttpReq (HttpReq HttpRequest String String (List (List String)) String))
      (data HttpOption (Method String) (Header String String) (Body String))
      (run-fibers (lambda ()
        (let ((server (perform http-serve ${port})))
          ;; Handle two requests
          (define handle-one (lambda ()
            (let ((req (perform http-accept server)))
              (match req
                ((HttpReq handle method url headers body)
                  (perform http-respond handle 200 (list) "ok"))))))
          (handle-one)
          (handle-one)
          (perform http-close-server server))))
    `);

    await new Promise(r => setTimeout(r, 100));

    const resp1 = await fetch(`http://127.0.0.1:${port}/a`);
    expect(resp1.status).toBe(200);
    expect(await resp1.text()).toBe('ok');

    const resp2 = await fetch(`http://127.0.0.1:${port}/b`);
    expect(resp2.status).toBe(200);
    expect(await resp2.text()).toBe('ok');

    await serverPromise;
  });

  test('http server POST body', async () => {
    const port = randomPort();

    const serverPromise = runAsync(`
      (data HttpResponse (HttpResponse Int (List (List String)) String))
      (data HttpReq (HttpReq HttpRequest String String (List (List String)) String))
      (data HttpOption (Method String) (Header String String) (Body String))
      (run-fibers (lambda ()
        (let ((server (perform http-serve ${port})))
          (let ((req (perform http-accept server)))
            (match req
              ((HttpReq handle method url headers body)
                (perform http-respond handle 200 (list) (+ "echo:" body)))))
          (perform http-close-server server))))
    `);

    await new Promise(r => setTimeout(r, 100));

    const resp = await fetch(`http://127.0.0.1:${port}`, {
      method: 'POST',
      body: 'payload',
    });
    expect(await resp.text()).toBe('echo:payload');

    await serverPromise;
  });
});
