type Token = { kind: 'lparen' } | { kind: 'rparen' } | { kind: 'identifier', value: string } | { kind: 'number', value: number } | { kind: 'spc' } | { kind: 'eof' }
type Expr = { kind: 'atom', value: string } | { kind: 'list', head: Expr, tail: Expr } | { kind: 'nil' }

namespace Lexer {
  export type T = { code: string, idx: number, tokens: Token[] }
  export const empty = (code: string): T => ({ code, idx: 0, tokens: [] });

  const isChar = (input: string, char: string): boolean => input === char;
  const isSpace = (input: string): boolean => isChar(input, ' ');
  const isNumber = (input: string): boolean => !!input.match(/[0-9]/);
  const isAlphanum = (input: string): boolean => !!input.match(/[0-9A-Za-z]/)

  export const peek = (lexer: T): [T, Token] => {
    const c = lexer.code[lexer.idx];

    if (c === undefined) return [lexer, { kind: 'eof' }];
    if (isChar(c, '(')) return [lexer, { kind: 'lparen' }];
    if (isChar(c, ')')) return [lexer, { kind: 'rparen' }];
    if (isNumber(c)) {
      let num = c;
      let peeked = peek({ ...lexer, idx: lexer.idx + 1 });
      let foundId = false;

      while (peeked[1].kind !== 'spc' && (peeked[1].kind == 'identifier' || peeked[1].kind == 'number')) {
        num += peeked[1].value;

        if (peeked[1].kind === 'identifier') {
          foundId = true;
          break;
        }

        peeked = peek({ ...peeked[0], idx: peeked[0].idx + 1 });
      }

      if (!foundId) return [lexer, { kind: 'number', value: parseInt(num) }];

      // TODO: Lembrar de adicionar o token no advance;
      return [lexer, { kind: 'identifier', value: num }];
    }
    if (isSpace(c)) return [lexer, { kind: 'spc' }];
    if (isAlphanum(c)) {
      let num = c;
      let peeked = peek({ ...lexer, idx: lexer.idx + 1 });

      while (peeked[1].kind !== 'spc' && (peeked[1].kind == 'identifier' || peeked[1].kind == 'number')) {
        num += peeked[1].value;
        peeked = peek({ ...peeked[0], idx: peeked[0].idx + 1 });
      }

      return [lexer, { kind: 'identifier', value: num }];
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
    if (isNumber(c)) {
      let num = c;
      let peeked = peek({ ...lexer, idx: lexer.idx + 1 });
      let foundId = false;

      while (peeked[1].kind !== 'spc' && (peeked[1].kind == 'identifier' || peeked[1].kind == 'number')) {
        num += peeked[1].value;

        if (peeked[1].kind === 'identifier') {
          foundId = true;
          break;
        }

        peeked = peek({ ...peeked[0], idx: peeked[0].idx + 1 });
      }

      if (!foundId) {
        const token: Token = { kind: 'number', value: parseInt(num) };
        return [addToken({ ...lexer, idx: lexer.idx + num.length }, token), token];
      }

      const token: Token = { kind: 'identifier', value: num };
      return [addToken({ ...lexer, idx: lexer.idx + num.length }, token), token];
    }

    if (isSpace(c)) {
      let peeked = peek({ ...lexer, idx: lexer.idx + 1 });

      while (peeked[1].kind === 'spc') {
        peeked = peek({ ...lexer, idx: lexer.idx + 1 });
      }

      return [addToken({ ...lexer, idx: peeked[0].idx }, { kind: 'spc' }), { kind: 'spc' }];
    }

    if (isAlphanum(c)) {
      let num = c;
      let peeked = peek({ ...lexer, idx: lexer.idx + 1 });

      while (peeked[1].kind !== 'spc' && (peeked[1].kind == 'identifier' || peeked[1].kind == 'number')) {
        num += peeked[1].value;
        peeked = peek({ ...peeked[0], idx: peeked[0].idx + 1 });
      }
      const token: Token = { kind: 'identifier', value: num };

      return [addToken({ ...lexer, idx: peeked[0].idx }, token), token];
    }

    throw new Error('Could not peek');
  }
}

console.log(Lexer.advance(Lexer.empty('(1234a')));
