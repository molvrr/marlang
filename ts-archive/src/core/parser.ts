import * as Lexer from './lexer';
import type { Expr, LispValue, SourceLocation } from './types';

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

// Combine two source locations to span from start of first to end of second
const spanLoc = (start: SourceLocation, end: SourceLocation): SourceLocation => ({
  start: start.start,
  end: end.end,
});

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
      expr: { kind: 'atom', value: { kind: 'number', value: token.value, isFloat: token.isFloat }, loc: token.loc }
    };
  }

  if (token.kind === 'identifier') {
    return {
      parser: advance(p),
      expr: { kind: 'atom', value: { kind: 'symbol', value: token.value }, loc: token.loc }
    };
  }

  if (token.kind === 'string') {
    return {
      parser: advance(p),
      expr: { kind: 'atom', value: { kind: 'string', value: token.value }, loc: token.loc }
    };
  }

  if (token.kind === 'lparen') {
    return parseList(advance(p), token.loc);
  }

  throw new Error(`Unexpected token: ${token.kind}`);
};

// Parse list contents (after lparen has been consumed)
// startLoc is the location of the opening paren
const parseList = (p: T, startLoc: SourceLocation): { parser: T, expr: Expr } => {
  p = skipSpc(p);
  const token = current(p);

  if (!token) {
    throw new Error('Unexpected end of input, expected )');
  }

  if (token.kind === 'rparen') {
    // Empty list - span from ( to )
    const loc = spanLoc(startLoc, token.loc);
    return { parser: advance(p), expr: { kind: 'nil', loc } };
  }

  // Parse head expression
  const { parser: afterHead, expr: head } = parseExpr(p);

  // Parse tail as rest of list
  const { parser: afterTail, expr: tail } = parseList(afterHead, startLoc);

  // List location spans from start of this list to end of tail
  const headLoc = head.loc || startLoc;
  const tailLoc = tail.loc || headLoc;
  const loc = spanLoc(startLoc, tailLoc);

  return {
    parser: afterTail,
    expr: { kind: 'list', head, tail, loc }
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
