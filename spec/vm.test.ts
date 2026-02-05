import { describe, test, expect } from 'bun:test';
import * as Lexer from '../src/core/lexer';
import * as Parser from '../src/core/parser';
import { compileProgram } from '../src/vm/compiler';
import { createVM, interpret, formatVMValue } from '../src/vm/vm';
import { disassemble } from '../src/vm/debug';

function run(code: string): string {
  const tokens = Lexer.lex(Lexer.empty(code));
  const parsed = Parser.parse(Parser.empty(tokens));
  const chunk = compileProgram(parsed.exprs);
  const vm = createVM();
  const result = interpret(vm, chunk);
  // Use the tree-walker's formatValue for consistency
  const { formatValue } = require('../src/core/interpreter');
  return formatValue(result);
}

function runVM(code: string) {
  const tokens = Lexer.lex(Lexer.empty(code));
  const parsed = Parser.parse(Parser.empty(tokens));
  const chunk = compileProgram(parsed.exprs);
  const vm = createVM();
  return interpret(vm, chunk);
}

describe('VM - Literals', () => {
  test('numbers', () => {
    expect(run('42')).toBe('42');
    expect(run('3.14')).toBe('3.14');
    expect(run('0')).toBe('0');
  });

  test('strings', () => {
    expect(run('"hello"')).toBe('"hello"');
  });

  test('booleans', () => {
    expect(run('#t')).toBe('#t');
    expect(run('#f')).toBe('#f');
  });

  test('nil', () => {
    expect(run('nil')).toBe('nil');
  });
});

describe('VM - Arithmetic', () => {
  test('addition', () => {
    expect(run('(+ 1 2)')).toBe('3');
  });

  test('subtraction', () => {
    expect(run('(- 10 3)')).toBe('7');
  });

  test('multiplication', () => {
    expect(run('(* 4 5)')).toBe('20');
  });

  test('division', () => {
    expect(run('(/ 10 2)')).toBe('5');
  });

  test('negate', () => {
    expect(run('(- 5)')).toBe('-5');
  });

  test('nested arithmetic', () => {
    expect(run('(+ (* 2 3) (- 10 4))')).toBe('12');
  });
});

describe('VM - Comparison', () => {
  test('less than', () => {
    expect(run('(< 1 2)')).toBe('#t');
    expect(run('(< 2 1)')).toBe('#f');
  });

  test('greater than', () => {
    expect(run('(> 2 1)')).toBe('#t');
  });

  test('equal', () => {
    expect(run('(= 5 5)')).toBe('#t');
    expect(run('(= 5 6)')).toBe('#f');
  });

  test('less than or equal', () => {
    expect(run('(<= 1 1)')).toBe('#t');
    expect(run('(<= 1 2)')).toBe('#t');
    expect(run('(<= 2 1)')).toBe('#f');
  });

  test('greater than or equal', () => {
    expect(run('(>= 2 2)')).toBe('#t');
  });
});

describe('VM - Control Flow', () => {
  test('if true branch', () => {
    expect(run('(if #t 1 2)')).toBe('1');
  });

  test('if false branch', () => {
    expect(run('(if #f 1 2)')).toBe('2');
  });

  test('if without else', () => {
    expect(run('(if #f 1)')).toBe('nil');
  });

  test('if with complex condition', () => {
    expect(run('(if (< 1 2) "yes" "no")')).toBe('"yes"');
  });

  test('cond', () => {
    expect(run('(cond (#f 1) (#t 2) (else 3))')).toBe('2');
  });

  test('cond else', () => {
    expect(run('(cond (#f 1) (#f 2) (else 3))')).toBe('3');
  });
});

describe('VM - Variables', () => {
  test('define and use global', () => {
    expect(run('(define x 42) x')).toBe('42');
  });

  test('define with expression', () => {
    expect(run('(define x (+ 1 2)) x')).toBe('3');
  });

  test('let binding', () => {
    expect(run('(let ((x 10)) x)')).toBe('10');
  });

  test('let multiple bindings', () => {
    expect(run('(let ((x 10) (y 20)) (+ x y))')).toBe('30');
  });

  test('let sequential bindings', () => {
    expect(run('(let ((x 10) (y (+ x 5))) y)')).toBe('15');
  });

  test('nested let', () => {
    expect(run('(let ((x 1)) (let ((y 2)) (+ x y)))')).toBe('3');
  });
});

describe('VM - Functions', () => {
  test('simple lambda call', () => {
    expect(run('((lambda (x) x) 42)')).toBe('42');
  });

  test('lambda with body', () => {
    expect(run('((lambda (x y) (+ x y)) 3 4)')).toBe('7');
  });

  test('define function and call', () => {
    expect(run('(define add (lambda (a b) (+ a b))) (add 10 20)')).toBe('30');
  });

  test('closure captures variable', () => {
    expect(run('(define make-adder (lambda (n) (lambda (x) (+ n x)))) (define add5 (make-adder 5)) (add5 3)')).toBe('8');
  });

  test('recursive function', () => {
    expect(run(`
      (define fib (lambda (n)
        (if (< n 2)
          n
          (+ (fib (- n 1)) (fib (- n 2))))))
      (fib 10)
    `)).toBe('55');
  });

  test('begin expression', () => {
    expect(run('(begin 1 2 3)')).toBe('3');
  });
});

describe('VM - Tail Call Optimization', () => {
  test('tail recursive function does not overflow', () => {
    const result = run(`
      (define loop (lambda (n acc)
        (if (= n 0)
          acc
          (loop (- n 1) (+ acc 1)))))
      (loop 100000 0)
    `);
    expect(result).toBe('100000');
  });

  test('tail recursive fibonacci', () => {
    const result = run(`
      (define fib-tail (lambda (n a b)
        (if (= n 0)
          a
          (fib-tail (- n 1) b (+ a b)))))
      (fib-tail 30 0 1)
    `);
    expect(result).toBe('832040');
  });
});

describe('VM - ADTs and Pattern Matching', () => {
  test('nullary constructor', () => {
    expect(run('None')).toBe('None');
  });

  test('constructor application', () => {
    expect(run('(Some 42)')).toBe('(Some 42)');
  });

  test('match wildcard', () => {
    expect(run('(match 42 (_ 1))')).toBe('1');
  });

  test('match variable', () => {
    expect(run('(match 42 (x x))')).toBe('42');
  });

  test('match number literal', () => {
    expect(run('(match 1 (1 "one") (2 "two") (_ "other"))')).toBe('"one"');
    expect(run('(match 2 (1 "one") (2 "two") (_ "other"))')).toBe('"two"');
    expect(run('(match 3 (1 "one") (2 "two") (_ "other"))')).toBe('"other"');
  });

  test('match constructor', () => {
    expect(run('(match (Some 42) ((Some x) x) (None 0))')).toBe('42');
    expect(run('(match None ((Some x) x) (None 0))')).toBe('0');
  });

  test('user-defined data type', () => {
    expect(run(`
      (data Color Red Green Blue)
      (match Green
        (Red "red")
        (Green "green")
        (Blue "blue"))
    `)).toBe('"green"');
  });

  test('list operations', () => {
    expect(run('(list 1 2 3)')).toBe('(1 2 3)');
  });
});

describe('VM - Effects', () => {
  test('handle returns body value when no effects performed', () => {
    expect(run(`
      (handle 42
        ((op k) -> 0))
    `)).toBe('42');
  });

  test('handle with return handler transforms result', () => {
    expect(run(`
      (handle 42
        ((return x) -> (+ x 1)))
    `)).toBe('43');
  });

  test('simple perform and handle with resume', () => {
    expect(run(`
      (handle
        (+ 1 (perform get))
        ((get k) -> (resume k 10)))
    `)).toBe('11');
  });

  test('perform with argument', () => {
    expect(run(`
      (handle
        (perform put 42)
        ((put x k) -> x))
    `)).toBe('42');
  });

  test('handler not resuming discards continuation', () => {
    expect(run(`
      (handle
        (begin
          (perform abort)
          100)
        ((abort k) -> 0))
    `)).toBe('0');
  });

  test('multiple performs handled sequentially', () => {
    expect(run(`
      (handle
        (+ (perform get) (perform get))
        ((get k) -> (resume k 5)))
    `)).toBe('10');
  });

  test('perform in function call', () => {
    expect(run(`
      (define double (lambda (x) (* x 2)))
      (handle
        (double (perform get))
        ((get k) -> (resume k 7)))
    `)).toBe('14');
  });

  test('conditional with perform', () => {
    expect(run(`
      (handle
        (if (perform ask)
          100
          0)
        ((ask k) -> (resume k #t)))
    `)).toBe('100');
  });

  test('perform in let binding', () => {
    expect(run(`
      (handle
        (let ((x (perform get)))
          (* x x))
        ((get k) -> (resume k 4)))
    `)).toBe('16');
  });

  test('return handler with perform', () => {
    expect(run(`
      (handle
        (perform get)
        ((get k) -> (resume k 10))
        ((return x) -> (* x 2)))
    `)).toBe('20');
  });

  test('nested handle with different effects', () => {
    expect(run(`
      (handle
        (handle
          (+ (perform get) (perform inc))
          ((get k) -> (resume k 5)))
        ((inc k) -> (resume k 1)))
    `)).toBe('6');
  });

  test('unhandled effect throws error', () => {
    expect(() => run(`
      (handle
        (perform unhandled)
        ((other k) -> 0))
    `)).toThrow('Unhandled effect');
  });

  test('handler with no continuation param still works', () => {
    expect(run(`
      (handle
        (begin (perform log) 42)
        ((log k) -> 0))
    `)).toBe('0');
  });

  test('effect declaration returns nil', () => {
    expect(run(`
      (effect Console
        (print (-> String Unit)))
    `)).toBe('nil');
  });

  test('perform outside handle with default handler works', () => {
    // print has a default handler
    expect(run('(perform print "hello")')).toBe('nil');
  });
});

describe('VM - Disassembly', () => {
  test('disassemble simple program', () => {
    const tokens = Lexer.lex(Lexer.empty('(+ 1 2)'));
    const parsed = Parser.parse(Parser.empty(tokens));
    const chunk = compileProgram(parsed.exprs);
    const output = disassemble(chunk);
    expect(output).toContain('PUSH_CONST');
    expect(output).toContain('ADD');
    expect(output).toContain('RETURN');
  });
});
