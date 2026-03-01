import { test, expect, describe } from 'bun:test';
import * as Lexer from '../src/core/lexer';

// Helper to lex code and return tokens (excluding whitespace/comments)
const lex = (code: string): Lexer.Token[] => {
  return Lexer.lex(Lexer.empty(code)).filter(t => t.kind !== 'spc');
};

// Helper to get token values
const tokenValues = (code: string): (string | number | boolean)[] => {
  return lex(code).map(t => {
    if (t.kind === 'identifier') return t.value;
    if (t.kind === 'number') return t.value;
    if (t.kind === 'string') return t.value;
    return t.kind;
  });
};

describe('Lexer - Numbers', () => {
  test('integer', () => {
    const tokens = lex('42');
    expect(tokens.length).toBe(1);
    expect(tokens[0]?.kind).toBe('number');
    if (tokens[0]?.kind === 'number') {
      expect(tokens[0].value).toBe(42);
      expect(tokens[0].isFloat).toBe(false);
    }
  });

  test('float', () => {
    const tokens = lex('3.14');
    expect(tokens.length).toBe(1);
    expect(tokens[0]?.kind).toBe('number');
    if (tokens[0]?.kind === 'number') {
      expect(tokens[0].value).toBe(3.14);
      expect(tokens[0].isFloat).toBe(true);
    }
  });

  test('float with leading zero', () => {
    const tokens = lex('0.5');
    expect(tokens.length).toBe(1);
    if (tokens[0]?.kind === 'number') {
      expect(tokens[0].value).toBe(0.5);
      expect(tokens[0].isFloat).toBe(true);
    }
  });

  test('multiple numbers', () => {
    expect(tokenValues('1 2 3')).toEqual([1, 2, 3]);
  });

  test('number followed by alpha becomes identifier', () => {
    const tokens = lex('123abc');
    expect(tokens.length).toBe(1);
    expect(tokens[0]?.kind).toBe('identifier');
    if (tokens[0]?.kind === 'identifier') {
      expect(tokens[0].value).toBe('123abc');
    }
  });
});

describe('Lexer - Strings', () => {
  test('simple string', () => {
    const tokens = lex('"hello"');
    expect(tokens.length).toBe(1);
    expect(tokens[0]?.kind).toBe('string');
    if (tokens[0]?.kind === 'string') {
      expect(tokens[0].value).toBe('hello');
    }
  });

  test('string with spaces', () => {
    const tokens = lex('"hello world"');
    expect(tokens.length).toBe(1);
    if (tokens[0]?.kind === 'string') {
      expect(tokens[0].value).toBe('hello world');
    }
  });

  test('escape sequence - newline', () => {
    const tokens = lex('"line1\\nline2"');
    expect(tokens.length).toBe(1);
    if (tokens[0]?.kind === 'string') {
      expect(tokens[0].value).toBe('line1\nline2');
    }
  });

  test('escape sequence - tab', () => {
    const tokens = lex('"col1\\tcol2"');
    expect(tokens.length).toBe(1);
    if (tokens[0]?.kind === 'string') {
      expect(tokens[0].value).toBe('col1\tcol2');
    }
  });

  test('escape sequence - backslash', () => {
    const tokens = lex('"path\\\\file"');
    expect(tokens.length).toBe(1);
    if (tokens[0]?.kind === 'string') {
      expect(tokens[0].value).toBe('path\\file');
    }
  });

  test('escape sequence - quote', () => {
    const tokens = lex('"say \\"hello\\""');
    expect(tokens.length).toBe(1);
    if (tokens[0]?.kind === 'string') {
      expect(tokens[0].value).toBe('say "hello"');
    }
  });

  test('empty string', () => {
    const tokens = lex('""');
    expect(tokens.length).toBe(1);
    if (tokens[0]?.kind === 'string') {
      expect(tokens[0].value).toBe('');
    }
  });

  test('unterminated string throws error', () => {
    expect(() => lex('"hello')).toThrow('Unterminated string literal');
  });
});

describe('Lexer - Identifiers', () => {
  test('simple identifier', () => {
    expect(tokenValues('foo')).toEqual(['foo']);
  });

  test('identifier with numbers', () => {
    expect(tokenValues('foo123')).toEqual(['foo123']);
  });

  test('identifier with hyphens', () => {
    expect(tokenValues('my-function')).toEqual(['my-function']);
  });

  test('identifier with underscores', () => {
    expect(tokenValues('my_var')).toEqual(['my_var']);
  });

  test('operators as identifiers', () => {
    expect(tokenValues('+')).toEqual(['+']);
    expect(tokenValues('-')).toEqual(['-']);
    expect(tokenValues('*')).toEqual(['*']);
    expect(tokenValues('/')).toEqual(['/']);
    expect(tokenValues('<=')).toEqual(['<=']);
    expect(tokenValues('>=')).toEqual(['>=']);
  });

  test('question mark identifier', () => {
    expect(tokenValues('empty?')).toEqual(['empty?']);
  });

  test('exclamation mark identifier', () => {
    expect(tokenValues('set!')).toEqual(['set!']);
  });
});

describe('Lexer - Parentheses', () => {
  test('left paren', () => {
    const tokens = lex('(');
    expect(tokens.length).toBe(1);
    expect(tokens[0]?.kind).toBe('lparen');
  });

  test('right paren', () => {
    const tokens = lex(')');
    expect(tokens.length).toBe(1);
    expect(tokens[0]?.kind).toBe('rparen');
  });

  test('balanced parens', () => {
    const tokens = lex('(+ 1 2)');
    expect(tokens[0]?.kind).toBe('lparen');
    expect(tokens[tokens.length - 1]?.kind).toBe('rparen');
  });

  test('nested parens', () => {
    const tokens = lex('((()))');
    expect(tokens.filter(t => t.kind === 'lparen').length).toBe(3);
    expect(tokens.filter(t => t.kind === 'rparen').length).toBe(3);
  });
});

describe('Lexer - Comments', () => {
  test('comment at end of line', () => {
    const tokens = lex('42 ; this is a comment');
    expect(tokens.length).toBe(1);
    expect(tokens[0]?.kind).toBe('number');
  });

  test('comment on its own line', () => {
    const tokens = lex('; comment\n42');
    expect(tokens.length).toBe(1);
    if (tokens[0]?.kind === 'number') {
      expect(tokens[0].value).toBe(42);
    }
  });

  test('multiple comments', () => {
    const tokens = lex(`
      ; first comment
      42 ; inline
      ; last comment
    `);
    expect(tokens.length).toBe(1);
  });
});

describe('Lexer - Whitespace', () => {
  test('spaces are skipped in output', () => {
    const tokens = lex('a   b   c');
    expect(tokens.length).toBe(3);
  });

  test('tabs are handled', () => {
    const tokens = lex('a\tb\tc');
    expect(tokens.length).toBe(3);
  });

  test('newlines are handled', () => {
    const tokens = lex('a\nb\nc');
    expect(tokens.length).toBe(3);
  });
});

describe('Lexer - Source Locations', () => {
  test('token location tracking', () => {
    const tokens = lex('foo');
    expect(tokens[0]?.loc.start.line).toBe(1);
    expect(tokens[0]?.loc.start.column).toBe(1);
  });

  test('multiline location tracking', () => {
    const code = 'a\nb';
    const tokens = lex(code);
    expect(tokens[0]?.loc.start.line).toBe(1);
    expect(tokens[1]?.loc.start.line).toBe(2);
  });

  test('column tracking after whitespace', () => {
    const tokens = lex('   foo');
    expect(tokens[0]?.loc.start.column).toBe(4);
  });
});

describe('Lexer - Edge Cases', () => {
  test('empty input', () => {
    const tokens = lex('');
    expect(tokens.length).toBe(0);
  });

  test('only whitespace', () => {
    const tokens = lex('   \n\t  ');
    expect(tokens.length).toBe(0);
  });

  test('only comments', () => {
    const tokens = lex('; just a comment');
    expect(tokens.length).toBe(0);
  });

  test('EOF handling', () => {
    const lexer = Lexer.empty('42');
    const [, token] = Lexer.advance(lexer);
    expect(token.kind).toBe('number');
  });
});
