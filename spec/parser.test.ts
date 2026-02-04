import { test, expect, describe } from 'bun:test';
import * as Lexer from '../lexer';
import * as Parser from '../parser';
import type { Expr } from '../types';

// Helper to parse code
const parse = (code: string): Expr[] => {
  const tokens = Lexer.lex(Lexer.empty(code));
  const result = Parser.parse(Parser.empty(tokens));
  return result.exprs;
};

// Helper to get atom value
const atomValue = (expr: Expr): string | number | undefined => {
  if (expr.kind === 'atom') {
    if (expr.value.kind === 'symbol') return expr.value.value;
    if (expr.value.kind === 'number') return expr.value.value;
    if (expr.value.kind === 'string') return expr.value.value;
  }
  return undefined;
};

// Helper to convert expr to array (for list exprs)
const listToArray = (expr: Expr): Expr[] => {
  const result: Expr[] = [];
  let current = expr;
  while (current.kind === 'list') {
    result.push(current.head);
    current = current.tail;
  }
  return result;
};

describe('Parser - Atoms', () => {
  test('number atom', () => {
    const exprs = parse('42');
    expect(exprs.length).toBe(1);
    expect(exprs[0]?.kind).toBe('atom');
    expect(atomValue(exprs[0]!)).toBe(42);
  });

  test('float atom', () => {
    const exprs = parse('3.14');
    expect(exprs.length).toBe(1);
    expect(atomValue(exprs[0]!)).toBe(3.14);
  });

  test('symbol atom', () => {
    const exprs = parse('foo');
    expect(exprs.length).toBe(1);
    expect(exprs[0]?.kind).toBe('atom');
    expect(atomValue(exprs[0]!)).toBe('foo');
  });

  test('string atom', () => {
    const exprs = parse('"hello"');
    expect(exprs.length).toBe(1);
    expect(atomValue(exprs[0]!)).toBe('hello');
  });

  test('multiple atoms', () => {
    const exprs = parse('1 2 3');
    expect(exprs.length).toBe(3);
  });
});

describe('Parser - Lists', () => {
  test('empty list', () => {
    const exprs = parse('()');
    expect(exprs.length).toBe(1);
    expect(exprs[0]?.kind).toBe('nil');
  });

  test('simple list', () => {
    const exprs = parse('(a b c)');
    expect(exprs.length).toBe(1);
    expect(exprs[0]?.kind).toBe('list');
    const elements = listToArray(exprs[0]!);
    expect(elements.length).toBe(3);
    expect(atomValue(elements[0]!)).toBe('a');
    expect(atomValue(elements[1]!)).toBe('b');
    expect(atomValue(elements[2]!)).toBe('c');
  });

  test('nested list', () => {
    const exprs = parse('(a (b c) d)');
    expect(exprs.length).toBe(1);
    const outer = listToArray(exprs[0]!);
    expect(outer.length).toBe(3);
    expect(outer[1]?.kind).toBe('list');
    const inner = listToArray(outer[1]!);
    expect(inner.length).toBe(2);
  });

  test('deeply nested list', () => {
    const exprs = parse('(((a)))');
    expect(exprs.length).toBe(1);
    const l1 = listToArray(exprs[0]!);
    expect(l1.length).toBe(1);
    const l2 = listToArray(l1[0]!);
    expect(l2.length).toBe(1);
    const l3 = listToArray(l2[0]!);
    expect(l3.length).toBe(1);
    expect(atomValue(l3[0]!)).toBe('a');
  });

  test('list with numbers', () => {
    const exprs = parse('(1 2 3)');
    const elements = listToArray(exprs[0]!);
    expect(elements.length).toBe(3);
    expect(atomValue(elements[0]!)).toBe(1);
    expect(atomValue(elements[1]!)).toBe(2);
    expect(atomValue(elements[2]!)).toBe(3);
  });

  test('mixed list', () => {
    const exprs = parse('(foo 42 "bar")');
    const elements = listToArray(exprs[0]!);
    expect(elements.length).toBe(3);
    expect(atomValue(elements[0]!)).toBe('foo');
    expect(atomValue(elements[1]!)).toBe(42);
    expect(atomValue(elements[2]!)).toBe('bar');
  });
});

describe('Parser - S-expressions', () => {
  test('function call syntax', () => {
    const exprs = parse('(+ 1 2)');
    expect(exprs.length).toBe(1);
    const elements = listToArray(exprs[0]!);
    expect(elements.length).toBe(3);
    expect(atomValue(elements[0]!)).toBe('+');
  });

  test('define expression', () => {
    const exprs = parse('(define x 42)');
    const elements = listToArray(exprs[0]!);
    expect(atomValue(elements[0]!)).toBe('define');
    expect(atomValue(elements[1]!)).toBe('x');
    expect(atomValue(elements[2]!)).toBe(42);
  });

  test('lambda expression', () => {
    const exprs = parse('(lambda (x) x)');
    const elements = listToArray(exprs[0]!);
    expect(atomValue(elements[0]!)).toBe('lambda');
    expect(elements[1]?.kind).toBe('list');
  });

  test('if expression', () => {
    const exprs = parse('(if cond then else)');
    const elements = listToArray(exprs[0]!);
    expect(elements.length).toBe(4);
    expect(atomValue(elements[0]!)).toBe('if');
  });

  test('let expression', () => {
    const exprs = parse('(let ((x 1)) x)');
    const elements = listToArray(exprs[0]!);
    expect(atomValue(elements[0]!)).toBe('let');
  });
});

describe('Parser - Error Handling', () => {
  test('unbalanced parens - missing close', () => {
    expect(() => parse('(+ 1 2')).toThrow();
  });

  test('unbalanced parens - extra close', () => {
    expect(() => parse('(+ 1 2))')).toThrow();
  });

  test('unbalanced parens - nested missing close', () => {
    expect(() => parse('(a (b c)')).toThrow();
  });

  test('unexpected close paren at start', () => {
    expect(() => parse(')')).toThrow();
  });

  test('unexpected close paren in middle', () => {
    expect(() => parse('(a ) b)')).toThrow();
  });
});

describe('Parser - Edge Cases', () => {
  test('empty input', () => {
    const exprs = parse('');
    expect(exprs.length).toBe(0);
  });

  test('only whitespace', () => {
    const exprs = parse('   \n\t  ');
    expect(exprs.length).toBe(0);
  });

  test('only comments', () => {
    const exprs = parse('; just a comment');
    expect(exprs.length).toBe(0);
  });

  test('expression after comment', () => {
    const exprs = parse('; comment\n42');
    expect(exprs.length).toBe(1);
    expect(atomValue(exprs[0]!)).toBe(42);
  });

  test('multiple top-level expressions', () => {
    const exprs = parse('(define a 1) (define b 2) (+ a b)');
    expect(exprs.length).toBe(3);
  });

  test('multiline expression', () => {
    const exprs = parse(`
      (define factorial
        (lambda (n)
          (if (= n 0)
            1
            (* n (factorial (- n 1))))))
    `);
    expect(exprs.length).toBe(1);
  });
});

describe('Parser - Source Locations', () => {
  test('atom has location', () => {
    const exprs = parse('foo');
    expect(exprs[0]?.loc).toBeDefined();
    expect(exprs[0]?.loc?.start.line).toBe(1);
    expect(exprs[0]?.loc?.start.column).toBe(1);
  });

  test('list has location', () => {
    const exprs = parse('(a b)');
    expect(exprs[0]?.loc).toBeDefined();
    expect(exprs[0]?.loc?.start.line).toBe(1);
  });

  test('nested expr has location', () => {
    const exprs = parse('(a (b c))');
    const elements = listToArray(exprs[0]!);
    expect(elements[1]?.loc).toBeDefined();
  });
});

describe('Parser - Complex Expressions', () => {
  test('data definition', () => {
    const exprs = parse('(data (Option a) None (Some a))');
    expect(exprs.length).toBe(1);
    const elements = listToArray(exprs[0]!);
    expect(atomValue(elements[0]!)).toBe('data');
  });

  test('match expression', () => {
    const exprs = parse(`
      (match x
        (None 0)
        ((Some v) v))
    `);
    expect(exprs.length).toBe(1);
    const elements = listToArray(exprs[0]!);
    expect(atomValue(elements[0]!)).toBe('match');
  });

  test('type annotated define', () => {
    const exprs = parse('(define (x : Int) 42)');
    expect(exprs.length).toBe(1);
    const elements = listToArray(exprs[0]!);
    expect(atomValue(elements[0]!)).toBe('define');
    // Second element should be a list (x : Int)
    expect(elements[1]?.kind).toBe('list');
  });

  test('cond expression', () => {
    const exprs = parse(`
      (cond
        ((< x 0) "negative")
        ((= x 0) "zero")
        (else "positive"))
    `);
    expect(exprs.length).toBe(1);
    const elements = listToArray(exprs[0]!);
    expect(atomValue(elements[0]!)).toBe('cond');
    expect(elements.length).toBe(4); // cond + 3 clauses
  });
});
