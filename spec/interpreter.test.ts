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

describe('Begin', () => {
  test('begin with single expression', () => {
    expect(eval_('(begin 42)')).toBe('42');
  });

  test('begin with multiple expressions', () => {
    expect(eval_('(begin 1 2 3)')).toBe('3');
  });

  test('begin with side effects', () => {
    const result = eval_(`
      (begin
        (define x 10)
        (define y 20)
        (+ x y))
    `);
    expect(result).toBe('30');
  });

  test('begin returns last expression value', () => {
    expect(eval_('(begin "first" "second" "third")')).toBe('"third"');
  });

  test('begin in function body', () => {
    const result = eval_(`
      (define test (lambda ()
        (begin
          (define a 1)
          (define b 2)
          (+ a b))))
      (test)
    `);
    expect(result).toBe('3');
  });

  test('nested begin', () => {
    const result = eval_(`
      (begin
        (begin 1 2)
        (begin 3 4))
    `);
    expect(result).toBe('4');
  });

  test('empty begin returns nil', () => {
    expect(eval_('(begin)')).toBe('nil');
  });
});

describe('Boolean Literals', () => {
  test('true comparison result', () => {
    expect(eval_('(< 1 2)')).toBe('#t');
  });

  test('false comparison result', () => {
    expect(eval_('(> 1 2)')).toBe('#f');
  });

  test('equality returns boolean', () => {
    expect(eval_('(= 5 5)')).toBe('#t');
    expect(eval_('(= 5 6)')).toBe('#f');
  });
});

describe('Negative Numbers', () => {
  test('unary minus', () => {
    expect(eval_('(- 5)')).toBe('-5');
  });

  test('negative result from subtraction', () => {
    expect(eval_('(- 3 10)')).toBe('-7');
  });

  test('negative in arithmetic', () => {
    expect(eval_('(+ (- 5) 10)')).toBe('5');
  });

  test('multiply negative', () => {
    expect(eval_('(* (- 3) 4)')).toBe('-12');
  });
});

describe('Let Bindings - Extended', () => {
  test('nested let bindings', () => {
    const result = eval_(`
      (let ((x 1))
        (let ((y 2))
          (+ x y)))
    `);
    expect(result).toBe('3');
  });

  test('let shadowing', () => {
    const result = eval_(`
      (let ((x 1))
        (let ((x 2))
          x))
    `);
    expect(result).toBe('2');
  });

  test('let sequential bindings', () => {
    const result = eval_(`
      (let ((x 1) (y (+ 1 1)))
        (+ x y))
    `);
    expect(result).toBe('3');
  });

  test('let with function', () => {
    const result = eval_(`
      (let ((f (lambda (x) (* x 2))))
        (f 5))
    `);
    expect(result).toBe('10');
  });
});

describe('Cond - Extended', () => {
  test('cond first branch matches', () => {
    expect(eval_('(cond ((< 1 2) 10) (else 20))')).toBe('10');
  });

  test('cond falls through to else', () => {
    expect(eval_('(cond ((> 1 2) 10) (else 20))')).toBe('20');
  });

  test('cond without else returns nil', () => {
    expect(eval_('(cond ((> 1 2) 10))')).toBe('nil');
  });

  test('cond with multiple false branches', () => {
    const result = eval_(`
      (cond
        ((> 1 2) "first")
        ((> 2 3) "second")
        ((< 1 2) "third")
        (else "fourth"))
    `);
    expect(result).toBe('"third"');
  });
});

describe('Error Handling', () => {
  test('undefined variable throws error', () => {
    expect(() => eval_('undefined_var')).toThrow('Undefined variable: undefined_var');
  });

  test('wrong number of arguments to builtin', () => {
    expect(() => eval_('(< 1)')).toThrow('< expects two arguments');
    expect(() => eval_('(< 1 2 3)')).toThrow('< expects two arguments');
  });

  test('wrong type for arithmetic', () => {
    expect(() => eval_('(+ 1 "two")')).toThrow('+ expects numbers');
  });

  test('wrong type for comparison', () => {
    expect(() => eval_('(< "a" "b")')).toThrow('< expects numbers');
  });

  test('apply non-function', () => {
    expect(() => eval_('(42 1 2)')).toThrow('Cannot apply non-function');
  });

  test('wrong arity for user function', () => {
    expect(() => eval_(`
      (define f (lambda (x y) (+ x y)))
      (f 1)
    `)).toThrow('Expected 2 arguments, got 1');
  });

  test('division - no division by zero check (returns Infinity)', () => {
    // Note: JS semantics - division by zero returns Infinity, not error
    expect(eval_('(/ 1 0)')).toBe('Infinity');
  });

  test('minus with no arguments throws', () => {
    expect(() => eval_('(-)')).toThrow('- expects at least one argument');
  });

  test('division with less than 2 arguments throws', () => {
    expect(() => eval_('(/ 5)')).toThrow('/ expects at least two arguments');
  });

  test('if with wrong number of arguments', () => {
    expect(() => eval_('(if)')).toThrow();
    expect(() => eval_('(if true)')).toThrow();
  });

  test('let with malformed bindings', () => {
    expect(() => eval_('(let (x) x)')).toThrow();
  });

  test('define with wrong arguments', () => {
    expect(() => eval_('(define)')).toThrow();
    expect(() => eval_('(define x)')).toThrow();
  });

  test('lambda with wrong arguments', () => {
    expect(() => eval_('(lambda)')).toThrow();
    expect(() => eval_('(lambda ())')).toThrow();
  });

  test('match with no matching pattern throws', () => {
    expect(() => eval_(`
      (data Color Red Green Blue)
      (match Red (Green 1))
    `)).toThrow('No pattern matched');
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
