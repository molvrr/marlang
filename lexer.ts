
export type Token =
  { kind: 'lparen' }
  | { kind: 'rparen' }
  | { kind: 'identifier', value: string }
  | { kind: 'number', value: number }
  | { kind: 'string', value: string }
  | { kind: 'spc' }
  | { kind: 'eof' }
export type T = { code: string, idx: number, tokens: Token[] }
export const empty = (code: string): T => ({ code, idx: 0, tokens: [] });

const isChar = (input: string, char: string): boolean => input === char;
const isSpace = (input: string): boolean => isChar(input, ' ');
const isNumber = (input: string): boolean => !!input.match(/[0-9]/);
const isNumberChar = (input: string): boolean => !!input.match(/[0-9.]/);
const isAlphanum = (input: string): boolean => !!input.match(/[0-9A-Za-z]/);
const isSymbolChar = (input: string): boolean => !!input.match(/[0-9A-Za-z+\-*\/<>=!?_]/);

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

export const peek = (lexer: T): [T, Token] => {
  const c = lexer.code[lexer.idx];

  if (c === undefined) return [lexer, { kind: 'eof' }];
  if (isChar(c, '(')) return [lexer, { kind: 'lparen' }];
  if (isChar(c, ')')) return [lexer, { kind: 'rparen' }];
  if (isChar(c, '"')) {
    const { value } = parseString(lexer.code, lexer.idx + 1);
    return [lexer, { kind: 'string', value }];
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
      return [lexer, { kind: 'identifier', value: num }];
    }

    return [lexer, { kind: 'number', value: parseFloat(num) }];
  }
  if (isSpace(c)) return [lexer, { kind: 'spc' }];
  if (isSymbolChar(c)) {
    let value = c;
    let idx = lexer.idx + 1;
    let char = lexer.code[idx];

    while (char !== undefined && isSymbolChar(char)) {
      value += char;
      idx++;
      char = lexer.code[idx];
    }

    return [lexer, { kind: 'identifier', value }];
  }

  throw new Error('Could not peek');
}

const incrIdx = (lexer: T): T => ({ ...lexer, idx: lexer.idx + 1 });
const addToken = (lexer: T, token: Token): T => ({ ...lexer, tokens: [...lexer.tokens, token] });

export const advance = (lexer: T): [T, Token] => {
  const c = lexer.code[lexer.idx];

  if (c === undefined) return [lexer, { kind: 'eof' }];
  if (isChar(c, '(')) {
    const token: Token = { kind: 'lparen' };
    return [addToken(incrIdx(lexer), token), token];
  }
  if (isChar(c, ')')) {
    const token: Token = { kind: 'rparen' };
    return [addToken(incrIdx(lexer), token), token];
  }
  if (isChar(c, '"')) {
    const { value, endIdx } = parseString(lexer.code, lexer.idx + 1);
    const token: Token = { kind: 'string', value };
    return [addToken({ ...lexer, idx: endIdx }, token), token];
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
      const token: Token = { kind: 'identifier', value: num };
      return [addToken({ ...lexer, idx }, token), token];
    }

    const token: Token = { kind: 'number', value: parseFloat(num) };
    return [addToken({ ...lexer, idx }, token), token];
  }

  if (isSpace(c)) {
    let peeked = peek({ ...lexer, idx: lexer.idx + 1 });

    while (peeked[1].kind === 'spc') {
      peeked = peek(incrIdx(peeked[0]));
    }

    return [addToken({ ...lexer, idx: peeked[0].idx }, { kind: 'spc' }), { kind: 'spc' }];
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
    const token: Token = { kind: 'identifier', value };

    return [addToken({ ...lexer, idx }, token), token];
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
