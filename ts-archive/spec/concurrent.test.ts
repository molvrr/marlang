import { describe, test, expect } from 'bun:test';
import * as Lexer from '../src/core/lexer';
import * as Parser from '../src/core/parser';
import { compileProgram } from '../src/vm/compiler';
import { createVM, interpret } from '../src/vm/vm';
import { runFibers } from '../src/vm/scheduler';
import { getLastRunFibersPromise } from '../src/vm/scheduler';
import type { VMValue, VMClosure } from '../src/vm/bytecode';
import { NIL } from '../src/vm/bytecode';

/** Run a Marlang program through the VM, awaiting run-fibers if needed.
 *  Returns the final result as a VMValue. */
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

/** Capture stdout output during an async run */
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

// ============================================================================
// Fiber Tests
// ============================================================================

describe('Concurrency - Fibers', () => {
  test('spawn returns pid', async () => {
    const output = await captureOutput(`
      (run-fibers (lambda ()
        (let ((pid (perform fiber-spawn (lambda () nil))))
          (perform println (show pid)))))
    `);
    expect(output.join('')).toContain('#<pid');
  });

  test('self returns pid', async () => {
    const output = await captureOutput(`
      (run-fibers (lambda ()
        (let ((me (perform fiber-self)))
          (perform println (show me)))))
    `);
    expect(output.join('')).toContain('#<pid 0>');
  });

  test('spawn runs fiber to completion', async () => {
    const output = await captureOutput(`
      (run-fibers (lambda ()
        (perform fiber-spawn (lambda ()
          (perform println "child")))
        (perform println "main")))
    `);
    const text = output.join('');
    expect(text).toContain('main');
    expect(text).toContain('child');
  });

  test('yield interleaves execution', async () => {
    const output = await captureOutput(`
      (run-fibers (lambda ()
        (perform fiber-spawn (lambda ()
          (perform println "B1")
          (perform fiber-yield)
          (perform println "B2")))
        (perform println "A1")
        (perform fiber-yield)
        (perform println "A2")))
    `);
    const text = output.join('');
    // A1 should come before A2, B1 before B2
    expect(text.indexOf('A1')).toBeLessThan(text.indexOf('A2'));
    expect(text.indexOf('B1')).toBeLessThan(text.indexOf('B2'));
  });

  test('multiple fibers run to completion', async () => {
    const output = await captureOutput(`
      (run-fibers (lambda ()
        (perform fiber-spawn (lambda () (perform println "f1")))
        (perform fiber-spawn (lambda () (perform println "f2")))
        (perform fiber-spawn (lambda () (perform println "f3")))
        (perform println "main")))
    `);
    const text = output.join('');
    expect(text).toContain('f1');
    expect(text).toContain('f2');
    expect(text).toContain('f3');
    expect(text).toContain('main');
  });

  test('fiber crash does not kill main', async () => {
    const output = await captureOutput(`
      (run-fibers (lambda ()
        (perform fiber-spawn (lambda () (error "boom")))
        (perform fiber-yield)
        (perform println "main survived")))
    `);
    expect(output.join('')).toContain('main survived');
  });
});

// ============================================================================
// Channel Tests
// ============================================================================

describe('Concurrency - Channels', () => {
  test('buffered channel send/recv', async () => {
    const output = await captureOutput(`
      (run-fibers (lambda ()
        (let ((ch (perform chan-new 10)))
          (perform chan-send ch 42)
          (let ((v (perform chan-recv ch)))
            (perform println v)))))
    `);
    expect(output.join('').trim()).toBe('42');
  });

  test('unbuffered channel synchronizes', async () => {
    const output = await captureOutput(`
      (run-fibers (lambda ()
        (let ((ch (perform chan-new 0)))
          (perform fiber-spawn (lambda ()
            (perform chan-send ch 99)))
          (let ((v (perform chan-recv ch)))
            (perform println v)))))
    `);
    expect(output.join('').trim()).toBe('99');
  });

  test('producer-consumer', async () => {
    const output = await captureOutput(`
      (run-fibers (lambda ()
        (let ((ch (perform chan-new 0)))
          (perform fiber-spawn (lambda ()
            (perform chan-send ch 1)
            (perform chan-send ch 2)
            (perform chan-send ch 3)
            (perform chan-close ch)))
          (define consume (lambda ()
            (let ((v (perform chan-recv ch)))
              (if (= v nil)
                nil
                (begin
                  (perform println v)
                  (consume))))))
          (consume))))
    `);
    expect(output.join('').trim()).toBe('1\n2\n3');
  });

  test('close wakes receivers', async () => {
    const output = await captureOutput(`
      (run-fibers (lambda ()
        (let ((ch (perform chan-new 0)))
          (perform fiber-spawn (lambda ()
            (let ((v (perform chan-recv ch)))
              (perform println (if (= v nil) "got-nil" "got-val")))))
          (perform fiber-yield)
          (perform chan-close ch)
          (perform fiber-yield))))
    `);
    expect(output.join('').trim()).toBe('got-nil');
  });

  test('fan-out collects results', async () => {
    const output = await captureOutput(`
      (run-fibers (lambda ()
        (let ((ch (perform chan-new 10)))
          (perform fiber-spawn (lambda () (perform chan-send ch (+ 1 1))))
          (perform fiber-spawn (lambda () (perform chan-send ch (+ 2 2))))
          (perform fiber-spawn (lambda () (perform chan-send ch (+ 3 3))))
          (let ((a (perform chan-recv ch))
                (b (perform chan-recv ch))
                (c (perform chan-recv ch)))
            (perform println (+ a (+ b c)))))))
    `);
    expect(output.join('').trim()).toBe('12');
  });
});

// ============================================================================
// Actor Tests
// ============================================================================

describe('Concurrency - Actors', () => {
  test('send/receive basic', async () => {
    const output = await captureOutput(`
      (run-fibers (lambda ()
        (let ((me (perform fiber-self)))
          (perform fiber-spawn (lambda ()
            (perform actor-send me "hello")))
          (let ((msg (perform actor-receive)))
            (perform println msg)))))
    `);
    expect(output.join('').trim()).toBe('hello');
  });

  test('receive blocks until message', async () => {
    const output = await captureOutput(`
      (run-fibers (lambda ()
        (let ((me (perform fiber-self)))
          (perform fiber-spawn (lambda ()
            (perform fiber-yield)
            (perform actor-send me "delayed")))
          (let ((msg (perform actor-receive)))
            (perform println msg)))))
    `);
    expect(output.join('').trim()).toBe('delayed');
  });

  test('selective receive', async () => {
    const output = await captureOutput(`
      (data Msg (A Int) (B Int))
      (run-fibers (lambda ()
        (let ((me (perform fiber-self)))
          (perform fiber-spawn (lambda ()
            (perform actor-send me (A 1))
            (perform actor-send me (B 2))
            (perform actor-send me (A 3))))
          (perform fiber-yield)
          (perform fiber-yield)
          ;; Select only B messages
          (let ((msg (perform actor-receive-match
                       (lambda (m)
                         (match m
                           ((B _) #t)
                           (_ #f))))))
            (match msg
              ((B n) (perform println n)))))))
    `);
    expect(output.join('').trim()).toBe('2');
  });
});

// ============================================================================
// Supervision Tests
// ============================================================================

describe('Concurrency - Supervision', () => {
  test('monitor delivers Down on crash', async () => {
    const output = await captureOutput(`
      (data Signal (Down Pid String) (Exit Pid String))
      (run-fibers (lambda ()
        (let ((worker (perform fiber-spawn (lambda ()
                        (perform fiber-yield)
                        (error "boom")))))
          (perform actor-monitor worker)
          (perform fiber-yield)
          (perform fiber-yield)
          (perform fiber-yield)
          (let ((msg (perform actor-receive)))
            (match msg
              ((Down pid reason) (perform println reason)))))))
    `);
    expect(output.join('').trim()).toBe('boom');
  });

  test('monitor delivers Down normal on clean exit', async () => {
    const output = await captureOutput(`
      (data Signal (Down Pid String) (Exit Pid String))
      (run-fibers (lambda ()
        (let ((worker (perform fiber-spawn (lambda () nil))))
          (perform actor-monitor worker)
          (perform fiber-yield)
          (perform fiber-yield)
          (let ((msg (perform actor-receive)))
            (match msg
              ((Down pid reason) (perform println reason)))))))
    `);
    expect(output.join('').trim()).toBe('normal');
  });

  test('trap-exit converts crash to message', async () => {
    const output = await captureOutput(`
      (data Signal (Down Pid String) (Exit Pid String))
      (run-fibers (lambda ()
        (perform actor-trap-exit #t)
        (let ((worker (perform fiber-spawn (lambda ()
                        (perform fiber-yield)
                        (error "crash")))))
          (perform actor-link worker)
          (perform fiber-yield)
          (perform fiber-yield)
          (perform fiber-yield)
          (let ((msg (perform actor-receive)))
            (match msg
              ((Exit pid reason) (perform println (+ "caught: " reason))))))))
    `);
    expect(output.join('').trim()).toBe('caught: crash');
  });
});

// ============================================================================
// IO Inside run-fibers Tests
// ============================================================================

describe('Concurrency - IO', () => {
  test('println inside run-fibers', async () => {
    const output = await captureOutput(`
      (run-fibers (lambda ()
        (perform println "hello from fibers")))
    `);
    expect(output.join('').trim()).toBe('hello from fibers');
  });

  test('multiple fibers printing', async () => {
    const output = await captureOutput(`
      (run-fibers (lambda ()
        (perform fiber-spawn (lambda () (perform println "fiber1")))
        (perform fiber-spawn (lambda () (perform println "fiber2")))
        (perform println "main")))
    `);
    const text = output.join('');
    expect(text).toContain('main');
    expect(text).toContain('fiber1');
    expect(text).toContain('fiber2');
  });

  test('async file read', async () => {
    // Create a temp file, read it inside run-fibers
    const tmpPath = '/tmp/marlang-test-read-' + Date.now() + '.txt';
    await Bun.write(tmpPath, 'test content');
    try {
      const output = await captureOutput(`
        (run-fibers (lambda ()
          (let ((content (perform read-file "${tmpPath}")))
            (perform println content))))
      `);
      expect(output.join('').trim()).toBe('test content');
    } finally {
      await Bun.write(tmpPath, ''); // cleanup
    }
  });

  test('concurrent file reads', async () => {
    const tmp1 = '/tmp/marlang-cr1-' + Date.now() + '.txt';
    const tmp2 = '/tmp/marlang-cr2-' + Date.now() + '.txt';
    await Bun.write(tmp1, 'file1');
    await Bun.write(tmp2, 'file2');
    try {
      const output = await captureOutput(`
        (run-fibers (lambda ()
          (let ((ch (perform chan-new 10)))
            (perform fiber-spawn (lambda ()
              (perform chan-send ch (perform read-file "${tmp1}"))))
            (perform fiber-spawn (lambda ()
              (perform chan-send ch (perform read-file "${tmp2}"))))
            (let ((a (perform chan-recv ch))
                  (b (perform chan-recv ch)))
              (perform println a)
              (perform println b)))))
      `);
      const text = output.join('');
      expect(text).toContain('file1');
      expect(text).toContain('file2');
    } finally {
      await Bun.write(tmp1, '');
      await Bun.write(tmp2, '');
    }
  });
});

// ============================================================================
// Edge Cases
// ============================================================================

describe('Concurrency - Edge Cases', () => {
  test('main fiber result returned', async () => {
    const result = await runAsync(`
      (run-fibers (lambda () 42))
    `);
    // The result comes through the promise
    const promise = getLastRunFibersPromise();
    // Result should have been captured already
  });

  test('IO outside run-fibers still works', async () => {
    // Default IO handler uses console.log — test via interpret directly
    const tokens = Lexer.lex(Lexer.empty(`(perform println "sync io")`));
    const parsed = Parser.parse(Parser.empty(tokens));
    const chunk = compileProgram(parsed.exprs);
    const vm = createVM();
    const origLog = console.log;
    const lines: string[] = [];
    console.log = (...args: any[]) => { lines.push(args.join(' ')); };
    try {
      interpret(vm, chunk);
    } finally {
      console.log = origLog;
    }
    expect(lines.join('\n')).toContain('sync io');
  });

  test('counter actor pattern', async () => {
    const output = await captureOutput(`
      (data CounterMsg
        (Increment Int)
        (GetCount Pid))
      (data CounterReply
        (Count Int))

      (define counter (lambda (count)
        (let ((msg (perform actor-receive)))
          (match msg
            ((Increment n) (counter (+ count n)))
            ((GetCount sender)
              (begin
                (perform actor-send sender (Count count))
                (counter count)))))))

      (run-fibers (lambda ()
        (let ((c (perform fiber-spawn (lambda () (counter 0))))
              (me (perform fiber-self)))
          (perform actor-send c (Increment 5))
          (perform actor-send c (Increment 3))
          (perform actor-send c (GetCount me))
          (let ((reply (perform actor-receive)))
            (match reply
              ((Count n) (perform println n)))))))
    `);
    expect(output.join('').trim()).toBe('8');
  });

  test('ping-pong pattern', async () => {
    const output = await captureOutput(`
      (data PingPong (Ping Pid) (Pong Pid))

      (define pong (lambda ()
        (let ((msg (perform actor-receive)))
          (match msg
            ((Ping sender)
              (let ((me (perform fiber-self)))
                (perform actor-send sender (Pong me))
                (pong)))))))

      (define ping (lambda (partner n)
        (if (<= n 0)
          (perform println "done")
          (let ((me (perform fiber-self)))
            (perform actor-send partner (Ping me))
            (let ((msg (perform actor-receive)))
              (match msg
                ((Pong _) (ping partner (- n 1)))))))))

      (run-fibers (lambda ()
        (let ((p (perform fiber-spawn pong)))
          (ping p 100))))
    `);
    expect(output.join('').trim()).toBe('done');
  });
});
