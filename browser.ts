// Browser entry point for Marlang
// Exports core functions for parsing, type checking, and evaluation

import * as Lexer from './src/core/lexer';
import * as Parser from './src/core/parser';
import * as Interpreter from './src/core/interpreter';
import { registerDefaultHandler } from './src/core/interpreter';
import { registerVirtualModule, registerPathPrefix } from './src/modules/module-loader';

// =============================================================================
// Browser Stdlib - Embedded module sources for browser environment
// =============================================================================

const STDLIB_OPTION = `; Option utilities for the built-in Option type
; Option is already defined as: (data (Option a) None (Some a))

; map : (-> (-> a b) (Option a) (Option b))
(define (map : (-> (-> a b) (Option a) (Option b)))
  (lambda (f opt)
    (match opt
      (None None)
      ((Some x) (Some (f x))))))

; flat-map : (-> (-> a (Option b)) (Option a) (Option b))
(define (flat-map : (-> (-> a (Option b)) (Option a) (Option b)))
  (lambda (f opt)
    (match opt
      (None None)
      ((Some x) (f x)))))

; unwrap-or : (-> a (Option a) a)
(define (unwrap-or : (-> a (Option a) a))
  (lambda (default opt)
    (match opt
      (None default)
      ((Some x) x))))

; unwrap-or-else : (-> (-> Unit a) (Option a) a)
(define (unwrap-or-else : (-> (-> Unit a) (Option a) a))
  (lambda (f opt)
    (match opt
      (None (f nil))
      ((Some x) x))))

; is-some : (-> (Option a) Bool)
(define (is-some : (-> (Option a) Bool))
  (lambda (opt)
    (match opt
      (None #f)
      ((Some _) #t))))

; is-none : (-> (Option a) Bool)
(define (is-none : (-> (Option a) Bool))
  (lambda (opt)
    (match opt
      (None #t)
      ((Some _) #f))))

; filter : (-> (-> a Bool) (Option a) (Option a))
(define (filter : (-> (-> a Bool) (Option a) (Option a)))
  (lambda (pred opt)
    (match opt
      (None None)
      ((Some x) (if (pred x) (Some x) None)))))

; and-then : (-> (Option b) (Option a) (Option b))
(define (and-then : (-> (Option b) (Option a) (Option b)))
  (lambda (other opt)
    (match opt
      (None None)
      ((Some _) other))))

; or-else : (-> (Option a) (Option a) (Option a))
(define (or-else : (-> (Option a) (Option a) (Option a)))
  (lambda (other opt)
    (match opt
      (None other)
      ((Some x) (Some x)))))
`;

const STDLIB_RESULT = `; Result type and utilities
(data (Result e a) (Err e) (Ok a))

; map : (-> (-> a b) (Result e a) (Result e b))
(define (map : (-> (-> a b) (Result e a) (Result e b)))
  (lambda (f res)
    (match res
      ((Err e) (Err e))
      ((Ok x) (Ok (f x))))))

; map-err : (-> (-> e f) (Result e a) (Result f a))
(define (map-err : (-> (-> e f) (Result e a) (Result f a)))
  (lambda (f res)
    (match res
      ((Err e) (Err (f e)))
      ((Ok x) (Ok x)))))

; flat-map : (-> (-> a (Result e b)) (Result e a) (Result e b))
(define (flat-map : (-> (-> a (Result e b)) (Result e a) (Result e b)))
  (lambda (f res)
    (match res
      ((Err e) (Err e))
      ((Ok x) (f x)))))

; unwrap-or : (-> a (Result e a) a)
(define (unwrap-or : (-> a (Result e a) a))
  (lambda (default res)
    (match res
      ((Err _) default)
      ((Ok x) x))))

; unwrap-or-else : (-> (-> e a) (Result e a) a)
(define (unwrap-or-else : (-> (-> e a) (Result e a) a))
  (lambda (f res)
    (match res
      ((Err e) (f e))
      ((Ok x) x))))

; is-ok : (-> (Result e a) Bool)
(define (is-ok : (-> (Result e a) Bool))
  (lambda (res)
    (match res
      ((Err _) #f)
      ((Ok _) #t))))

; is-err : (-> (Result e a) Bool)
(define (is-err : (-> (Result e a) Bool))
  (lambda (res)
    (match res
      ((Err _) #t)
      ((Ok _) #f))))

; ok : (-> (Result e a) (Option a))
(define (ok : (-> (Result e a) (Option a)))
  (lambda (res)
    (match res
      ((Err _) None)
      ((Ok x) (Some x)))))

; err : (-> (Result e a) (Option e))
(define (err : (-> (Result e a) (Option e)))
  (lambda (res)
    (match res
      ((Err e) (Some e))
      ((Ok _) None))))

; and-then : (-> (Result e b) (Result e a) (Result e b))
(define (and-then : (-> (Result e b) (Result e a) (Result e b)))
  (lambda (other res)
    (match res
      ((Err e) (Err e))
      ((Ok _) other))))

; or-else : (-> (Result e a) (Result e a) (Result e a))
(define (or-else : (-> (Result e a) (Result e a) (Result e a)))
  (lambda (other res)
    (match res
      ((Err _) other)
      ((Ok x) (Ok x)))))
`;

const STDLIB_IO = `; IO module - Basic IO operations using the effect system
(effect IO
  (print (-> String Unit))
  (println (-> String Unit))
  (read-line (-> Unit String))
  (read-file (-> String String))
  (write-file (-> String String Unit)))

; print : (-> String {IO} Unit)
(define print (lambda (msg) (perform print msg)))

; println : (-> String {IO} Unit)
(define println (lambda (msg) (perform println msg)))

; read-line : (-> Unit {IO} String)
(define read-line (lambda () (perform read-line ())))

; read-file : (-> String {IO} String)
(define read-file (lambda (path) (perform read-file path)))

; write-file : (-> String String {IO} Unit)
(define write-file (lambda (path content) (perform write-file path content)))

; print-int : (-> Int {IO} Unit)
(define print-int (lambda (n) (perform print (show n))))

; println-int : (-> Int {IO} Unit)
(define println-int (lambda (n) (perform println (show n))))

; print-bool : (-> Bool {IO} Unit)
(define print-bool (lambda (b) (perform print (show b))))

; println-bool : (-> Bool {IO} Unit)
(define println-bool (lambda (b) (perform println (show b))))
`;

// Register virtual modules for browser stdlib
// Use a virtual path prefix that the resolver will use
const BROWSER_STDLIB_PATH = '/browser-stdlib';

registerVirtualModule(`${BROWSER_STDLIB_PATH}/option.mar`, STDLIB_OPTION);
registerVirtualModule(`${BROWSER_STDLIB_PATH}/result.mar`, STDLIB_RESULT);
registerVirtualModule(`${BROWSER_STDLIB_PATH}/io.mar`, STDLIB_IO);

// Register browser-compatible std: resolver that uses virtual modules
registerPathPrefix('std', (moduleName: string, _fromFile: string): string => {
  return `${BROWSER_STDLIB_PATH}/${moduleName}.mar`;
});

// =============================================================================
// Browser IO Handlers
// =============================================================================

// Print without newline (browser console doesn't support this, so we use console.log)
registerDefaultHandler('IO', 'print', (args) => {
  const msg = args[0];
  if (msg?.kind === 'string') {
    console.log(msg.value);
  }
  return { kind: 'nil' };
});

// Print with newline
registerDefaultHandler('IO', 'println', (args) => {
  const msg = args[0];
  if (msg?.kind === 'string') {
    console.log(msg.value);
  }
  return { kind: 'nil' };
});

// Read line - use browser prompt
registerDefaultHandler('IO', 'read-line', () => {
  const input = prompt('') ?? '';
  return { kind: 'string', value: input };
});

// File operations not available in browser
registerDefaultHandler('IO', 'read-file', () => {
  throw new Error('read-file is not available in browser environment');
});

registerDefaultHandler('IO', 'write-file', () => {
  throw new Error('write-file is not available in browser environment');
});
import { typeCheck as _typeCheck, formatError, formatTypes, formatType } from './src/types/type-checker';
import { buildTypeMap as _buildTypeMap, pathKey } from './src/types/type-map';
import type { Expr, RuntimeValue } from './src/core/types';
import type { TypeCheckResult } from './src/types/type-checker';
import type { TypeMapResult, NodePath, NodeTypeInfo } from './src/types/type-map';

export type { Expr, RuntimeValue, TypeCheckResult, TypeMapResult, NodePath, NodeTypeInfo };
export { pathKey };

// Parse source code into AST
export const parse = (code: string): Expr[] => {
  const tokens = Lexer.lex(Lexer.empty(code));
  const parsed = Parser.parse(Parser.empty(tokens));
  return parsed.exprs;
};

// Type check an AST
// Re-export formatError, formatTypes, formatType directly
export { formatError, formatTypes, formatType };

// Virtual file path for browser code - enables imports
const BROWSER_FILE_PATH = '/browser/main.mar';

// Browser-compatible typeCheck that includes file path for import support
export const typeCheck = (exprs: Expr[], filePath?: string): TypeCheckResult => {
  return _typeCheck(exprs, filePath ?? BROWSER_FILE_PATH);
};

// Browser-compatible buildTypeMap that includes file path for import support
export const buildTypeMap = (exprs: Expr[], filePath?: string): TypeMapResult => {
  return _buildTypeMap(exprs, filePath ?? BROWSER_FILE_PATH);
};

// Create a fresh interpreter with a virtual file path for browser imports
export const createInterpreter = () => Interpreter.empty(BROWSER_FILE_PATH);

// Evaluate expressions with an interpreter
export const evaluate = (interp: Interpreter.T, exprs: Expr[]): Interpreter.EvalResult => {
  return Interpreter.evalMany(interp, exprs);
};

// Evaluate a single expression
export const evaluateOne = (interp: Interpreter.T, expr: Expr): Interpreter.EvalResult => {
  return Interpreter.evalExpr(interp, expr);
};

// Format a runtime value for display
export const formatValue = Interpreter.formatValue;

// Convenience: parse, typecheck, and evaluate in one call
export const run = (code: string, options: { typecheck?: boolean } = {}): {
  value: RuntimeValue;
  typeResult?: TypeCheckResult;
} => {
  const exprs = parse(code);

  let typeResult: TypeCheckResult | undefined;
  if (options.typecheck !== false) {
    typeResult = typeCheck(exprs);  // Uses BROWSER_FILE_PATH by default
    if (!typeResult.ok) {
      throw new Error(`Type error: ${formatError(typeResult.error)}`);
    }
  }

  const interp = createInterpreter();
  const { value } = evaluate(interp, exprs);

  return { value, typeResult };
};

// Re-export types for TypeScript users
export type { Type } from './src/types/type-ast';
export { formatScheme } from './src/types/type-ast';

// ===== Builtin Reference for UI =====

export interface BuiltinCategory {
  name: string;
  items: BuiltinItem[];
}

export interface BuiltinItem {
  name: string;
  type: string;
  description?: string;
}

export interface DataTypeInfo {
  name: string;
  typeParams: string[];
  constructors: BuiltinItem[];
}

export interface TypeClassInfo {
  name: string;
  typeParam: string;
  superclasses: string[];
  methods: BuiltinItem[];
}

export interface EffectSyntaxItem {
  name: string;
  syntax: string;
  description: string;
  example?: string;
}

export interface BuiltinReference {
  builtins: BuiltinCategory[];
  dataTypes: DataTypeInfo[];
  typeClasses: TypeClassInfo[];
  effectSyntax: EffectSyntaxItem[];
}

export const getBuiltinReference = (): BuiltinReference => ({
  builtins: [
    {
      name: 'Arithmetic',
      items: [
        { name: '+', type: 'Num a => a -> a -> a', description: 'Addition' },
        { name: '-', type: 'Num a => a -> a -> a', description: 'Subtraction' },
        { name: '*', type: 'Num a => a -> a -> a', description: 'Multiplication' },
        { name: '/', type: 'Num a => a -> a -> a', description: 'Division' },
        { name: 'negate', type: 'Num a => a -> a', description: 'Negation' },
      ],
    },
    {
      name: 'Comparison',
      items: [
        { name: '=', type: 'Eq a => a -> a -> Bool', description: 'Equality' },
        { name: '<', type: 'Ord a => a -> a -> Bool', description: 'Less than' },
        { name: '>', type: 'Ord a => a -> a -> Bool', description: 'Greater than' },
        { name: '<=', type: 'Ord a => a -> a -> Bool', description: 'Less than or equal' },
        { name: '>=', type: 'Ord a => a -> a -> Bool', description: 'Greater than or equal' },
      ],
    },
    {
      name: 'Lists',
      items: [
        { name: 'list', type: 'a -> List a', description: 'Create a list' },
        { name: 'nil', type: 'Unit', description: 'Unit/empty value' },
      ],
    },
    {
      name: 'Output',
      items: [
        { name: 'show', type: 'Show a => a -> String', description: 'Convert to string' },
      ],
    },
    {
      name: 'FFI',
      items: [
        { name: 'ffi-open', type: 'String -> List a -> FFILibrary', description: 'Open a shared library' },
        { name: 'ffi-call', type: 'FFILibrary -> String -> a', description: 'Call a library function' },
        { name: 'ffi-close', type: 'FFILibrary -> Unit', description: 'Close a library' },
        { name: 'ffi-struct', type: 'String -> List a -> FFIStructDef', description: 'Define a struct type' },
        { name: 'struct-new', type: 'FFIStructDef -> a -> FFIStruct', description: 'Create a struct instance' },
        { name: 'struct-get', type: 'FFIStruct -> String -> a', description: 'Get a struct field' },
        { name: 'struct-set', type: 'FFIStruct -> String -> a -> Unit', description: 'Set a struct field' },
      ],
    },
  ],
  dataTypes: [
    {
      name: 'Option',
      typeParams: ['a'],
      constructors: [
        { name: 'None', type: 'Option a', description: 'No value' },
        { name: 'Some', type: 'a -> Option a', description: 'Wrap a value' },
      ],
    },
    {
      name: 'List',
      typeParams: ['a'],
      constructors: [
        { name: 'Nil', type: 'List a', description: 'Empty list' },
        { name: 'Cons', type: 'a -> List a -> List a', description: 'Prepend element' },
      ],
    },
    {
      name: 'Pair',
      typeParams: ['a', 'b'],
      constructors: [
        { name: 'MkPair', type: 'a -> b -> Pair a b', description: 'Create a pair' },
      ],
    },
  ],
  typeClasses: [
    {
      name: 'Num',
      typeParam: 'a',
      superclasses: [],
      methods: [
        { name: '+', type: 'a -> a -> a' },
        { name: '-', type: 'a -> a -> a' },
        { name: '*', type: 'a -> a -> a' },
        { name: '/', type: 'a -> a -> a' },
        { name: 'negate', type: 'a -> a' },
      ],
    },
    {
      name: 'Eq',
      typeParam: 'a',
      superclasses: [],
      methods: [
        { name: '=', type: 'a -> a -> Bool' },
      ],
    },
    {
      name: 'Ord',
      typeParam: 'a',
      superclasses: ['Eq'],
      methods: [
        { name: '<', type: 'a -> a -> Bool' },
        { name: '>', type: 'a -> a -> Bool' },
        { name: '<=', type: 'a -> a -> Bool' },
        { name: '>=', type: 'a -> a -> Bool' },
      ],
    },
    {
      name: 'Show',
      typeParam: 'a',
      superclasses: [],
      methods: [
        { name: 'show', type: 'a -> String' },
      ],
    },
  ],
  effectSyntax: [
    {
      name: 'effect',
      syntax: '(effect Name (op : Type -> Type) ...)',
      description: 'Declare an effect with typed operations',
      example: '(effect State\n  (get : Unit -> Int)\n  (put : Int -> Unit))',
    },
    {
      name: 'perform',
      syntax: '(perform op arg)',
      description: 'Invoke an effect operation',
      example: '(perform get ())\n(perform put 42)',
    },
    {
      name: 'handle',
      syntax: '(handle expr ((op args k) -> body) ...)',
      description: 'Handle effects with pattern-matched handlers',
      example: '(handle\n  (perform get ())\n  ((get k) -> (resume k 10)))',
    },
    {
      name: 'resume',
      syntax: '(resume k value)',
      description: 'Resume the continuation with a value',
      example: '((get k) -> (resume k 42))',
    },
    {
      name: 'return',
      syntax: '((return x) -> body)',
      description: 'Handle the final return value in a handler',
      example: '(handle 42\n  ((return x) -> (+ x 1)))',
    },
    {
      name: 'IO Effect',
      syntax: '(import "std:io" as io)',
      description: 'Built-in IO handlers work automatically. Use io/println, io/print, io/read-line. In browser: read-line uses prompt(), file ops unavailable.',
      example: '(import "std:io" as io)\n(io/println "Hello!")\n(io/print "Name: ")\n(let ((n (io/read-line)))\n  (io/println n))',
    },
  ],
});
