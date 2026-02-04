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
