// Browser entry point for Marlang
// Exports core functions for parsing, type checking, and evaluation

import * as Lexer from './lexer';
import * as Parser from './parser';
import * as Interpreter from './interpreter';
import { typeCheck, formatError, formatTypes, formatType } from './type-checker';
import { buildTypeMap, pathKey } from './type-map';
import type { Expr, RuntimeValue } from './types';
import type { TypeCheckResult } from './type-checker';
import type { TypeMapResult, NodePath, NodeTypeInfo } from './type-map';

export type { Expr, RuntimeValue, TypeCheckResult, TypeMapResult, NodePath, NodeTypeInfo };
export { buildTypeMap, pathKey };

// Parse source code into AST
export const parse = (code: string): Expr[] => {
  const tokens = Lexer.lex(Lexer.empty(code));
  const parsed = Parser.parse(Parser.empty(tokens));
  return parsed.exprs;
};

// Type check an AST
export { typeCheck, formatError, formatTypes, formatType };

// Create a fresh interpreter
export const createInterpreter = () => Interpreter.empty();

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
    typeResult = typeCheck(exprs);
    if (!typeResult.ok) {
      throw new Error(`Type error: ${formatError(typeResult.error)}`);
    }
  }

  const interp = createInterpreter();
  const { value } = evaluate(interp, exprs);

  return { value, typeResult };
};

// Re-export types for TypeScript users
export type { Type } from './type-ast';
export { formatScheme } from './type-ast';

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

export interface BuiltinReference {
  builtins: BuiltinCategory[];
  dataTypes: DataTypeInfo[];
  typeClasses: TypeClassInfo[];
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
});
