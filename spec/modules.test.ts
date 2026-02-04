import { test, expect, describe } from 'bun:test';
import * as path from 'path';
import * as Lexer from '../src/core/lexer';
import * as Parser from '../src/core/parser';
import * as Interpreter from '../src/core/interpreter';
import { typeCheck, formatError } from '../src/types/type-checker';
import { formatType } from '../src/types/type-ast';

// Path to fixtures
const fixturesDir = path.join(__dirname, 'fixtures/modules');

// Helper to parse, type check, and evaluate code with file context
const runWithContext = (code: string, filePath: string): string => {
  const tokens = Lexer.lex(Lexer.empty(code));
  const parsed = Parser.parse(Parser.empty(tokens));

  // Type check with file path
  const typeResult = typeCheck(parsed.exprs, filePath);
  if (!typeResult.ok) {
    throw new Error(`Type error: ${formatError(typeResult.error)}`);
  }

  // Evaluate with file path
  const result = Interpreter.evalMany(Interpreter.empty(filePath), parsed.exprs);
  return Interpreter.formatValue(result.value);
};

// Helper for type checking only
const checkWithContext = (code: string, filePath: string): { ok: boolean; error?: string } => {
  const tokens = Lexer.lex(Lexer.empty(code));
  const parsed = Parser.parse(Parser.empty(tokens));
  const result = typeCheck(parsed.exprs, filePath);

  if (!result.ok) {
    return { ok: false, error: formatError(result.error) };
  }
  return { ok: true };
};

// Create a virtual file path for test code
const testFile = (name: string) => path.join(fixturesDir, name);

describe('Module System - Import All', () => {
  test('imports all bindings from module', () => {
    const code = `
      (import "./math.mar")
      (square 5)
    `;
    const result = runWithContext(code, testFile('test.mar'));
    expect(result).toBe('25');
  });

  test('can use multiple imported functions', () => {
    const code = `
      (import "./math.mar")
      (add (square 2) (double 3))
    `;
    const result = runWithContext(code, testFile('test.mar'));
    expect(result).toBe('10'); // 4 + 6
  });
});

describe('Module System - Selective Imports', () => {
  test('imports only selected bindings', () => {
    const code = `
      (import "./math.mar" (square))
      (square 6)
    `;
    const result = runWithContext(code, testFile('test.mar'));
    expect(result).toBe('36');
  });

  test('can import multiple selected bindings', () => {
    const code = `
      (import "./math.mar" (square double))
      (+ (square 3) (double 4))
    `;
    const result = runWithContext(code, testFile('test.mar'));
    expect(result).toBe('17'); // 9 + 8
  });

  test('error when importing non-existent binding', () => {
    const code = `
      (import "./math.mar" (nonexistent))
      42
    `;
    expect(() => runWithContext(code, testFile('test.mar'))).toThrow('does not export');
  });
});

describe('Module System - Qualified Imports', () => {
  test('imports with qualifier prefix', () => {
    const code = `
      (import "./math.mar" as m)
      (m/square 7)
    `;
    const result = runWithContext(code, testFile('test.mar'));
    expect(result).toBe('49');
  });

  test('can use multiple qualified functions', () => {
    const code = `
      (import "./math.mar" as math)
      (math/add (math/square 2) (math/double 5))
    `;
    const result = runWithContext(code, testFile('test.mar'));
    expect(result).toBe('14'); // 4 + 10
  });
});

describe('Module System - Path Resolution', () => {
  test('resolves relative paths', () => {
    const code = `
      (import "./math.mar")
      (square 3)
    `;
    const result = runWithContext(code, testFile('test.mar'));
    expect(result).toBe('9');
  });

  test('auto-appends .mar extension', () => {
    const code = `
      (import "./math")
      (square 4)
    `;
    const result = runWithContext(code, testFile('test.mar'));
    expect(result).toBe('16');
  });
});

describe('Module System - Circular Dependencies', () => {
  test('detects circular dependency', () => {
    const code = `
      (import "./circular-a.mar")
      a-value
    `;
    expect(() => runWithContext(code, testFile('test.mar'))).toThrow('Circular dependency');
  });
});

describe('Module System - Module Caching', () => {
  test('caches modules (diamond dependency)', () => {
    // test.mar imports both math.mar directly and uses-math.mar (which also imports math.mar)
    // math.mar should only be loaded once
    const code = `
      (import "./math.mar")
      (import "./uses-math.mar")
      (+ (square 2) (quad 2))
    `;
    const result = runWithContext(code, testFile('test.mar'));
    expect(result).toBe('20'); // 4 + 16
  });
});

describe('Module System - Type Checking', () => {
  test('type checks imported functions', () => {
    const code = `
      (import "./math.mar")
      (square 5)
    `;
    const result = checkWithContext(code, testFile('test.mar'));
    expect(result.ok).toBe(true);
  });

  test('catches type errors with imported functions', () => {
    const code = `
      (import "./math.mar")
      (square "hello")
    `;
    const result = checkWithContext(code, testFile('test.mar'));
    expect(result.ok).toBe(false);
    expect(result.error).toContain('cannot unify');
  });

  test('imports data types from modules', () => {
    const code = `
      (import "./types.mar")
      (Ok 42)
    `;
    const result = checkWithContext(code, testFile('test.mar'));
    expect(result.ok).toBe(true);
  });

  test('uses imported data types correctly', () => {
    const code = `
      (import "./types.mar")
      (map-result (lambda (x) (+ x 1)) (Ok 5))
    `;
    const result = runWithContext(code, testFile('test.mar'));
    expect(result).toBe('(Ok 6)');
  });
});

describe('Module System - Effects', () => {
  test('imports effect definitions', () => {
    const code = `
      (import "./effects.mar")
      (handle
        (count-up 3)
        ((inc k) -> (resume k 1))
        ((get k) -> (resume k 0)))
    `;
    const result = checkWithContext(code, testFile('test.mar'));
    expect(result.ok).toBe(true);
  });
});

describe('Module System - Error Handling', () => {
  test('error for non-existent module', () => {
    const code = `
      (import "./nonexistent.mar")
      42
    `;
    expect(() => runWithContext(code, testFile('test.mar'))).toThrow('Module not found');
  });

  test('error for import in REPL context', () => {
    const code = `(import "./math.mar")`;
    const tokens = Lexer.lex(Lexer.empty(code));
    const parsed = Parser.parse(Parser.empty(tokens));

    // Without file path (REPL mode)
    expect(() => {
      Interpreter.evalMany(Interpreter.empty(), parsed.exprs);
    }).toThrow('Cannot use import without a file context');
  });
});
