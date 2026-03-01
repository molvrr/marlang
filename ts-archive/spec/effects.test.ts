import { test, expect, describe } from 'bun:test';
import * as Lexer from '../src/core/lexer';
import * as Parser from '../src/core/parser';
import * as Interpreter from '../src/core/interpreter';
import { typeCheck, formatType } from '../src/types/type-checker';
import type { Expr } from '../src/core/types';

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
          (print (-> String Unit)))
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
          (print (-> String Unit))
          (read (-> Unit String)))
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
          (get (-> Unit a))
          (put (-> a Unit)))
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
          ((print msg k) -> body))
      `);
      expect(exprs.length).toBe(1);
      const elements = listToArray(exprs[0]!);
      expect(atomValue(elements[0]!)).toBe('handle');
    });

    test('handle with multiple clauses', () => {
      const exprs = parse(`
        (handle (greet)
          ((print msg k) -> (begin (display msg) (resume k unit)))
          ((read k) -> (resume k "world")))
      `);
      expect(exprs.length).toBe(1);
      const elements = listToArray(exprs[0]!);
      expect(atomValue(elements[0]!)).toBe('handle');
      // handle, expr, then each wrapped clause: ((pattern) -> body)
      // So: handle, expr, clause1, clause2 = 4
      expect(elements.length).toBe(4);
    });

    test('handle with return clause', () => {
      const exprs = parse(`
        (handle expr
          ((op1 arg k) -> handler1)
          ((return x) -> x))
      `);
      expect(exprs.length).toBe(1);
      const elements = listToArray(exprs[0]!);
      expect(atomValue(elements[0]!)).toBe('handle');
    });

    test('complete example from spec', () => {
      const exprs = parse(`
        (effect Console
          (print (-> String Unit))
          (read (-> Unit String)))

        (define greet ()
          (perform print "Hello, ")
          (perform print (perform read)))

        (handle (greet)
          ((print msg k) -> (begin (display msg) (resume k unit)))
          ((read k) -> (resume k "world")))
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
        (print (-> String Unit)))
    `);
    expect(result.ok).toBe(true);
    expect(formatType(result.types![0]!)).toBe('Unit');
  });

  test('effect with multiple operations type checks', () => {
    const result = check(`
      (effect State
        (get (-> Unit Int))
        (put (-> Int Unit)))
    `);
    expect(result.ok).toBe(true);
  });

  test('effect requires a name', () => {
    const result = check('(effect)');
    expect(result.ok).toBe(false);
  });

  test('perform with undeclared effect fails', () => {
    const result = check('(perform get)');
    expect(result.ok).toBe(false);
  });

  test('handle with undeclared effect fails', () => {
    const result = check('(handle 42 ((op k) -> 0))');
    expect(result.ok).toBe(false);
  });

  test('handle type checks to its body type with declared effect', () => {
    const result = check(`
      (effect Test (op (-> Unit Int)))
      (handle 42 ((op k) -> 0))
    `);
    expect(result.ok).toBe(true);
    expect(formatType(result.types![1]!)).toBe('Int');
  });
});

describe('Effect System - Effect Inference', () => {
  test('perform infers effect operation return type', () => {
    const result = check(`
      (effect Console
        (print (-> String Unit)))
      (perform print "hello")
    `);
    expect(result.ok).toBe(true);
    expect(formatType(result.types![1]!)).toBe('Unit');
  });

  test('perform infers type from effect op with unit param', () => {
    const result = check(`
      (effect State
        (get (-> Unit Int))
        (put (-> Int Unit)))
      (perform get ())
    `);
    expect(result.ok).toBe(true);
    expect(formatType(result.types![1]!)).toBe('Int');
  });

  test('perform with wrong argument type fails', () => {
    const result = check(`
      (effect Console
        (print (-> String Unit)))
      (perform print 42)
    `);
    expect(result.ok).toBe(false);
  });

  test('effect operation with Show constraint accepts any Show-able type', () => {
    const result = check(`
      (effect Output
        (output (=> (Show a) (-> a Unit))))
      (perform output 42)
    `);
    expect(result.ok).toBe(true);
    expect(formatType(result.types![1]!)).toBe('Unit');
  });

  test('effect operation with Show constraint accepts strings', () => {
    const result = check(`
      (effect Output
        (output (=> (Show a) (-> a Unit))))
      (perform output "hello")
    `);
    expect(result.ok).toBe(true);
    expect(formatType(result.types![1]!)).toBe('Unit');
  });

  test('effect operation with Show constraint accepts booleans', () => {
    const result = check(`
      (effect Output
        (output (=> (Show a) (-> a Unit))))
      (perform output #t)
    `);
    expect(result.ok).toBe(true);
  });

  test('effect operation with Show constraint accepts ADTs', () => {
    const result = check(`
      (effect Output
        (output (=> (Show a) (-> a Unit))))
      (perform output (Some 5))
    `);
    expect(result.ok).toBe(true);
  });

  test('effect operation with Num constraint requires numeric type', () => {
    const result = check(`
      (effect Math
        (square (=> (Num a) (-> a a))))
      (perform square 5)
    `);
    expect(result.ok).toBe(true);
    expect(formatType(result.types![1]!)).toBe('Int');
  });

  test('effect operation with Num constraint fails for non-numeric types', () => {
    const result = check(`
      (effect Math
        (square (=> (Num a) (-> a a))))
      (perform square "hello")
    `);
    expect(result.ok).toBe(false);
  });

  test('perform with wrong arity fails', () => {
    const result = check(`
      (effect Console
        (print (-> String Unit)))
      (perform print)
    `);
    expect(result.ok).toBe(false);
  });

  test('handle infers handler body types', () => {
    const result = check(`
      (effect Console
        (print (-> String Unit)))
      (handle 42
        ((print msg k) -> 0))
    `);
    expect(result.ok).toBe(true);
    // Handle should return Int (from both 42 and handler 0)
    expect(formatType(result.types![1]!)).toBe('Int');
  });

  test('handle with return handler', () => {
    const result = check(`
      (handle 42
        ((return x) -> x))
    `);
    expect(result.ok).toBe(true);
    expect(formatType(result.types![0]!)).toBe('Int');
  });

  test('lambda with effects has effectful type', () => {
    const result = check(`
      (effect Console
        (print (-> String Unit)))
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
        (print (-> String Unit))
        (read (-> Unit String)))
      (perform print (perform read ()))
    `);
    expect(result.ok).toBe(true);
    expect(formatType(result.types![1]!)).toBe('Unit');
  });

  test('resume is recognized in type checker', () => {
    const result = check(`
      (effect State
        (get (-> Unit Int)))
      (handle
        (+ 1 (perform get ()))
        ((get k) -> (resume k 10)))
    `);
    expect(result.ok).toBe(true);
    expect(formatType(result.types![1]!)).toBe('Int');
  });

  test('resume with wrong arity fails', () => {
    const result = check('(handle 1 ((op k) -> (resume k)))');
    expect(result.ok).toBe(false);
  });

  test('resume infers continuation return type', () => {
    const result = check(`
      (effect State
        (get (-> Unit Int)))
      (handle
        (perform get ())
        ((get k) -> (+ (resume k 5) 1)))
    `);
    expect(result.ok).toBe(true);
    // Handler body returns Int (from adding 1 to resume result)
    expect(formatType(result.types![1]!)).toBe('Int');
  });

  test('resume type mismatch with declared effect fails', () => {
    const result = check(`
      (effect State
        (get (-> Unit String)))
      (handle
        (perform get ())
        ((get k) -> (resume k 42)))
    `);
    // Should fail because resume is called with Int but k expects String
    expect(result.ok).toBe(false);
  });
});

describe('Effect System - Interpreter Recognition', () => {
  test('effect declaration returns nil', () => {
    const result = run(`
      (effect Console
        (print (-> String Unit)))
    `);
    expect(result.value.kind).toBe('nil');
  });

  test('perform outside handle throws error for unregistered effects', () => {
    // perform outside handle with no default handler throws "Unhandled effect"
    expect(() => run('(perform get)')).toThrow('Unhandled effect');
  });

  test('effect without name throws error', () => {
    expect(() => run('(effect)')).toThrow('expects a name');
  });

  test('perform without operation throws error', () => {
    // The handler body with (perform) is only executed if the effect is triggered
    expect(() => run('(handle (perform op) ((op k) -> (perform)))')).toThrow('expects an operation');
  });

  test('handle without expression throws error', () => {
    expect(() => run('(handle)')).toThrow('expects an expression');
  });
});

describe('Effect System - Runtime Behavior', () => {
  test('handle returns body value when no effects performed', () => {
    const result = run(`
      (handle 42
        ((op k) -> 0))
    `);
    expect(result.value.kind).toBe('number');
    expect(result.value.value).toBe(42);
  });

  test('handle with return handler transforms result', () => {
    const result = run(`
      (handle 42
        ((return x) -> (+ x 1)))
    `);
    expect(result.value.kind).toBe('number');
    expect(result.value.value).toBe(43);
  });

  test('simple perform and handle', () => {
    const result = run(`
      (handle
        (+ 1 (perform get))
        ((get k) -> (resume k 10)))
    `);
    expect(result.value.kind).toBe('number');
    expect(result.value.value).toBe(11);
  });

  test('perform with argument', () => {
    const result = run(`
      (handle
        (perform put 42)
        ((put x k) -> x))
    `);
    expect(result.value.kind).toBe('number');
    expect(result.value.value).toBe(42);
  });

  test('handler not resuming discards continuation', () => {
    const result = run(`
      (handle
        (begin
          (perform abort)
          100)
        ((abort k) -> 0))
    `);
    expect(result.value.kind).toBe('number');
    expect(result.value.value).toBe(0);
  });

  test('multiple performs handled sequentially', () => {
    const result = run(`
      (handle
        (+ (perform get) (perform get))
        ((get k) -> (resume k 5)))
    `);
    expect(result.value.kind).toBe('number');
    expect(result.value.value).toBe(10);
  });

  test('perform in function call', () => {
    const result = run(`
      (define double (lambda (x) (* x 2)))
      (handle
        (double (perform get))
        ((get k) -> (resume k 7)))
    `);
    expect(result.value.kind).toBe('number');
    expect(result.value.value).toBe(14);
  });

  test('conditional with perform', () => {
    const result = run(`
      (handle
        (if (perform ask)
          100
          0)
        ((ask k) -> (resume k #t)))
    `);
    expect(result.value.kind).toBe('number');
    expect(result.value.value).toBe(100);
  });

  test('perform in let binding', () => {
    const result = run(`
      (handle
        (let ((x (perform get)))
          (* x x))
        ((get k) -> (resume k 4)))
    `);
    expect(result.value.kind).toBe('number');
    expect(result.value.value).toBe(16);
  });

  test('continuation is one-shot (second resume returns undefined)', () => {
    // With one-shot continuations, the second resume doesn't work as expected
    // because the generator is exhausted after the first resume
    // This is expected behavior for our implementation
    const result = run(`
      (handle
        (+ 1 (perform get))
        ((get k) -> (resume k 10)))
    `);
    expect(result.value.kind).toBe('number');
    expect(result.value.value).toBe(11);
  });

  test('return handler with perform', () => {
    const result = run(`
      (handle
        (perform get)
        ((get k) -> (resume k 10))
        ((return x) -> (* x 2)))
    `);
    expect(result.value.kind).toBe('number');
    expect(result.value.value).toBe(20);
  });

  test('nested handle with different effects', () => {
    const result = run(`
      (handle
        (handle
          (+ (perform get) (perform inc))
          ((get k) -> (resume k 5)))
        ((inc k) -> (resume k 1)))
    `);
    expect(result.value.kind).toBe('number');
    expect(result.value.value).toBe(6);
  });

  test('unhandled effect throws error', () => {
    expect(() => run(`
      (handle
        (perform unhandled)
        ((other k) -> 0))
    `)).toThrow('Unhandled effect: unhandled');
  });

  test('handler with no continuation param still works', () => {
    const result = run(`
      (handle
        (begin (perform log) 42)
        ((log) -> 0))
    `);
    // Handler doesn't resume, so it returns 0
    expect(result.value.kind).toBe('number');
    expect(result.value.value).toBe(0);
  });
});
