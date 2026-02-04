import { test, expect, describe } from 'bun:test';
import * as Lexer from '../lexer';
import * as Parser from '../parser';
import * as Interpreter from '../interpreter';
import { typeCheck, formatError, formatType } from '../type-checker';
import type { Type } from '../type-ast';

// Helper to parse and type check code
const check = (code: string): { ok: boolean; types?: Type[]; error?: string } => {
  const tokens = Lexer.lex(Lexer.empty(code));
  const parsed = Parser.parse(Parser.empty(tokens));
  const result = typeCheck(parsed.exprs);

  if (!result.ok) {
    return { ok: false, error: formatError(result.error) };
  }
  return { ok: true, types: result.types };
};

// Helper to parse, type check, and evaluate code
const run = (code: string): string => {
  const tokens = Lexer.lex(Lexer.empty(code));
  const parsed = Parser.parse(Parser.empty(tokens));
  const result = Interpreter.evalMany(Interpreter.empty(), parsed.exprs);
  return Interpreter.formatValue(result.value);
};

describe('Type Checker - Basic Types', () => {
  test('integer literals', () => {
    const result = check('42');
    expect(result.ok).toBe(true);
    expect(formatType(result.types![0]!)).toBe('Int');
  });

  test('float literals', () => {
    const result = check('3.14');
    expect(result.ok).toBe(true);
    expect(formatType(result.types![0]!)).toBe('Float');
  });

  test('string literals', () => {
    const result = check('"hello"');
    expect(result.ok).toBe(true);
    expect(formatType(result.types![0]!)).toBe('String');
  });

  test('boolean literals', () => {
    const result = check('#t');
    expect(result.ok).toBe(true);
    expect(formatType(result.types![0]!)).toBe('Bool');
  });
});

describe('Type Checker - Arithmetic', () => {
  test('addition of integers', () => {
    const result = check('(+ 1 2)');
    expect(result.ok).toBe(true);
    expect(formatType(result.types![0]!)).toBe('Int');
  });

  test('addition of floats', () => {
    const result = check('(+ 1.0 2.0)');
    expect(result.ok).toBe(true);
    expect(formatType(result.types![0]!)).toBe('Float');
  });

  test('comparison returns Bool', () => {
    const result = check('(< 1 2)');
    expect(result.ok).toBe(true);
    expect(formatType(result.types![0]!)).toBe('Bool');
  });
});

describe('Type Checker - Type Annotations', () => {
  test('annotated define with Int', () => {
    const result = check('(define (x : Int) 42)');
    expect(result.ok).toBe(true);
  });

  test('annotated define with Float', () => {
    const result = check('(define (y : Float) 3.14)');
    expect(result.ok).toBe(true);
  });

  test('type mismatch in annotated define', () => {
    const result = check('(define (x : Int) "hello")');
    expect(result.ok).toBe(false);
    expect(result.error).toContain('cannot unify');
  });

  test('annotated lambda parameter', () => {
    const result = check('(lambda ((x : Int)) x)');
    expect(result.ok).toBe(true);
  });

  test('function type annotation', () => {
    const result = check('(define (add : (-> Int Int Int)) (lambda (a b) (+ a b)))');
    expect(result.ok).toBe(true);
  });
});

describe('Type Checker - Functions', () => {
  test('identity function is polymorphic', () => {
    const result = check('(define id (lambda (x) x))');
    expect(result.ok).toBe(true);
  });

  test('function application', () => {
    const code = `
      (define (add : (-> Int Int Int)) (lambda (a b) (+ a b)))
      (add 1 2)
    `;
    const result = check(code);
    expect(result.ok).toBe(true);
    expect(formatType(result.types![result.types!.length - 1]!)).toBe('Int');
  });
});

describe('Type Checker - If Expressions', () => {
  test('if branches must have same type', () => {
    const result = check('(if #t 1 2)');
    expect(result.ok).toBe(true);
    expect(formatType(result.types![0]!)).toBe('Int');
  });

  test('if condition must be Bool', () => {
    const result = check('(if 1 2 3)');
    expect(result.ok).toBe(false);
    expect(result.error).toContain('cannot unify');
  });

  test('if branches type mismatch', () => {
    const result = check('(if #t 1 "hello")');
    expect(result.ok).toBe(false);
    expect(result.error).toContain('cannot unify');
  });
});

describe('Type Checker - Let Expressions', () => {
  test('basic let binding', () => {
    const result = check('(let ((x 42)) x)');
    expect(result.ok).toBe(true);
    expect(formatType(result.types![0]!)).toBe('Int');
  });

  test('let with type annotation', () => {
    const result = check('(let (((x : Int) 42)) x)');
    expect(result.ok).toBe(true);
    expect(formatType(result.types![0]!)).toBe('Int');
  });

  test('let generalization', () => {
    const code = `
      (let ((id (lambda (x) x)))
        (begin (id 1) (id "hello")))
    `;
    const result = check(code);
    expect(result.ok).toBe(true);
  });
});

describe('Algebraic Data Types', () => {
  test('data definition creates constructors', () => {
    const code = `
      (data (Option a) None (Some a))
      None
    `;
    const result = run(code);
    expect(result).toBe('None');
  });

  test('constructor with arguments', () => {
    const code = `
      (data (Option a) None (Some a))
      (Some 42)
    `;
    const result = run(code);
    expect(result).toBe('(Some 42)');
  });

  test('list data type', () => {
    const code = `
      (data (List a) Nil (Cons a (List a)))
      (Cons 1 (Cons 2 Nil))
    `;
    const result = run(code);
    expect(result).toBe('(Cons 1 (Cons 2 Nil))');
  });
});

describe('Pattern Matching', () => {
  test('match on Option None', () => {
    const code = `
      (data (Option a) None (Some a))
      (match None
        (None 0)
        ((Some v) v))
    `;
    const result = run(code);
    expect(result).toBe('0');
  });

  test('match on Option Some', () => {
    const code = `
      (data (Option a) None (Some a))
      (match (Some 42)
        (None 0)
        ((Some v) v))
    `;
    const result = run(code);
    expect(result).toBe('42');
  });

  test('match with wildcard', () => {
    const code = `
      (data (Option a) None (Some a))
      (match (Some 42)
        (_ 99))
    `;
    const result = run(code);
    expect(result).toBe('99');
  });

  test('match on list', () => {
    const code = `
      (data (List a) Nil (Cons a (List a)))
      (define head
        (lambda (lst)
          (match lst
            (Nil 0)
            ((Cons h t) h))))
      (head (Cons 42 Nil))
    `;
    const result = run(code);
    expect(result).toBe('42');
  });

  test('recursive pattern matching', () => {
    const code = `
      (data (List a) Nil (Cons a (List a)))
      (define length
        (lambda (lst)
          (match lst
            (Nil 0)
            ((Cons h t) (+ 1 (length t))))))
      (length (Cons 1 (Cons 2 (Cons 3 Nil))))
    `;
    const result = run(code);
    expect(result).toBe('3');
  });

  test('nested pattern matching', () => {
    const code = `
      (data (Option a) None (Some a))
      (data (Pair a b) (MkPair a b))
      (match (MkPair (Some 1) (Some 2))
        ((MkPair (Some x) (Some y)) (+ x y))
        (_ 0))
    `;
    const result = run(code);
    expect(result).toBe('3');
  });
});

describe('Type Checker - ADTs', () => {
  test('Option type inference', () => {
    const code = `
      (data (Option a) None (Some a))
      (Some 42)
    `;
    const result = check(code);
    expect(result.ok).toBe(true);
  });

  test('match expression type inference', () => {
    const code = `
      (data (Option a) None (Some a))
      (define unwrap
        (lambda (opt)
          (match opt
            (None 0)
            ((Some v) v))))
      (unwrap (Some 42))
    `;
    const result = check(code);
    expect(result.ok).toBe(true);
    expect(formatType(result.types![result.types!.length - 1]!)).toBe('Int');
  });
});

describe('Typeclasses', () => {
  test('Num constraint on +', () => {
    const code = `
      (define double (lambda (x) (+ x x)))
      (double 5)
    `;
    const result = check(code);
    expect(result.ok).toBe(true);
    expect(formatType(result.types![result.types!.length - 1]!)).toBe('Int');
  });

  test('Num constraint with Float', () => {
    const code = `
      (define double (lambda (x) (+ x x)))
      (double 2.5)
    `;
    const result = check(code);
    expect(result.ok).toBe(true);
    expect(formatType(result.types![result.types!.length - 1]!)).toBe('Float');
  });
});

describe('Recursive Types (μ-types)', () => {
  test('self-application omega', () => {
    const code = `(define omega (lambda (x) (x x)))`;
    const result = check(code);
    expect(result.ok).toBe(true);
  });

  test('Y-combinator loop pattern', () => {
    const code = `
      (define loop (lambda (f) (if #t nil (f f))))
      (loop loop)
    `;
    const result = check(code);
    expect(result.ok).toBe(true);
  });

  test('self-application with operation', () => {
    const code = `
      (define selfapp (lambda (f) (f f)))
    `;
    const result = check(code);
    expect(result.ok).toBe(true);
  });

  test('mu-type annotation parsing', () => {
    const code = `
      (define (f : (mu a (-> a Int))) (lambda (x) 42))
    `;
    const result = check(code);
    expect(result.ok).toBe(true);
  });
});

describe('Integration Tests', () => {
  test('factorial with type annotation', () => {
    const code = `
      (define (factorial : (-> Int Int))
        (lambda (n)
          (if (= n 0)
            1
            (* n (factorial (- n 1))))))
      (factorial 5)
    `;
    const checkResult = check(code);
    expect(checkResult.ok).toBe(true);

    const runResult = run(code);
    expect(runResult).toBe('120');
  });

  test('map function on ADT list', () => {
    const code = `
      (data (List a) Nil (Cons a (List a)))

      (define map
        (lambda (f lst)
          (match lst
            (Nil Nil)
            ((Cons h t) (Cons (f h) (map f t))))))

      (define double (lambda (x) (+ x x)))

      (map double (Cons 1 (Cons 2 (Cons 3 Nil))))
    `;
    const runResult = run(code);
    expect(runResult).toBe('(Cons 2 (Cons 4 (Cons 6 Nil)))');
  });

  test('fold function on ADT list', () => {
    const code = `
      (data (List a) Nil (Cons a (List a)))

      (define foldl
        (lambda (f acc lst)
          (match lst
            (Nil acc)
            ((Cons h t) (foldl f (f acc h) t)))))

      (foldl + 0 (Cons 1 (Cons 2 (Cons 3 Nil))))
    `;
    const runResult = run(code);
    expect(runResult).toBe('6');
  });
});
