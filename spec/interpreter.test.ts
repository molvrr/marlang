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

// Helper to run code with fresh interpreter
const eval_ = (code: string): string => {
  const tokens = Lexer.lex(Lexer.empty(code));
  const parsed = Parser.parse(Parser.empty(tokens));
  const result = Interpreter.evalMany(Interpreter.empty(), parsed.exprs);
  return Interpreter.formatValue(result.value);
};

describe('Arithmetic', () => {
  test('addition', () => {
    expect(eval_('(+ 1 2)')).toBe('3');
  });

  test('subtraction', () => {
    expect(eval_('(- 10 3)')).toBe('7');
  });

  test('multiplication', () => {
    expect(eval_('(* 4 5)')).toBe('20');
  });

  test('division', () => {
    expect(eval_('(/ 20 4)')).toBe('5');
  });

  test('variadic addition', () => {
    expect(eval_('(+ 1 2 3 4 5)')).toBe('15');
  });
});

describe('Variables', () => {
  test('define and use variable', () => {
    const result = eval_(`
      (define x 10)
      (+ x 5)
    `);
    expect(result).toBe('15');
  });
});

describe('Functions', () => {
  test('define and call lambda', () => {
    const result = eval_(`
      (define square (lambda (n) (* n n)))
      (square 4)
    `);
    expect(result).toBe('16');
  });
});

describe('Conditionals', () => {
  test('if true branch', () => {
    expect(eval_('(if (< 1 2) 10 20)')).toBe('10');
  });

  test('if false branch', () => {
    expect(eval_('(if (> 1 2) 10 20)')).toBe('20');
  });
});

describe('Let Bindings', () => {
  test('basic let binding', () => {
    expect(eval_('(let ((a 1) (b 2)) (+ a b))')).toBe('3');
  });
});

describe('Recursion', () => {
  test('factorial', () => {
    const result = eval_(`
      (define fact (lambda (n) (if (= n 0) 1 (* n (fact (- n 1))))))
      (fact 5)
    `);
    expect(result).toBe('120');
  });
});

describe('Closures', () => {
  test('make-adder closure', () => {
    const result = eval_(`
      (define make-adder (lambda (n) (lambda (x) (+ n x))))
      ((make-adder 5) 10)
    `);
    expect(result).toBe('15');
  });
});

describe('Cond', () => {
  test('cond with multiple branches', () => {
    let interp = Interpreter.empty();
    let result = run(interp, `
      (define grade (lambda (score)
        (cond
          ((>= score 90) 1)
          ((>= score 80) 2)
          ((>= score 70) 3)
          (else 4))))
    `);
    interp = result.interp;

    result = run(interp, '(grade 95)');
    expect(result.value).toBe('1');

    result = run(interp, '(grade 85)');
    expect(result.value).toBe('2');

    result = run(interp, '(grade 65)');
    expect(result.value).toBe('4');
  });
});

describe('Strings', () => {
  test('string literal', () => {
    expect(eval_('"hello world"')).toBe('"hello world"');
  });

  test('list of strings', () => {
    expect(eval_('(list "a" "b" "c")')).toBe('("a" "b" "c")');
  });
});

describe('FFI', () => {
  test('libm math functions', () => {
    let interp = Interpreter.empty();

    let result = run(interp, `
      (define math (ffi-open "m" (list
        (list "sin" (list "f64") "f64")
        (list "cos" (list "f64") "f64")
        (list "pow" (list "f64" "f64") "f64"))))
    `);
    interp = result.interp;

    result = run(interp, '(ffi-call math "sin" 0.0)');
    expect(result.value).toBe('0');

    result = run(interp, '(ffi-call math "sin" 1.5707963267948966)');
    expect(result.value).toBe('1');

    result = run(interp, '(ffi-call math "cos" 0.0)');
    expect(result.value).toBe('1');

    result = run(interp, '(ffi-call math "pow" 2.0 10.0)');
    expect(result.value).toBe('1024');

    result = run(interp, '(ffi-close math)');
    expect(result.value).toBe('nil');
  });
});
