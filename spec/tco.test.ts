import { test, expect, describe } from 'bun:test';
import * as Lexer from '../lexer';
import * as Parser from '../parser';
import * as Interpreter from '../interpreter';

// Helper to run code and return result
const run = (interp: Interpreter.T, code: string): { interp: Interpreter.T; value: string } => {
  const tokens = Lexer.lex(Lexer.empty(code));
  const parsed = Parser.parse(Parser.empty(tokens));
  const result = Interpreter.evalMany(interp, parsed.exprs);
  return { interp: result.interp, value: Interpreter.formatValue(result.value) };
};

describe('Tail Call Optimization', () => {
  test('tail-recursive factorial with small value', () => {
    let interp = Interpreter.empty();
    const factTailCode = `
      (define fact-tail (lambda (n acc)
        (if (= n 0)
          acc
          (fact-tail (- n 1) (* n acc)))))
    `;
    let result = run(interp, factTailCode);
    interp = result.interp;

    result = run(interp, '(fact-tail 10 1)');
    expect(result.value).toBe('3628800');
  });

  test('tail-recursive factorial with large value (no stack overflow)', () => {
    let interp = Interpreter.empty();
    const factTailCode = `
      (define fact-tail (lambda (n acc)
        (if (= n 0)
          acc
          (fact-tail (- n 1) (* n acc)))))
    `;
    let result = run(interp, factTailCode);
    interp = result.interp;

    // This would stack overflow without TCO
    result = run(interp, '(fact-tail 50000 1)');
    // Result is Infinity due to number overflow, but no stack overflow
    expect(result.value).toBe('Infinity');
  });

  test('mutual recursion is-even/is-odd', () => {
    let interp = Interpreter.empty();

    const isEvenCode = `
      (define is-even (lambda (n)
        (if (= n 0) 1 (is-odd (- n 1)))))
    `;
    const isOddCode = `
      (define is-odd (lambda (n)
        (if (= n 0) 0 (is-even (- n 1)))))
    `;

    let result = run(interp, isEvenCode);
    interp = result.interp;
    result = run(interp, isOddCode);
    interp = result.interp;

    result = run(interp, '(is-even 4)');
    expect(result.value).toBe('1');

    result = run(interp, '(is-even 5)');
    expect(result.value).toBe('0');
  });

  test('mutual recursion with large value (no stack overflow)', () => {
    let interp = Interpreter.empty();

    const isEvenCode = `
      (define is-even (lambda (n)
        (if (= n 0) 1 (is-odd (- n 1)))))
    `;
    const isOddCode = `
      (define is-odd (lambda (n)
        (if (= n 0) 0 (is-even (- n 1)))))
    `;

    let result = run(interp, isEvenCode);
    interp = result.interp;
    result = run(interp, isOddCode);
    interp = result.interp;

    // This would stack overflow without TCO
    result = run(interp, '(is-even 50000)');
    expect(result.value).toBe('1');
  });

  test('non-tail recursion still works', () => {
    let interp = Interpreter.empty();
    const factCode = `
      (define fact (lambda (n)
        (if (= n 0) 1 (* n (fact (- n 1))))))
    `;
    let result = run(interp, factCode);
    interp = result.interp;

    result = run(interp, '(fact 10)');
    expect(result.value).toBe('3628800');
  });

  test('tail calls in let bodies', () => {
    let interp = Interpreter.empty();
    const letTailCode = `
      (define count-down (lambda (n)
        (let ((m (- n 1)))
          (if (= m 0)
            0
            (count-down m)))))
    `;
    let result = run(interp, letTailCode);
    interp = result.interp;

    // This would stack overflow without TCO
    result = run(interp, '(count-down 50000)');
    expect(result.value).toBe('0');
  });

  test('tail calls in cond', () => {
    let interp = Interpreter.empty();
    const condTailCode = `
      (define count-cond (lambda (n)
        (cond
          ((= n 0) 0)
          (else (count-cond (- n 1))))))
    `;
    let result = run(interp, condTailCode);
    interp = result.interp;

    // This would stack overflow without TCO
    result = run(interp, '(count-cond 50000)');
    expect(result.value).toBe('0');
  });
});
