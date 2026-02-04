import type { SourceLocation } from './types';

export type TokenLocation = SourceLocation;

export type Token =
  { kind: 'lparen', loc: TokenLocation }
  | { kind: 'rparen', loc: TokenLocation }
  | { kind: 'identifier', value: string, loc: TokenLocation }
  | { kind: 'number', value: number, isFloat: boolean, loc: TokenLocation }
  | { kind: 'string', value: string, loc: TokenLocation }
  | { kind: 'spc', loc: TokenLocation }
  | { kind: 'eof', loc: TokenLocation }

export type T = {
  code: string,
  idx: number,
  line: number,
  column: number,
  tokens: Token[]
}

export const empty = (code: string): T => ({
  code,
  idx: 0,
  line: 1,
  column: 1,
  tokens: []
});

// Create a source location from start to end positions
const makeLoc = (
  startLine: number, startColumn: number, startOffset: number,
  endLine: number, endColumn: number, endOffset: number
): TokenLocation => ({
  start: { line: startLine, column: startColumn, offset: startOffset },
  end: { line: endLine, column: endColumn, offset: endOffset },
});

const isChar = (input: string, char: string): boolean => input === char;
const isWhitespace = (input: string): boolean => input === ' ' || input === '\n' || input === '\r' || input === '\t';
const isNumber = (input: string): boolean => !!input.match(/[0-9]/);
const isNumberChar = (input: string): boolean => !!input.match(/[0-9.]/);
const isAlphanum = (input: string): boolean => !!input.match(/[0-9A-Za-z]/);
const isSymbolChar = (input: string): boolean => !!input.match(/[0-9A-Za-z+\-*\/<>=!?_#:]/);

// Parse a string literal starting at idx (after the opening quote)
const parseString = (code: string, startIdx: number): { value: string, endIdx: number } => {
  let value = '';
  let idx = startIdx;

  while (idx < code.length) {
    const c = code[idx];
    if (c === '"') {
      return { value, endIdx: idx + 1 };
    }
    if (c === '\\' && idx + 1 < code.length) {
      const next = code[idx + 1];
      switch (next) {
        case 'n': value += '\n'; idx += 2; continue;
        case 't': value += '\t'; idx += 2; continue;
        case '\\': value += '\\'; idx += 2; continue;
        case '"': value += '"'; idx += 2; continue;
        default: value += next; idx += 2; continue;
      }
    }
    value += c;
    idx++;
  }
  throw new Error('Unterminated string literal');
};

// Calculate line and column after advancing through text
const calculatePosition = (
  code: string,
  startIdx: number,
  endIdx: number,
  startLine: number,
  startColumn: number
): { line: number; column: number } => {
  let line = startLine;
  let column = startColumn;
  for (let i = startIdx; i < endIdx; i++) {
    if (code[i] === '\n') {
      line++;
      column = 1;
    } else {
      column++;
    }
  }
  return { line, column };
};

export const peek = (lexer: T): [T, Token] => {
  const c = lexer.code[lexer.idx];
  const startLoc = { line: lexer.line, column: lexer.column, offset: lexer.idx };

  if (c === undefined) {
    const loc = makeLoc(startLoc.line, startLoc.column, startLoc.offset, startLoc.line, startLoc.column, startLoc.offset);
    return [lexer, { kind: 'eof', loc }];
  }
  if (isChar(c, '(')) {
    const loc = makeLoc(startLoc.line, startLoc.column, startLoc.offset, startLoc.line, startLoc.column + 1, lexer.idx + 1);
    return [lexer, { kind: 'lparen', loc }];
  }
  if (isChar(c, ')')) {
    const loc = makeLoc(startLoc.line, startLoc.column, startLoc.offset, startLoc.line, startLoc.column + 1, lexer.idx + 1);
    return [lexer, { kind: 'rparen', loc }];
  }
  if (isChar(c, ';')) {
    const loc = makeLoc(startLoc.line, startLoc.column, startLoc.offset, startLoc.line, startLoc.column + 1, lexer.idx + 1);
    return [lexer, { kind: 'spc', loc }];
  }
  if (isChar(c, '"')) {
    const { value, endIdx } = parseString(lexer.code, lexer.idx + 1);
    const endPos = calculatePosition(lexer.code, lexer.idx, endIdx, lexer.line, lexer.column);
    const loc = makeLoc(startLoc.line, startLoc.column, startLoc.offset, endPos.line, endPos.column, endIdx);
    return [lexer, { kind: 'string', value, loc }];
  }
  if (isNumber(c)) {
    let num = c;
    let idx = lexer.idx + 1;
    let char = lexer.code[idx];

    while (char !== undefined && isNumberChar(char)) {
      num += char;
      idx++;
      char = lexer.code[idx];
    }

    // Check if it's followed by an alpha character (making it an identifier)
    if (char !== undefined && isAlphanum(char)) {
      while (char !== undefined && isSymbolChar(char)) {
        num += char;
        idx++;
        char = lexer.code[idx];
      }
      const endPos = calculatePosition(lexer.code, lexer.idx, idx, lexer.line, lexer.column);
      const loc = makeLoc(startLoc.line, startLoc.column, startLoc.offset, endPos.line, endPos.column, idx);
      return [lexer, { kind: 'identifier', value: num, loc }];
    }

    const endPos = calculatePosition(lexer.code, lexer.idx, idx, lexer.line, lexer.column);
    const loc = makeLoc(startLoc.line, startLoc.column, startLoc.offset, endPos.line, endPos.column, idx);
    return [lexer, { kind: 'number', value: parseFloat(num), isFloat: num.includes('.'), loc }];
  }
  if (isWhitespace(c)) {
    const loc = makeLoc(startLoc.line, startLoc.column, startLoc.offset, startLoc.line, startLoc.column + 1, lexer.idx + 1);
    return [lexer, { kind: 'spc', loc }];
  }
  if (isSymbolChar(c)) {
    let value = c;
    let idx = lexer.idx + 1;
    let char = lexer.code[idx];

    while (char !== undefined && isSymbolChar(char)) {
      value += char;
      idx++;
      char = lexer.code[idx];
    }

    const endPos = calculatePosition(lexer.code, lexer.idx, idx, lexer.line, lexer.column);
    const loc = makeLoc(startLoc.line, startLoc.column, startLoc.offset, endPos.line, endPos.column, idx);
    return [lexer, { kind: 'identifier', value, loc }];
  }

  throw new Error('Could not peek');
}

// Increment index and update line/column for a single character
const incrIdx = (lexer: T): T => {
  const c = lexer.code[lexer.idx];
  if (c === '\n') {
    return { ...lexer, idx: lexer.idx + 1, line: lexer.line + 1, column: 1 };
  }
  return { ...lexer, idx: lexer.idx + 1, column: lexer.column + 1 };
};

// Advance index to a specific position, updating line/column
const advanceToIdx = (lexer: T, newIdx: number): T => {
  let line = lexer.line;
  let column = lexer.column;
  for (let i = lexer.idx; i < newIdx; i++) {
    if (lexer.code[i] === '\n') {
      line++;
      column = 1;
    } else {
      column++;
    }
  }
  return { ...lexer, idx: newIdx, line, column };
};

const addToken = (lexer: T, token: Token): T => ({ ...lexer, tokens: [...lexer.tokens, token] });

// Skip a comment from current position to end of line
const skipComment = (code: string, startIdx: number): number => {
  let idx = startIdx;
  while (idx < code.length && code[idx] !== '\n') {
    idx++;
  }
  return idx;
};

export const advance = (lexer: T): [T, Token] => {
  const c = lexer.code[lexer.idx];
  const startLoc = { line: lexer.line, column: lexer.column, offset: lexer.idx };

  if (c === undefined) {
    const loc = makeLoc(startLoc.line, startLoc.column, startLoc.offset, startLoc.line, startLoc.column, startLoc.offset);
    return [lexer, { kind: 'eof', loc }];
  }
  if (isChar(c, '(')) {
    const loc = makeLoc(startLoc.line, startLoc.column, startLoc.offset, startLoc.line, startLoc.column + 1, lexer.idx + 1);
    const token: Token = { kind: 'lparen', loc };
    return [addToken(incrIdx(lexer), token), token];
  }
  if (isChar(c, ')')) {
    const loc = makeLoc(startLoc.line, startLoc.column, startLoc.offset, startLoc.line, startLoc.column + 1, lexer.idx + 1);
    const token: Token = { kind: 'rparen', loc };
    return [addToken(incrIdx(lexer), token), token];
  }
  if (isChar(c, ';')) {
    // Skip comment until end of line
    const endIdx = skipComment(lexer.code, lexer.idx);
    const newLexer = advanceToIdx(lexer, endIdx);
    const loc = makeLoc(startLoc.line, startLoc.column, startLoc.offset, newLexer.line, newLexer.column, endIdx);
    const token: Token = { kind: 'spc', loc };
    return [addToken(newLexer, token), token];
  }
  if (isChar(c, '"')) {
    const { value, endIdx } = parseString(lexer.code, lexer.idx + 1);
    const newLexer = advanceToIdx(lexer, endIdx);
    const loc = makeLoc(startLoc.line, startLoc.column, startLoc.offset, newLexer.line, newLexer.column, endIdx);
    const token: Token = { kind: 'string', value, loc };
    return [addToken(newLexer, token), token];
  }
  if (isNumber(c)) {
    let num = c;
    let idx = lexer.idx + 1;
    let char = lexer.code[idx];

    while (char !== undefined && isNumberChar(char)) {
      num += char;
      idx++;
      char = lexer.code[idx];
    }

    // Check if it's followed by an alpha character (making it an identifier)
    if (char !== undefined && isAlphanum(char)) {
      while (char !== undefined && isSymbolChar(char)) {
        num += char;
        idx++;
        char = lexer.code[idx];
      }
      const newLexer = advanceToIdx(lexer, idx);
      const loc = makeLoc(startLoc.line, startLoc.column, startLoc.offset, newLexer.line, newLexer.column, idx);
      const token: Token = { kind: 'identifier', value: num, loc };
      return [addToken(newLexer, token), token];
    }

    const newLexer = advanceToIdx(lexer, idx);
    const loc = makeLoc(startLoc.line, startLoc.column, startLoc.offset, newLexer.line, newLexer.column, idx);
    const token: Token = { kind: 'number', value: parseFloat(num), isFloat: num.includes('.'), loc };
    return [addToken(newLexer, token), token];
  }

  if (isWhitespace(c)) {
    const newLexer = incrIdx(lexer);
    const loc = makeLoc(startLoc.line, startLoc.column, startLoc.offset, newLexer.line, newLexer.column, newLexer.idx);
    const token: Token = { kind: 'spc', loc };
    return [addToken(newLexer, token), token];
  }

  if (isSymbolChar(c)) {
    let value = c;
    let idx = lexer.idx + 1;
    let char = lexer.code[idx];

    while (char !== undefined && isSymbolChar(char)) {
      value += char;
      idx++;
      char = lexer.code[idx];
    }
    const newLexer = advanceToIdx(lexer, idx);
    const loc = makeLoc(startLoc.line, startLoc.column, startLoc.offset, newLexer.line, newLexer.column, idx);
    const token: Token = { kind: 'identifier', value, loc };

    return [addToken(newLexer, token), token];
  }

  throw new Error('Could not peek');
}

export const lex = (lexer: T): Token[] => {
  const tokens: Token[] = [];
  let consumed = advance(lexer);

  while (consumed[1].kind !== 'eof') {
    tokens.push(consumed[1]);
    consumed = advance(consumed[0]);
  }

  return tokens;
}
