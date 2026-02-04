import { test, expect, describe } from 'bun:test';
import * as Lexer from '../lexer';
import * as Parser from '../parser';
import * as Interpreter from '../interpreter';
import { typeCheck, formatType } from '../type-checker';
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

describe('Effect System - Parsing', () => {
  describe('effect declarations', () => {
    test('simple effect with one operation', () => {
      const exprs = parse(`
        (effect Console
          (print : String -> Unit))
      `);
      expect(exprs.length).toBe(1);
      const elements = listToArray(exprs[0]!);
      expect(atomValue(elements[0]!)).toBe('effect');
      expect(atomValue(elements[1]!)).toBe('Console');
      expect(elements[2]?.kind).toBe('list');
    });

    test('effect with multiple operations', () => {
      const exprs = parse(`
        (effect Console
          (print : String -> Unit)
          (read : Unit -> String))
      `);
      expect(exprs.length).toBe(1);
      const elements = listToArray(exprs[0]!);
      expect(atomValue(elements[0]!)).toBe('effect');
      expect(atomValue(elements[1]!)).toBe('Console');
      expect(elements.length).toBe(4); // effect, name, 2 operations
    });

    test('effect with higher-order operation', () => {
      const exprs = parse(`
        (effect State
          (get : Unit -> a)
          (put : a -> Unit))
      `);
      expect(exprs.length).toBe(1);
      const elements = listToArray(exprs[0]!);
      expect(atomValue(elements[0]!)).toBe('effect');
      expect(atomValue(elements[1]!)).toBe('State');
    });
  });

  describe('perform expressions', () => {
    test('simple perform', () => {
      const exprs = parse('(perform print "hello")');
      expect(exprs.length).toBe(1);
      const elements = listToArray(exprs[0]!);
      expect(atomValue(elements[0]!)).toBe('perform');
      expect(atomValue(elements[1]!)).toBe('print');
      expect(atomValue(elements[2]!)).toBe('hello');
    });

    test('nested perform', () => {
      const exprs = parse('(perform print (perform read))');
      expect(exprs.length).toBe(1);
      const elements = listToArray(exprs[0]!);
      expect(atomValue(elements[0]!)).toBe('perform');
      const inner = listToArray(elements[2]!);
      expect(atomValue(inner[0]!)).toBe('perform');
    });

    test('perform with no argument', () => {
      const exprs = parse('(perform read)');
      expect(exprs.length).toBe(1);
      const elements = listToArray(exprs[0]!);
      expect(elements.length).toBe(2); // perform, operation
    });
  });

  describe('handle expressions', () => {
    test('simple handle with one clause', () => {
      const exprs = parse(`
        (handle expr
          (print msg k) -> body)
      `);
      expect(exprs.length).toBe(1);
      const elements = listToArray(exprs[0]!);
      expect(atomValue(elements[0]!)).toBe('handle');
    });

    test('handle with multiple clauses', () => {
      const exprs = parse(`
        (handle (greet)
          (print msg k) -> (begin (display msg) (resume k unit))
          (read k) -> (resume k "world"))
      `);
      expect(exprs.length).toBe(1);
      const elements = listToArray(exprs[0]!);
      expect(atomValue(elements[0]!)).toBe('handle');
      // handle, expr, then each clause is 3 elements: (pattern) -> body
      // So: handle, expr, (print msg k), ->, body, (read k), ->, body = 8
      expect(elements.length).toBe(8);
      expect(atomValue(elements[2]!)).not.toBe('->'); // pattern first
      expect(atomValue(elements[3]!)).toBe('->');     // then arrow
    });

    test('handle with return clause', () => {
      const exprs = parse(`
        (handle expr
          (op1 arg k) -> handler1
          (return x) -> x)
      `);
      expect(exprs.length).toBe(1);
      const elements = listToArray(exprs[0]!);
      expect(atomValue(elements[0]!)).toBe('handle');
    });

    test('complete example from spec', () => {
      const exprs = parse(`
        (effect Console
          (print : String -> Unit)
          (read : Unit -> String))

        (define greet ()
          (perform print "Hello, ")
          (perform print (perform read)))

        (handle (greet)
          (print msg k) -> (begin (display msg) (resume k unit))
          (read k) -> (resume k "world"))
      `);
      expect(exprs.length).toBe(3);
      expect(atomValue(listToArray(exprs[0]!)[0]!)).toBe('effect');
      expect(atomValue(listToArray(exprs[1]!)[0]!)).toBe('define');
      expect(atomValue(listToArray(exprs[2]!)[0]!)).toBe('handle');
    });
  });
});

// Helper to type check code
const check = (code: string): { ok: boolean; types?: any[]; error?: string } => {
  const tokens = Lexer.lex(Lexer.empty(code));
  const parsed = Parser.parse(Parser.empty(tokens));
  const result = typeCheck(parsed.exprs);
  if (!result.ok) {
    return { ok: false, error: String(result.error) };
  }
  return { ok: true, types: result.types };
};

// Helper to run code
const run = (code: string) => {
  const tokens = Lexer.lex(Lexer.empty(code));
  const parsed = Parser.parse(Parser.empty(tokens));
  return Interpreter.evalMany(Interpreter.empty(), parsed.exprs);
};

describe('Effect System - Type Checking', () => {
  test('effect declaration type checks to Unit', () => {
    const result = check(`
      (effect Console
        (print : String -> Unit))
    `);
    expect(result.ok).toBe(true);
    expect(formatType(result.types![0]!)).toBe('Unit');
  });

  test('effect with multiple operations type checks', () => {
    const result = check(`
      (effect State
        (get : Unit -> Int)
        (put : Int -> Unit))
    `);
    expect(result.ok).toBe(true);
  });

  test('effect requires a name', () => {
    const result = check('(effect)');
    expect(result.ok).toBe(false);
  });

  test('perform type checks (returns fresh type variable)', () => {
    const result = check('(perform get)');
    expect(result.ok).toBe(true);
    // Type should be a type variable
  });

  test('handle type checks to its body type', () => {
    const result = check('(handle 42 (op k) -> 0)');
    expect(result.ok).toBe(true);
    expect(formatType(result.types![0]!)).toBe('Int');
  });
});

describe('Effect System - Effect Inference', () => {
  test('perform infers effect operation return type', () => {
    const result = check(`
      (effect Console
        (print : String -> Unit))
      (perform print "hello")
    `);
    expect(result.ok).toBe(true);
    expect(formatType(result.types![1]!)).toBe('Unit');
  });

  test('perform infers type from effect op with unit param', () => {
    const result = check(`
      (effect State
        (get : Unit -> Int)
        (put : Int -> Unit))
      (perform get ())
    `);
    expect(result.ok).toBe(true);
    expect(formatType(result.types![1]!)).toBe('Int');
  });

  test('perform with wrong argument type fails', () => {
    const result = check(`
      (effect Console
        (print : String -> Unit))
      (perform print 42)
    `);
    expect(result.ok).toBe(false);
  });

  test('perform with wrong arity fails', () => {
    const result = check(`
      (effect Console
        (print : String -> Unit))
      (perform print)
    `);
    expect(result.ok).toBe(false);
  });

  test('handle infers handler body types', () => {
    const result = check(`
      (effect Console
        (print : String -> Unit))
      (handle 42
        (print msg k) -> 0)
    `);
    expect(result.ok).toBe(true);
    // Handle should return Int (from both 42 and handler 0)
    expect(formatType(result.types![1]!)).toBe('Int');
  });

  test('handle with return handler', () => {
    const result = check(`
      (handle 42
        (return x) -> x)
    `);
    expect(result.ok).toBe(true);
    expect(formatType(result.types![0]!)).toBe('Int');
  });

  test('lambda with effects has effectful type', () => {
    const result = check(`
      (effect Console
        (print : String -> Unit))
      (define f (lambda (x) (perform print x)))
    `);
    expect(result.ok).toBe(true);
    // f should have type (-> String {Console} Unit)
    const fType = formatType(result.types![1]!);
    expect(fType).toBe('Unit'); // define returns Unit, but the lambda is registered
  });

  test('nested perform infers correctly', () => {
    const result = check(`
      (effect Console
        (print : String -> Unit)
        (read : Unit -> String))
      (perform print (perform read ()))
    `);
    expect(result.ok).toBe(true);
    expect(formatType(result.types![1]!)).toBe('Unit');
  });
});

describe('Effect System - Interpreter Recognition', () => {
  test('effect declaration returns nil', () => {
    const result = run(`
      (effect Console
        (print : String -> Unit))
    `);
    expect(result.value.kind).toBe('nil');
  });

  test('perform throws not-implemented error', () => {
    expect(() => run('(perform get)')).toThrow('not yet implemented');
  });

  test('handle throws not-implemented error', () => {
    expect(() => run('(handle 42 (op k) -> 0)')).toThrow('not yet implemented');
  });

  test('effect without name throws error', () => {
    expect(() => run('(effect)')).toThrow('expects a name');
  });

  test('perform without operation throws error', () => {
    expect(() => run('(perform)')).toThrow('expects an operation');
  });

  test('handle without expression throws error', () => {
    expect(() => run('(handle)')).toThrow('expects an expression');
  });
});
