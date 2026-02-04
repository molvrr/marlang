import { test, expect, describe } from 'bun:test';
import * as Lexer from '../src/core/lexer';
import * as Parser from '../src/core/parser';
import * as Interpreter from '../src/core/interpreter';
import { typeCheck, formatError, formatType } from '../src/types/type-checker';
import * as path from 'path';
import {
  registerPathPrefix,
  unregisterPathPrefix,
  getRegisteredPrefixes,
  resolveModulePath,
} from '../src/modules/module-loader';

// Helper to parse, type check, and evaluate code with file context for imports
const runWithImports = (code: string, testFile: string): string => {
  const tokens = Lexer.lex(Lexer.empty(code));
  const parsed = Parser.parse(Parser.empty(tokens));

  // Type check with file context
  const filePath = path.resolve(__dirname, testFile);
  const typeResult = typeCheck(parsed.exprs, filePath);
  if (!typeResult.ok) {
    throw new Error(`Type error: ${formatError(typeResult.error)}`);
  }

  // Evaluate with file context
  const interp = Interpreter.empty(filePath);
  const result = Interpreter.evalMany(interp, parsed.exprs);
  return Interpreter.formatValue(result.value);
};

// Helper to check types with file context
const checkWithImports = (code: string, testFile: string): { ok: boolean; types?: any[]; error?: string } => {
  const tokens = Lexer.lex(Lexer.empty(code));
  const parsed = Parser.parse(Parser.empty(tokens));
  const filePath = path.resolve(__dirname, testFile);
  const result = typeCheck(parsed.exprs, filePath);

  if (!result.ok) {
    return { ok: false, error: formatError(result.error) };
  }
  return { ok: true, types: result.types };
};

describe('stdlib/option.mar', () => {
  const optionImport = '(import "../stdlib/option.mar" as opt)';
  const testFile = 'stdlib.test.mar';

  describe('map', () => {
    test('maps over Some', () => {
      const code = `
        ${optionImport}
        (opt/map (lambda (x) (+ x 1)) (Some 5))
      `;
      expect(runWithImports(code, testFile)).toBe('(Some 6)');
    });

    test('maps over None returns None', () => {
      const code = `
        ${optionImport}
        (opt/map (lambda (x) (+ x 1)) None)
      `;
      expect(runWithImports(code, testFile)).toBe('None');
    });
  });

  describe('flat-map', () => {
    test('flat-maps over Some with Some result', () => {
      const code = `
        ${optionImport}
        (opt/flat-map (lambda (x) (Some (+ x 1))) (Some 5))
      `;
      expect(runWithImports(code, testFile)).toBe('(Some 6)');
    });

    test('flat-maps over Some with None result', () => {
      const code = `
        ${optionImport}
        (opt/flat-map (lambda (x) None) (Some 5))
      `;
      expect(runWithImports(code, testFile)).toBe('None');
    });

    test('flat-maps over None returns None', () => {
      const code = `
        ${optionImport}
        (opt/flat-map (lambda (x) (Some (+ x 1))) None)
      `;
      expect(runWithImports(code, testFile)).toBe('None');
    });
  });

  describe('unwrap-or', () => {
    test('unwraps Some', () => {
      const code = `
        ${optionImport}
        (opt/unwrap-or 0 (Some 42))
      `;
      expect(runWithImports(code, testFile)).toBe('42');
    });

    test('returns default for None', () => {
      const code = `
        ${optionImport}
        (opt/unwrap-or 0 None)
      `;
      expect(runWithImports(code, testFile)).toBe('0');
    });
  });

  describe('unwrap-or-else', () => {
    test('unwraps Some without calling function', () => {
      const code = `
        ${optionImport}
        (opt/unwrap-or-else (lambda (_) 999) (Some 42))
      `;
      expect(runWithImports(code, testFile)).toBe('42');
    });

    test('calls function for None', () => {
      const code = `
        ${optionImport}
        (opt/unwrap-or-else (lambda (_) 999) None)
      `;
      expect(runWithImports(code, testFile)).toBe('999');
    });
  });

  describe('is-some / is-none', () => {
    test('is-some returns true for Some', () => {
      const code = `
        ${optionImport}
        (opt/is-some (Some 42))
      `;
      expect(runWithImports(code, testFile)).toBe('#t');
    });

    test('is-some returns false for None', () => {
      const code = `
        ${optionImport}
        (opt/is-some None)
      `;
      expect(runWithImports(code, testFile)).toBe('#f');
    });

    test('is-none returns false for Some', () => {
      const code = `
        ${optionImport}
        (opt/is-none (Some 42))
      `;
      expect(runWithImports(code, testFile)).toBe('#f');
    });

    test('is-none returns true for None', () => {
      const code = `
        ${optionImport}
        (opt/is-none None)
      `;
      expect(runWithImports(code, testFile)).toBe('#t');
    });
  });

  describe('filter', () => {
    test('keeps value when predicate is true', () => {
      const code = `
        ${optionImport}
        (opt/filter (lambda (x) (> x 10)) (Some 42))
      `;
      expect(runWithImports(code, testFile)).toBe('(Some 42)');
    });

    test('returns None when predicate is false', () => {
      const code = `
        ${optionImport}
        (opt/filter (lambda (x) (> x 100)) (Some 42))
      `;
      expect(runWithImports(code, testFile)).toBe('None');
    });

    test('returns None for None input', () => {
      const code = `
        ${optionImport}
        (opt/filter (lambda (x) #t) None)
      `;
      expect(runWithImports(code, testFile)).toBe('None');
    });
  });

  describe('and-then', () => {
    test('returns second when first is Some', () => {
      const code = `
        ${optionImport}
        (opt/and-then (Some 99) (Some 42))
      `;
      expect(runWithImports(code, testFile)).toBe('(Some 99)');
    });

    test('returns None when first is None', () => {
      const code = `
        ${optionImport}
        (opt/and-then (Some 99) None)
      `;
      expect(runWithImports(code, testFile)).toBe('None');
    });
  });

  describe('or-else', () => {
    test('returns first when it is Some', () => {
      const code = `
        ${optionImport}
        (opt/or-else (Some 99) (Some 42))
      `;
      expect(runWithImports(code, testFile)).toBe('(Some 42)');
    });

    test('returns second when first is None', () => {
      const code = `
        ${optionImport}
        (opt/or-else (Some 99) None)
      `;
      expect(runWithImports(code, testFile)).toBe('(Some 99)');
    });
  });

  describe('type checking', () => {
    test('map has correct polymorphic type', () => {
      const code = `
        ${optionImport}
        (opt/map (lambda (x) (+ x 1)) (Some 5))
      `;
      const result = checkWithImports(code, testFile);
      expect(result.ok).toBe(true);
    });

    test('chaining operations type checks', () => {
      const code = `
        ${optionImport}
        (opt/flat-map
          (lambda (x) (if (> x 0) (Some (* x 2)) None))
          (opt/map (lambda (y) (+ y 1)) (Some 5)))
      `;
      const result = checkWithImports(code, testFile);
      expect(result.ok).toBe(true);
    });
  });
});

describe('stdlib/result.mar', () => {
  const resultImport = '(import "../stdlib/result.mar" as res)';
  const testFile = 'stdlib.test.mar';

  describe('basic construction', () => {
    test('Ok constructor', () => {
      const code = `
        ${resultImport}
        (res/Ok 42)
      `;
      expect(runWithImports(code, testFile)).toBe('(Ok 42)');
    });

    test('Err constructor', () => {
      const code = `
        ${resultImport}
        (res/Err "error message")
      `;
      expect(runWithImports(code, testFile)).toBe('(Err "error message")');
    });
  });

  describe('map', () => {
    test('maps over Ok', () => {
      const code = `
        ${resultImport}
        (res/map (lambda (x) (+ x 1)) (res/Ok 5))
      `;
      expect(runWithImports(code, testFile)).toBe('(Ok 6)');
    });

    test('preserves Err', () => {
      const code = `
        ${resultImport}
        (res/map (lambda (x) (+ x 1)) (res/Err "oops"))
      `;
      expect(runWithImports(code, testFile)).toBe('(Err "oops")');
    });
  });

  describe('map-err', () => {
    test('preserves Ok', () => {
      const code = `
        ${resultImport}
        (res/map-err (lambda (e) "transformed") (res/Ok 42))
      `;
      expect(runWithImports(code, testFile)).toBe('(Ok 42)');
    });

    test('maps over Err', () => {
      const code = `
        ${resultImport}
        (res/map-err (lambda (e) "transformed") (res/Err "original"))
      `;
      expect(runWithImports(code, testFile)).toBe('(Err "transformed")');
    });
  });

  describe('flat-map', () => {
    test('flat-maps over Ok with Ok result', () => {
      const code = `
        ${resultImport}
        (res/flat-map (lambda (x) (res/Ok (+ x 1))) (res/Ok 5))
      `;
      expect(runWithImports(code, testFile)).toBe('(Ok 6)');
    });

    test('flat-maps over Ok with Err result', () => {
      const code = `
        ${resultImport}
        (res/flat-map (lambda (x) (res/Err "failed")) (res/Ok 5))
      `;
      expect(runWithImports(code, testFile)).toBe('(Err "failed")');
    });

    test('flat-maps over Err returns original Err', () => {
      const code = `
        ${resultImport}
        (res/flat-map (lambda (x) (res/Ok (+ x 1))) (res/Err "original"))
      `;
      expect(runWithImports(code, testFile)).toBe('(Err "original")');
    });
  });

  describe('unwrap-or', () => {
    test('unwraps Ok', () => {
      const code = `
        ${resultImport}
        (res/unwrap-or 0 (res/Ok 42))
      `;
      expect(runWithImports(code, testFile)).toBe('42');
    });

    test('returns default for Err', () => {
      const code = `
        ${resultImport}
        (res/unwrap-or 0 (res/Err "error"))
      `;
      expect(runWithImports(code, testFile)).toBe('0');
    });
  });

  describe('unwrap-or-else', () => {
    test('unwraps Ok without calling function', () => {
      const code = `
        ${resultImport}
        (res/unwrap-or-else (lambda (e) 999) (res/Ok 42))
      `;
      expect(runWithImports(code, testFile)).toBe('42');
    });

    test('calls function with error for Err', () => {
      const code = `
        ${resultImport}
        (res/unwrap-or-else (lambda (e) 999) (res/Err "ignored"))
      `;
      expect(runWithImports(code, testFile)).toBe('999');
    });
  });

  describe('is-ok / is-err', () => {
    test('is-ok returns true for Ok', () => {
      const code = `
        ${resultImport}
        (res/is-ok (res/Ok 42))
      `;
      expect(runWithImports(code, testFile)).toBe('#t');
    });

    test('is-ok returns false for Err', () => {
      const code = `
        ${resultImport}
        (res/is-ok (res/Err "error"))
      `;
      expect(runWithImports(code, testFile)).toBe('#f');
    });

    test('is-err returns false for Ok', () => {
      const code = `
        ${resultImport}
        (res/is-err (res/Ok 42))
      `;
      expect(runWithImports(code, testFile)).toBe('#f');
    });

    test('is-err returns true for Err', () => {
      const code = `
        ${resultImport}
        (res/is-err (res/Err "error"))
      `;
      expect(runWithImports(code, testFile)).toBe('#t');
    });
  });

  describe('ok / err conversions', () => {
    test('ok extracts value from Ok as Some', () => {
      const code = `
        ${resultImport}
        (res/ok (res/Ok 42))
      `;
      expect(runWithImports(code, testFile)).toBe('(Some 42)');
    });

    test('ok returns None for Err', () => {
      const code = `
        ${resultImport}
        (res/ok (res/Err "error"))
      `;
      expect(runWithImports(code, testFile)).toBe('None');
    });

    test('err extracts error from Err as Some', () => {
      const code = `
        ${resultImport}
        (res/err (res/Err "oops"))
      `;
      expect(runWithImports(code, testFile)).toBe('(Some "oops")');
    });

    test('err returns None for Ok', () => {
      const code = `
        ${resultImport}
        (res/err (res/Ok 42))
      `;
      expect(runWithImports(code, testFile)).toBe('None');
    });
  });

  describe('and-then', () => {
    test('returns second when first is Ok', () => {
      const code = `
        ${resultImport}
        (res/and-then (res/Ok 99) (res/Ok 42))
      `;
      expect(runWithImports(code, testFile)).toBe('(Ok 99)');
    });

    test('returns Err when first is Err', () => {
      const code = `
        ${resultImport}
        (res/and-then (res/Ok 99) (res/Err "error"))
      `;
      expect(runWithImports(code, testFile)).toBe('(Err "error")');
    });
  });

  describe('or-else', () => {
    test('returns first when it is Ok', () => {
      const code = `
        ${resultImport}
        (res/or-else (res/Ok 99) (res/Ok 42))
      `;
      expect(runWithImports(code, testFile)).toBe('(Ok 42)');
    });

    test('returns second when first is Err', () => {
      const code = `
        ${resultImport}
        (res/or-else (res/Ok 99) (res/Err "error"))
      `;
      expect(runWithImports(code, testFile)).toBe('(Ok 99)');
    });
  });

  describe('type checking', () => {
    test('Result type is properly inferred', () => {
      const code = `
        ${resultImport}
        (define safe-div
          (lambda (a b)
            (if (= b 0)
                (res/Err "division by zero")
                (res/Ok (/ a b)))))
        (safe-div 10 2)
      `;
      const result = checkWithImports(code, testFile);
      expect(result.ok).toBe(true);
    });

    test('qualified type annotations work', () => {
      // After the mergeEnv fix, qualified type names in annotations should work
      const code = `
        ${resultImport}
        (define (safe-div : (-> Int Int (res/Result String Int)))
          (lambda (a b)
            (if (= b 0)
                (res/Err "division by zero")
                (res/Ok (/ a b)))))
        (safe-div 10 2)
      `;
      const result = checkWithImports(code, testFile);
      expect(result.ok).toBe(true);
    });

    test('chaining operations type checks', () => {
      const code = `
        ${resultImport}
        (res/flat-map
          (lambda (x) (if (> x 0) (res/Ok (* x 2)) (res/Err "negative")))
          (res/map (lambda (y) (+ y 1)) (res/Ok 5)))
      `;
      const result = checkWithImports(code, testFile);
      expect(result.ok).toBe(true);
    });
  });
});

describe('Option and Result interop', () => {
  const imports = `
    (import "../stdlib/option.mar" as opt)
    (import "../stdlib/result.mar" as res)
  `;
  const testFile = 'stdlib.test.mar';

  test('convert Result to Option', () => {
    const code = `
      ${imports}
      (opt/map (lambda (x) (+ x 1)) (res/ok (res/Ok 5)))
    `;
    expect(runWithImports(code, testFile)).toBe('(Some 6)');
  });

  test('Option and Result can be used together', () => {
    const code = `
      ${imports}
      (let ((maybe-value (Some 10))
            (result (opt/map (lambda (x) (res/Ok (* x 2))) maybe-value)))
        (opt/unwrap-or (res/Err "no value") result))
    `;
    expect(runWithImports(code, testFile)).toBe('(Ok 20)');
  });
});

describe('std: import prefix', () => {
  const testFile = 'stdlib.test.mar';

  test('import option with std: prefix', () => {
    const code = `
      (import "std:option" as opt)
      (opt/map (lambda (x) (+ x 1)) (Some 5))
    `;
    expect(runWithImports(code, testFile)).toBe('(Some 6)');
  });

  test('import result with std: prefix', () => {
    const code = `
      (import "std:result" as res)
      (res/map (lambda (x) (* x 2)) (res/Ok 10))
    `;
    expect(runWithImports(code, testFile)).toBe('(Ok 20)');
  });

  test('std: imports work together', () => {
    const code = `
      (import "std:option" as opt)
      (import "std:result" as res)
      (opt/unwrap-or 0 (res/ok (res/Ok 42)))
    `;
    expect(runWithImports(code, testFile)).toBe('42');
  });

  test('std: imports type check correctly', () => {
    const code = `
      (import "std:result" as res)
      (define (safe-div : (-> Int Int (res/Result String Int)))
        (lambda (a b)
          (if (= b 0)
              (res/Err "division by zero")
              (res/Ok (/ a b)))))
      (safe-div 10 2)
    `;
    const result = checkWithImports(code, testFile);
    expect(result.ok).toBe(true);
  });
});

describe('stdlib/io.mar', () => {
  const ioImport = '(import "std:io" as io)';
  const testFile = 'stdlib.test.mar';

  describe('IO effect declaration', () => {
    test('IO module type checks', () => {
      const code = `${ioImport}`;
      const result = checkWithImports(code, testFile);
      expect(result.ok).toBe(true);
    });

    test('print function has IO effect', () => {
      const code = `
        ${ioImport}
        (handle
          (io/print "hello")
          ((print msg k) -> (resume k nil)))
      `;
      expect(runWithImports(code, testFile)).toBe('nil');
    });

    test('println function has IO effect', () => {
      const code = `
        ${ioImport}
        (handle
          (io/println "hello")
          ((println msg k) -> (resume k nil)))
      `;
      expect(runWithImports(code, testFile)).toBe('nil');
    });

    test('read-line function returns String', () => {
      const code = `
        ${ioImport}
        (handle
          (io/read-line)
          ((read-line k) -> (resume k "test input")))
      `;
      expect(runWithImports(code, testFile)).toBe('"test input"');
    });

    test('read-file function takes path and returns content', () => {
      const code = `
        ${ioImport}
        (handle
          (io/read-file "/path/to/file")
          ((read-file path k) -> (resume k "file contents")))
      `;
      expect(runWithImports(code, testFile)).toBe('"file contents"');
    });

    test('write-file function takes path and content', () => {
      const code = `
        ${ioImport}
        (handle
          (io/write-file "/path/to/file" "content")
          ((write-file path content k) -> (resume k nil)))
      `;
      expect(runWithImports(code, testFile)).toBe('nil');
    });

    test('convenience functions work', () => {
      const code = `
        ${ioImport}
        (handle
          (io/println-int 42)
          ((println msg k) -> (resume k nil)))
      `;
      expect(runWithImports(code, testFile)).toBe('nil');
    });

    test('chaining IO operations', () => {
      const code = `
        ${ioImport}
        (handle
          (begin
            (io/print "Name: ")
            (let ((name (io/read-line)))
              (io/println name)
              name))
          ((print msg k) -> (resume k nil))
          ((println msg k) -> (resume k nil))
          ((read-line k) -> (resume k "Alice")))
      `;
      expect(runWithImports(code, testFile)).toBe('"Alice"');
    });
  });
});

describe('extensible path prefix system', () => {
  test('std prefix is registered by default', () => {
    expect(getRegisteredPrefixes()).toContain('std');
  });

  test('can register custom prefix', () => {
    // Register a custom prefix that points to stdlib (for testing)
    registerPathPrefix('mylib', (name) => path.join(__dirname, '..', 'stdlib', name));

    expect(getRegisteredPrefixes()).toContain('mylib');

    // Resolve a path using the custom prefix
    const resolved = resolveModulePath('mylib:option', '/some/file.mar');
    expect(resolved).toContain('stdlib');
    expect(resolved).toContain('option.mar');

    // Clean up
    unregisterPathPrefix('mylib');
    expect(getRegisteredPrefixes()).not.toContain('mylib');
  });

  test('unregister returns false for non-existent prefix', () => {
    expect(unregisterPathPrefix('nonexistent')).toBe(false);
  });

  test('throws error for unknown prefix', () => {
    expect(() => {
      resolveModulePath('unknown:module', '/some/file.mar');
    }).toThrow(/Unknown import prefix/);
  });

  test('custom prefix can resolve paths dynamically based on fromFile', () => {
    // Register a prefix that resolves relative to the importing file's directory
    registerPathPrefix('local', (name, fromFile) => {
      const fromDir = path.dirname(fromFile);
      return path.join(fromDir, 'local_modules', name);
    });

    const resolved = resolveModulePath('local:utils', '/project/src/main.mar');
    expect(resolved).toBe(path.normalize('/project/src/local_modules/utils.mar'));

    // Clean up
    unregisterPathPrefix('local');
  });
});
