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
// TCP Tests
// ============================================================================

describe('Networking - TCP', () => {
  test('tcp echo server', async () => {
    const port = randomPort();
    const output = await captureOutput(`
      (run-fibers (lambda ()
        (let ((listener (perform tcp-listen ${port})))
          ;; Server fiber: accept one connection, echo back, close
          (perform fiber-spawn (lambda ()
            (let ((client (perform tcp-accept listener)))
              (let ((data (perform tcp-recv client)))
                (perform tcp-send client data)
                (perform tcp-close client)))))
          ;; Give server time to start
          (perform fiber-yield)
          ;; Client: connect, send, recv, print
          (let ((sock (perform tcp-connect "127.0.0.1" ${port})))
            (perform tcp-send sock "hello")
            (let ((resp (perform tcp-recv sock)))
              (perform println resp)
              (perform tcp-close sock)))
          (perform tcp-close listener))))
    `);
    expect(output.join('').trim()).toBe('hello');
  });

  test('tcp recv returns nil on close', async () => {
    const port = randomPort();
    const output = await captureOutput(`
      (run-fibers (lambda ()
        (let ((listener (perform tcp-listen ${port})))
          ;; Server: accept and immediately close
          (perform fiber-spawn (lambda ()
            (let ((client (perform tcp-accept listener)))
              (perform tcp-close client))))
          (perform fiber-yield)
          ;; Client: connect, recv should get nil
          (let ((sock (perform tcp-connect "127.0.0.1" ${port})))
            (let ((data (perform tcp-recv sock)))
              (perform println (if (= data nil) "closed" "data")))
            (perform tcp-close sock))
          (perform tcp-close listener))))
    `);
    expect(output.join('').trim()).toBe('closed');
  });

  test('tcp send and receive data', async () => {
    const port = randomPort();
    const output = await captureOutput(`
      (run-fibers (lambda ()
        (let ((listener (perform tcp-listen ${port})))
          ;; Server: accept, receive once, echo it back
          (perform fiber-spawn (lambda ()
            (let ((client (perform tcp-accept listener)))
              (let ((d1 (perform tcp-recv client)))
                (perform tcp-send client (+ "echo:" d1))
                (perform tcp-close client)))))
          (perform fiber-yield)
          ;; Client
          (let ((sock (perform tcp-connect "127.0.0.1" ${port})))
            (perform tcp-send sock "foobar")
            (let ((resp (perform tcp-recv sock)))
              (perform println resp)
              (perform tcp-close sock)))
          (perform tcp-close listener))))
    `);
    expect(output.join('').trim()).toBe('echo:foobar');
  });

  test('tcp listener close wakes acceptor', async () => {
    const port = randomPort();
    const output = await captureOutput(`
      (run-fibers (lambda ()
        (let ((listener (perform tcp-listen ${port})))
          ;; Fiber waiting to accept
          (perform fiber-spawn (lambda ()
            (let ((result (perform tcp-accept listener)))
              (perform println (if (= result nil) "woke-nil" "got-conn")))))
          (perform fiber-yield)
          ;; Close the listener while someone is waiting
          (perform tcp-close listener)
          (perform fiber-yield)
          (perform fiber-yield))))
    `);
    expect(output.join('').trim()).toBe('woke-nil');
  });
});
