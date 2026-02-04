import * as Lexer from './lexer';
import type { Expr, LispValue } from './types';

export type T = { tokens: Lexer.Token[], idx: number, exprs: Expr[] }

export const empty = (tokens: Lexer.Token[]): T => ({ tokens, idx: 0, exprs: [] });

// Peek at current token without consuming
const current = (p: T): Lexer.Token | undefined => p.tokens[p.idx];

// Advance parser index
const advance = (p: T): T => ({ ...p, idx: p.idx + 1 });

// Skip whitespace tokens
const skipSpc = (p: T): T => {
  while (current(p)?.kind === 'spc') p = advance(p);
  return p;
};

// Parse a single expression
const parseExpr = (p: T): { parser: T, expr: Expr } => {
  p = skipSpc(p);
  const token = current(p);

  if (!token) {
    return { parser: p, expr: { kind: 'nil' } };
  }

  if (token.kind === 'number') {
    return {
      parser: advance(p),
      expr: { kind: 'atom', value: { kind: 'number', value: token.value } }
    };
  }

  if (token.kind === 'identifier') {
    return {
      parser: advance(p),
      expr: { kind: 'atom', value: { kind: 'symbol', value: token.value } }
    };
  }

  if (token.kind === 'string') {
    return {
      parser: advance(p),
      expr: { kind: 'atom', value: { kind: 'string', value: token.value } }
    };
  }

  if (token.kind === 'lparen') {
    return parseList(advance(p));
  }

  throw new Error(`Unexpected token: ${token.kind}`);
};

// Parse list contents (after lparen has been consumed)
const parseList = (p: T): { parser: T, expr: Expr } => {
  p = skipSpc(p);
  const token = current(p);

  if (!token) {
    throw new Error('Unexpected end of input, expected )');
  }

  if (token.kind === 'rparen') {
    return { parser: advance(p), expr: { kind: 'nil' } };
  }

  // Parse head expression
  const { parser: afterHead, expr: head } = parseExpr(p);

  // Parse tail as rest of list
  const { parser: afterTail, expr: tail } = parseList(afterHead);

  return {
    parser: afterTail,
    expr: { kind: 'list', head, tail }
  };
};

export const parse = (parser: T): T => {
  while (parser.idx < parser.tokens.length) {
    parser = skipSpc(parser);
    if (parser.idx >= parser.tokens.length) break;

    const { parser: nextParser, expr } = parseExpr(parser);
    parser = { ...nextParser, exprs: [...nextParser.exprs, expr] };
  }
  return parser;
};
