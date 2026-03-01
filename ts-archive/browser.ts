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

const STDLIB_LIST = `; List utilities for the built-in List type
; List is already defined as: (data (List a) Nil (Cons a (List a)))

; Pair type for zip operations
(data (Pair a b) (Pair a b))

(define (foldl : (-> (-> b a b) b (List a) b))
  (lambda (f acc lst)
    (match lst
      ((Nil) acc)
      ((Cons h t) (foldl f (f acc h) t)))))

(define (foldr : (-> (-> a b b) b (List a) b))
  (lambda (f acc lst)
    (match lst
      ((Nil) acc)
      ((Cons h t) (f h (foldr f acc t))))))

(define (length : (-> (List a) Int))
  (lambda (lst)
    (match lst
      ((Nil) 0)
      ((Cons _ t) (+ 1 (length t))))))

(define (reverse : (-> (List a) (List a)))
  (lambda (lst)
    (foldl (lambda (acc x) (Cons x acc)) Nil lst)))

(define (concat : (-> (List a) (List a) (List a)))
  (lambda (xs ys)
    (match xs
      ((Nil) ys)
      ((Cons h t) (Cons h (concat t ys))))))

(define (singleton : (-> a (List a)))
  (lambda (x) (Cons x Nil)))

(define (is-empty : (-> (List a) Bool))
  (lambda (lst)
    (match lst
      ((Nil) #t)
      ((Cons _ _) #f))))

(define (head : (-> (List a) (Option a)))
  (lambda (lst)
    (match lst
      ((Nil) None)
      ((Cons h _) (Some h)))))

(define (tail : (-> (List a) (Option (List a))))
  (lambda (lst)
    (match lst
      ((Nil) None)
      ((Cons _ t) (Some t)))))

(define (last : (-> (List a) (Option a)))
  (lambda (lst)
    (match lst
      ((Nil) None)
      ((Cons h (Nil)) (Some h))
      ((Cons _ t) (last t)))))

(define (nth : (-> Int (List a) (Option a)))
  (lambda (n lst)
    (match lst
      ((Nil) None)
      ((Cons h t) (if (= n 0) (Some h) (nth (- n 1) t))))))

(define (map : (-> (-> a b) (List a) (List b)))
  (lambda (f lst)
    (match lst
      ((Nil) Nil)
      ((Cons h t) (Cons (f h) (map f t))))))

(define (filter : (-> (-> a Bool) (List a) (List a)))
  (lambda (pred lst)
    (match lst
      ((Nil) Nil)
      ((Cons h t)
        (if (pred h)
          (Cons h (filter pred t))
          (filter pred t))))))

(define (any : (-> (-> a Bool) (List a) Bool))
  (lambda (pred lst)
    (match lst
      ((Nil) #f)
      ((Cons h t) (if (pred h) #t (any pred t))))))

(define (all : (-> (-> a Bool) (List a) Bool))
  (lambda (pred lst)
    (match lst
      ((Nil) #t)
      ((Cons h t) (if (pred h) (all pred t) #f)))))

(define (find : (-> (-> a Bool) (List a) (Option a)))
  (lambda (pred lst)
    (match lst
      ((Nil) None)
      ((Cons h t) (if (pred h) (Some h) (find pred t))))))

(define (take : (-> Int (List a) (List a)))
  (lambda (n lst)
    (if (<= n 0)
      Nil
      (match lst
        ((Nil) Nil)
        ((Cons h t) (Cons h (take (- n 1) t)))))))

(define (drop : (-> Int (List a) (List a)))
  (lambda (n lst)
    (if (<= n 0)
      lst
      (match lst
        ((Nil) Nil)
        ((Cons _ t) (drop (- n 1) t))))))

(define (take-while : (-> (-> a Bool) (List a) (List a)))
  (lambda (pred lst)
    (match lst
      ((Nil) Nil)
      ((Cons h t)
        (if (pred h)
          (Cons h (take-while pred t))
          Nil)))))

(define (drop-while : (-> (-> a Bool) (List a) (List a)))
  (lambda (pred lst)
    (match lst
      ((Nil) Nil)
      ((Cons h t)
        (if (pred h)
          (drop-while pred t)
          lst)))))

(define (zip : (-> (List a) (List b) (List (Pair a b))))
  (lambda (xs ys)
    (match xs
      ((Nil) Nil)
      ((Cons x xt)
        (match ys
          ((Nil) Nil)
          ((Cons y yt) (Cons (Pair x y) (zip xt yt))))))))

(define (zip-with : (-> (-> a b c) (List a) (List b) (List c)))
  (lambda (f xs ys)
    (match xs
      ((Nil) Nil)
      ((Cons x xt)
        (match ys
          ((Nil) Nil)
          ((Cons y yt) (Cons (f x y) (zip-with f xt yt))))))))
`;

const STDLIB_STRING = `; String utilities module
(define (length : (-> String Int))
  (lambda (s) (string-length s)))

(define (concat : (-> String String String))
  (lambda (a b) (string-concat a b)))

(define (is-empty : (-> String Bool))
  (lambda (s) (string-is-empty s)))

(define (reverse : (-> String String))
  (lambda (s) (string-reverse s)))

(define (repeat : (-> Int String String))
  (lambda (n s) (string-repeat s n)))

(define (to-upper : (-> String String))
  (lambda (s) (string-to-upper s)))

(define (to-lower : (-> String String))
  (lambda (s) (string-to-lower s)))

(define (trim : (-> String String))
  (lambda (s) (string-trim s)))

(define (trim-start : (-> String String))
  (lambda (s) (string-trim-start s)))

(define (trim-end : (-> String String))
  (lambda (s) (string-trim-end s)))

(define (starts-with : (-> String String Bool))
  (lambda (prefix s) (string-starts-with s prefix)))

(define (ends-with : (-> String String Bool))
  (lambda (suffix s) (string-ends-with s suffix)))

(define (contains : (-> String String Bool))
  (lambda (sub s) (string-contains s sub)))

(define (index-of : (-> String String (Option Int)))
  (lambda (sub s) (string-index-of s sub)))

(define (substring : (-> Int Int String String))
  (lambda (start end s) (string-substring s start end)))

(define (char-at : (-> Int String (Option String)))
  (lambda (idx s) (string-char-at s idx)))

(define (take : (-> Int String String))
  (lambda (n s) (string-substring s 0 n)))

(define (drop : (-> Int String String))
  (lambda (n s) (string-substring s n (string-length s))))

(define (split : (-> String String (List String)))
  (lambda (sep s) (string-split s sep)))

(define (join : (-> String (List String) String))
  (lambda (sep parts) (string-join sep parts)))

(define (lines : (-> String (List String)))
  (lambda (s) (string-split s "\\n")))

(define (unlines : (-> (List String) String))
  (lambda (parts) (string-join "\\n" parts)))

(define (words : (-> String (List String)))
  (lambda (s) (string-split s " ")))

(define (unwords : (-> (List String) String))
  (lambda (parts) (string-join " " parts)))

(define (replace : (-> String String String String))
  (lambda (from to s) (string-replace s from to)))

(define (replace-all : (-> String String String String))
  (lambda (from to s) (string-replace-all s from to)))

(define (pad-start : (-> Int String String String))
  (lambda (len pad s) (string-pad-start s len pad)))

(define (pad-end : (-> Int String String String))
  (lambda (len pad s) (string-pad-end s len pad)))

(define (center : (-> Int String String))
  (lambda (width s)
    (let ((len (string-length s)))
      (if (>= len width)
        s
        (let ((padding (- width len))
              (left (/ padding 2))
              (right (- padding (/ padding 2))))
          (string-concat (string-repeat " " left)
                        (string-concat s (string-repeat " " right))))))))

(define (to-char-code : (-> String Int))
  (lambda (c) (char-code c)))

(define (from-char-code : (-> Int String))
  (lambda (n) (char-from-code n)))
`;

const STDLIB_MATH = `; Math utilities module
(define (pi : Float) 3.141592653589793)
(define (e : Float) 2.718281828459045)
(define (tau : Float) 6.283185307179586)

(define (min-of : (=> (Ord a) (-> a a a)))
  (lambda (x y) (if (< x y) x y)))

(define (max-of : (=> (Ord a) (-> a a a)))
  (lambda (x y) (if (> x y) x y)))

(define (clamp : (=> (Ord a) (-> a a a a)))
  (lambda (lo hi x)
    (if (< x lo) lo
      (if (> x hi) hi x))))

(define (square : (=> (Num a) (-> a a)))
  (lambda (x) (* x x)))

(define (cube : (=> (Num a) (-> a a)))
  (lambda (x) (* x (* x x))))

(define (div : (=> (Num a) (-> a a a)))
  (lambda (a b)
    (let ((r (rem a b)))
      (/ (- a r) b))))

(define (even : (-> Int Bool))
  (lambda (n) (= (rem n 2) 0)))

(define (odd : (-> Int Bool))
  (lambda (n) (if (even n) #f #t)))

(define (log-base : (-> Float Float Float))
  (lambda (base x) (/ (log x) (log base))))

(define (degrees : (-> Float Float))
  (lambda (radians) (* radians (/ 180.0 pi))))

(define (radians : (-> Float Float))
  (lambda (deg) (* deg (/ pi 180.0))))

(define (gcd : (-> Int Int Int))
  (lambda (a b)
    (if (= b 0)
      (abs a)
      (gcd b (rem a b)))))

(define (lcm : (-> Int Int Int))
  (lambda (a b)
    (if (= a 0)
      0
      (abs (* (div a (gcd a b)) b)))))

(define (factorial : (-> Int Int))
  (lambda (n)
    (if (<= n 1)
      1
      (* n (factorial (- n 1))))))
`;

const STDLIB_CONCURRENT = `;; Concurrency primitives: fibers (green threads) and channels

(effect Concurrent
  (fiber-spawn (-> (-> Unit Unit) Pid))
  (fiber-yield (-> Unit Unit))
  (fiber-self  (-> Unit Pid)))

(effect Channel
  (chan-new   (-> Int Channel))
  (chan-send  (-> Channel a Unit))
  (chan-recv  (-> Channel a))
  (chan-close (-> Channel Unit)))

(define spawn     (lambda (f) (perform fiber-spawn f)))
(define yield     (lambda () (perform fiber-yield)))
(define self      (lambda () (perform fiber-self)))
(define chan-new   (lambda (cap) (perform chan-new cap)))
(define chan-send  (lambda (ch v) (perform chan-send ch v)))
(define chan-recv  (lambda (ch) (perform chan-recv ch)))
(define chan-close (lambda (ch) (perform chan-close ch)))
(define run       (lambda (f) (run-fibers f)))
`;

const STDLIB_ACTOR = `;; Actor model: mailboxes, selective receive, supervision

(data Signal
  (Down Pid String)
  (Exit Pid String))

(effect Actor
  (actor-send          (-> Pid a Unit))
  (actor-receive       (-> Unit a))
  (actor-receive-match (-> (-> a Bool) a))
  (actor-monitor       (-> Pid Unit))
  (actor-link          (-> Pid Unit))
  (actor-unlink        (-> Pid Unit))
  (actor-trap-exit     (-> Bool Unit)))

(define send          (lambda (pid msg) (perform actor-send pid msg)))
(define receive       (lambda () (perform actor-receive)))
(define receive-match (lambda (pred) (perform actor-receive-match pred)))
(define monitor       (lambda (pid) (perform actor-monitor pid)))
(define link          (lambda (pid) (perform actor-link pid)))
(define unlink        (lambda (pid) (perform actor-unlink pid)))
(define trap-exit     (lambda (b) (perform actor-trap-exit b)))
`;

// Register virtual modules for browser stdlib
// Use a virtual path prefix that the resolver will use
const BROWSER_STDLIB_PATH = '/browser-stdlib';

registerVirtualModule(`${BROWSER_STDLIB_PATH}/option.mar`, STDLIB_OPTION);
registerVirtualModule(`${BROWSER_STDLIB_PATH}/result.mar`, STDLIB_RESULT);
registerVirtualModule(`${BROWSER_STDLIB_PATH}/io.mar`, STDLIB_IO);
registerVirtualModule(`${BROWSER_STDLIB_PATH}/list.mar`, STDLIB_LIST);
registerVirtualModule(`${BROWSER_STDLIB_PATH}/string.mar`, STDLIB_STRING);
registerVirtualModule(`${BROWSER_STDLIB_PATH}/math.mar`, STDLIB_MATH);
registerVirtualModule(`${BROWSER_STDLIB_PATH}/concurrent.mar`, STDLIB_CONCURRENT);
registerVirtualModule(`${BROWSER_STDLIB_PATH}/actor.mar`, STDLIB_ACTOR);

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

export interface StdlibModule {
  name: string;
  importPath: string;
  description: string;
  items: BuiltinItem[];
}

export interface BuiltinReference {
  builtins: BuiltinCategory[];
  dataTypes: DataTypeInfo[];
  typeClasses: TypeClassInfo[];
  effectSyntax: EffectSyntaxItem[];
  concurrency: EffectSyntaxItem[];
  stdlib: StdlibModule[];
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
    {
      name: 'Concurrency',
      items: [
        { name: 'run-fibers', type: '(() -> a) -> a', description: 'Run a closure under the fiber scheduler' },
        { name: 'error', type: 'String -> a', description: 'Raise a runtime error' },
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
  concurrency: [
    {
      name: 'Fibers',
      syntax: '(run-fibers (lambda () body))',
      description: 'Green threads with cooperative scheduling. Use fiber-spawn to create fibers, fiber-yield to yield, fiber-self to get current Pid.',
      example: '(run-fibers (lambda ()\n  (perform fiber-spawn (lambda ()\n    (perform println "child")))\n  (perform println "parent")))',
    },
    {
      name: 'Channels',
      syntax: '(perform chan-new size)',
      description: 'CSP-style channels for synchronous communication. chan-new creates a channel (0 = unbuffered), chan-send/chan-recv transfer values, chan-close closes.',
      example: '(run-fibers (lambda ()\n  (let ((ch (perform chan-new 0)))\n    (perform fiber-spawn (lambda ()\n      (perform chan-send ch 42)))\n    (perform println (perform chan-recv ch)))))',
    },
    {
      name: 'Actors',
      syntax: '(perform actor-send pid msg)',
      description: 'Erlang-style actors with mailboxes. actor-send sends a message, actor-receive blocks until a message arrives, actor-receive-match filters by predicate.',
      example: '(run-fibers (lambda ()\n  (let ((me (perform fiber-self)))\n    (perform fiber-spawn (lambda ()\n      (perform actor-send me "hello")))\n    (perform println\n      (perform actor-receive)))))',
    },
    {
      name: 'Supervision',
      syntax: '(perform actor-monitor pid)',
      description: 'Fault-tolerance via monitors and links. actor-monitor watches a fiber (delivers Down on exit), actor-link creates bidirectional links, actor-trap-exit converts linked crashes to Exit messages.',
      example: '(run-fibers (lambda ()\n  (let ((w (perform fiber-spawn\n              (lambda () (error "crash!")))))\n    (perform actor-monitor w)\n    (perform fiber-yield)\n    (perform fiber-yield)\n    (match (perform actor-receive)\n      ((Down pid reason)\n        (perform println reason))\n      (_ nil)))))',
    },
  ],
  stdlib: [
    {
      name: 'Option',
      importPath: 'std:option',
      description: 'Utilities for the built-in Option type',
      items: [
        { name: 'map', type: '(a -> b) -> Option a -> Option b', description: 'Apply a function to the value inside Some' },
        { name: 'flat-map', type: '(a -> Option b) -> Option a -> Option b', description: 'Apply a function returning Option, flatten result' },
        { name: 'unwrap-or', type: 'a -> Option a -> a', description: 'Get value or return default' },
        { name: 'unwrap-or-else', type: '(Unit -> a) -> Option a -> a', description: 'Get value or compute default lazily' },
        { name: 'is-some', type: 'Option a -> Bool', description: 'Check if Option contains a value' },
        { name: 'is-none', type: 'Option a -> Bool', description: 'Check if Option is empty' },
        { name: 'filter', type: '(a -> Bool) -> Option a -> Option a', description: 'Keep value only if predicate holds' },
        { name: 'and-then', type: 'Option b -> Option a -> Option b', description: 'Return second if first is Some' },
        { name: 'or-else', type: 'Option a -> Option a -> Option a', description: 'Return first if Some, otherwise second' },
      ],
    },
    {
      name: 'Result',
      importPath: 'std:result',
      description: 'Result type for error handling (Ok/Err)',
      items: [
        { name: 'Ok', type: 'a -> Result e a', description: 'Success constructor' },
        { name: 'Err', type: 'e -> Result e a', description: 'Error constructor' },
        { name: 'map', type: '(a -> b) -> Result e a -> Result e b', description: 'Apply function to success value' },
        { name: 'map-err', type: '(e -> f) -> Result e a -> Result f a', description: 'Apply function to error value' },
        { name: 'flat-map', type: '(a -> Result e b) -> Result e a -> Result e b', description: 'Chain computations that may fail' },
        { name: 'unwrap-or', type: 'a -> Result e a -> a', description: 'Get success value or default' },
        { name: 'is-ok', type: 'Result e a -> Bool', description: 'Check if Result is Ok' },
        { name: 'is-err', type: 'Result e a -> Bool', description: 'Check if Result is Err' },
        { name: 'ok', type: 'Result e a -> Option a', description: 'Convert to Option, discarding error' },
        { name: 'err', type: 'Result e a -> Option e', description: 'Convert to Option of error' },
      ],
    },
    {
      name: 'List',
      importPath: 'std:list',
      description: 'List utilities (map, fold, filter, etc.)',
      items: [
        { name: 'map', type: '(a -> b) -> List a -> List b', description: 'Apply function to each element' },
        { name: 'filter', type: '(a -> Bool) -> List a -> List a', description: 'Keep elements satisfying predicate' },
        { name: 'foldl', type: '(b -> a -> b) -> b -> List a -> b', description: 'Left fold (tail-recursive)' },
        { name: 'foldr', type: '(a -> b -> b) -> b -> List a -> b', description: 'Right fold' },
        { name: 'length', type: 'List a -> Int', description: 'Count elements' },
        { name: 'reverse', type: 'List a -> List a', description: 'Reverse a list' },
        { name: 'concat', type: 'List a -> List a -> List a', description: 'Append two lists' },
        { name: 'head', type: 'List a -> Option a', description: 'First element or None' },
        { name: 'tail', type: 'List a -> Option (List a)', description: 'All but first, or None' },
        { name: 'nth', type: 'Int -> List a -> Option a', description: 'Element at index (0-based)' },
        { name: 'find', type: '(a -> Bool) -> List a -> Option a', description: 'First element matching predicate' },
        { name: 'any', type: '(a -> Bool) -> List a -> Bool', description: 'True if any element matches' },
        { name: 'all', type: '(a -> Bool) -> List a -> Bool', description: 'True if all elements match' },
        { name: 'take', type: 'Int -> List a -> List a', description: 'Take first n elements' },
        { name: 'drop', type: 'Int -> List a -> List a', description: 'Drop first n elements' },
        { name: 'zip', type: 'List a -> List b -> List (Pair a b)', description: 'Pair up elements from two lists' },
      ],
    },
    {
      name: 'String',
      importPath: 'std:string',
      description: 'String manipulation utilities',
      items: [
        { name: 'length', type: 'String -> Int', description: 'String length' },
        { name: 'concat', type: 'String -> String -> String', description: 'Concatenate two strings' },
        { name: 'is-empty', type: 'String -> Bool', description: 'Check if empty' },
        { name: 'reverse', type: 'String -> String', description: 'Reverse a string' },
        { name: 'to-upper', type: 'String -> String', description: 'Convert to uppercase' },
        { name: 'to-lower', type: 'String -> String', description: 'Convert to lowercase' },
        { name: 'trim', type: 'String -> String', description: 'Remove whitespace from both ends' },
        { name: 'starts-with', type: 'String -> String -> Bool', description: 'Check if starts with prefix' },
        { name: 'ends-with', type: 'String -> String -> Bool', description: 'Check if ends with suffix' },
        { name: 'contains', type: 'String -> String -> Bool', description: 'Check if contains substring' },
        { name: 'split', type: 'String -> String -> List String', description: 'Split by separator' },
        { name: 'join', type: 'String -> List String -> String', description: 'Join with separator' },
        { name: 'replace', type: 'String -> String -> String -> String', description: 'Replace first occurrence' },
        { name: 'replace-all', type: 'String -> String -> String -> String', description: 'Replace all occurrences' },
        { name: 'substring', type: 'Int -> Int -> String -> String', description: 'Extract substring (start, end)' },
        { name: 'char-at', type: 'Int -> String -> Option String', description: 'Character at index' },
      ],
    },
    {
      name: 'Math',
      importPath: 'std:math',
      description: 'Mathematical functions and constants',
      items: [
        { name: 'pi', type: 'Float', description: '3.14159...' },
        { name: 'e', type: 'Float', description: "Euler's number (2.71828...)" },
        { name: 'tau', type: 'Float', description: '2 * pi' },
        { name: 'min-of', type: 'Ord a => a -> a -> a', description: 'Minimum of two values' },
        { name: 'max-of', type: 'Ord a => a -> a -> a', description: 'Maximum of two values' },
        { name: 'clamp', type: 'Ord a => a -> a -> a -> a', description: 'Clamp between lo and hi' },
        { name: 'square', type: 'Num a => a -> a', description: 'Square a number' },
        { name: 'cube', type: 'Num a => a -> a', description: 'Cube a number' },
        { name: 'div', type: 'Num a => a -> a -> a', description: 'Integer division' },
        { name: 'even', type: 'Int -> Bool', description: 'Check if even' },
        { name: 'odd', type: 'Int -> Bool', description: 'Check if odd' },
        { name: 'gcd', type: 'Int -> Int -> Int', description: 'Greatest common divisor' },
        { name: 'lcm', type: 'Int -> Int -> Int', description: 'Least common multiple' },
        { name: 'factorial', type: 'Int -> Int', description: 'Factorial (n!)' },
        { name: 'log-base', type: 'Float -> Float -> Float', description: 'Logarithm with arbitrary base' },
        { name: 'degrees', type: 'Float -> Float', description: 'Radians to degrees' },
        { name: 'radians', type: 'Float -> Float', description: 'Degrees to radians' },
      ],
    },
    {
      name: 'IO',
      importPath: 'std:io',
      description: 'IO effect operations (print, file, stdin)',
      items: [
        { name: 'print', type: 'Show a => a -> Unit', description: 'Output without newline' },
        { name: 'println', type: 'Show a => a -> Unit', description: 'Output with newline' },
        { name: 'read-line', type: 'Unit -> String', description: 'Read a line from stdin' },
        { name: 'read-file', type: 'String -> String', description: 'Read file contents' },
        { name: 'write-file', type: 'String -> String -> Unit', description: 'Write string to file' },
      ],
    },
    {
      name: 'Concurrent',
      importPath: 'std:concurrent',
      description: 'Fiber and channel convenience wrappers',
      items: [
        { name: 'spawn', type: '(Unit -> Unit) -> Pid', description: 'Spawn a fiber' },
        { name: 'yield', type: 'Unit -> Unit', description: 'Yield to scheduler' },
        { name: 'self', type: 'Unit -> Pid', description: 'Get current fiber Pid' },
        { name: 'chan-new', type: 'Int -> Channel', description: 'Create a channel' },
        { name: 'chan-send', type: 'Channel -> a -> Unit', description: 'Send to channel' },
        { name: 'chan-recv', type: 'Channel -> a', description: 'Receive from channel' },
        { name: 'chan-close', type: 'Channel -> Unit', description: 'Close a channel' },
        { name: 'run', type: '(Unit -> a) -> a', description: 'Run under fiber scheduler' },
      ],
    },
    {
      name: 'Actor',
      importPath: 'std:actor',
      description: 'Actor model with mailboxes and supervision',
      items: [
        { name: 'send', type: 'Pid -> a -> Unit', description: 'Send message to actor' },
        { name: 'receive', type: 'Unit -> a', description: 'Receive next message' },
        { name: 'receive-match', type: '(a -> Bool) -> a', description: 'Receive matching message' },
        { name: 'monitor', type: 'Pid -> Unit', description: 'Monitor a fiber (delivers Down)' },
        { name: 'link', type: 'Pid -> Unit', description: 'Bidirectional link' },
        { name: 'unlink', type: 'Pid -> Unit', description: 'Remove a link' },
        { name: 'trap-exit', type: 'Bool -> Unit', description: 'Convert crashes to Exit messages' },
      ],
    },
  ],
});
