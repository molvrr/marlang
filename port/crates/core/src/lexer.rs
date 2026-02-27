use std::sync::Arc;

use crate::intern::{Interner, Symbol};
use crate::span::Span;

#[derive(Debug, Clone)]
pub enum Token {
    LParen(Span),
    RParen(Span),
    Identifier(Symbol, Span),
    Number(f64, bool, Span), // (value, is_float, span)
    String(Arc<str>, Span),
    Quote(Symbol, Span),
    Quasiquote(Symbol, Span),
    Unquote(Symbol, Span),
    UnquoteSplicing(Symbol, Span),
    Eof(Span),
}

impl Token {
    pub fn span(&self) -> Span {
        match self {
            Token::LParen(s)
            | Token::RParen(s)
            | Token::Identifier(_, s)
            | Token::Number(_, _, s)
            | Token::String(_, s)
            | Token::Quote(_, s)
            | Token::Quasiquote(_, s)
            | Token::Unquote(_, s)
            | Token::UnquoteSplicing(_, s)
            | Token::Eof(s) => *s,
        }
    }
}

#[derive(Debug)]
pub struct LexError {
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for LexError {}

pub struct Lexer<'a> {
    source: &'a [u8],
    pos: usize,
    interner: &'a Interner,
}

impl<'a> Lexer<'a> {
    pub fn new(source: &'a str, interner: &'a Interner) -> Self {
        Lexer {
            source: source.as_bytes(),
            pos: 0,
            interner,
        }
    }

    pub fn lex_all(&mut self) -> Result<Vec<Token>, LexError> {
        let mut tokens = Vec::new();
        loop {
            let tok = self.next_token()?;
            if matches!(tok, Token::Eof(_)) {
                break;
            }
            tokens.push(tok);
        }
        Ok(tokens)
    }

    fn peek(&self) -> Option<u8> {
        self.source.get(self.pos).copied()
    }

    fn advance(&mut self) -> u8 {
        let b = self.source[self.pos];
        self.pos += 1;
        b
    }

    fn span_from(&self, start: usize) -> Span {
        Span::new(start as u32, self.pos as u32)
    }

    fn skip_whitespace_and_comments(&mut self) {
        while let Some(b) = self.peek() {
            if b == b';' {
                // Skip comment to end of line
                while let Some(b) = self.peek() {
                    if b == b'\n' {
                        break;
                    }
                    self.advance();
                }
            } else if b.is_ascii_whitespace() {
                self.advance();
            } else {
                break;
            }
        }
    }

    fn next_token(&mut self) -> Result<Token, LexError> {
        self.skip_whitespace_and_comments();

        let Some(b) = self.peek() else {
            let span = Span::new(self.pos as u32, self.pos as u32);
            return Ok(Token::Eof(span));
        };

        match b {
            b'(' => {
                let start = self.pos;
                self.advance();
                Ok(Token::LParen(self.span_from(start)))
            }
            b')' => {
                let start = self.pos;
                self.advance();
                Ok(Token::RParen(self.span_from(start)))
            }
            b'\'' => {
                let start = self.pos;
                self.advance();
                let sym = self.interner.intern("quote");
                Ok(Token::Quote(sym, self.span_from(start)))
            }
            b'`' => {
                let start = self.pos;
                self.advance();
                let sym = self.interner.intern("quasiquote");
                Ok(Token::Quasiquote(sym, self.span_from(start)))
            }
            b',' => {
                let start = self.pos;
                self.advance();
                if self.peek() == Some(b'@') {
                    self.advance();
                    let sym = self.interner.intern("unquote-splicing");
                    Ok(Token::UnquoteSplicing(sym, self.span_from(start)))
                } else {
                    let sym = self.interner.intern("unquote");
                    Ok(Token::Unquote(sym, self.span_from(start)))
                }
            }
            b'"' => self.lex_string(),
            b'0'..=b'9' => self.lex_number_or_ident(),
            _ if is_symbol_char(b) => self.lex_identifier(),
            _ => Err(LexError {
                message: format!("unexpected character: '{}'", b as char),
                span: Span::new(self.pos as u32, (self.pos + 1) as u32),
            }),
        }
    }

    fn lex_string(&mut self) -> Result<Token, LexError> {
        let start = self.pos;
        self.advance(); // consume opening quote

        let mut value = String::new();
        loop {
            let Some(b) = self.peek() else {
                return Err(LexError {
                    message: "Unterminated string literal".to_string(),
                    span: self.span_from(start),
                });
            };
            if b == b'"' {
                self.advance();
                break;
            }
            if b == b'\\' {
                self.advance();
                let Some(next) = self.peek() else {
                    return Err(LexError {
                        message: "Unterminated string literal".to_string(),
                        span: self.span_from(start),
                    });
                };
                match next {
                    b'n' => value.push('\n'),
                    b't' => value.push('\t'),
                    b'\\' => value.push('\\'),
                    b'"' => value.push('"'),
                    other => value.push(other as char),
                }
                self.advance();
            } else {
                value.push(b as char);
                self.advance();
            }
        }

        let span = self.span_from(start);
        Ok(Token::String(Arc::from(value.as_str()), span))
    }

    fn lex_number_or_ident(&mut self) -> Result<Token, LexError> {
        let start = self.pos;
        let mut text = String::new();

        // Consume digits and dots
        while let Some(b) = self.peek() {
            if b.is_ascii_digit() || b == b'.' {
                text.push(b as char);
                self.advance();
            } else {
                break;
            }
        }

        // If followed by an alphanumeric char, it's an identifier (e.g. "123abc")
        if let Some(b) = self.peek() {
            if b.is_ascii_alphanumeric() {
                while let Some(b) = self.peek() {
                    if is_symbol_char(b) {
                        text.push(b as char);
                        self.advance();
                    } else {
                        break;
                    }
                }
                let span = self.span_from(start);
                let sym = self.interner.intern(&text);
                return Ok(Token::Identifier(sym, span));
            }
        }

        let span = self.span_from(start);
        let is_float = text.contains('.');
        let value: f64 = text.parse().map_err(|_| LexError {
            message: format!("invalid number: {text}"),
            span,
        })?;
        Ok(Token::Number(value, is_float, span))
    }

    fn lex_identifier(&mut self) -> Result<Token, LexError> {
        let start = self.pos;
        let mut text = String::new();

        // Special case: '-' might be a negative number
        if self.peek() == Some(b'-') {
            // Check if next char is a digit
            if self
                .source
                .get(self.pos + 1)
                .is_some_and(|b| b.is_ascii_digit())
            {
                text.push('-');
                self.advance();
                // Now lex the number part
                while let Some(b) = self.peek() {
                    if b.is_ascii_digit() || b == b'.' {
                        text.push(b as char);
                        self.advance();
                    } else {
                        break;
                    }
                }
                // Check if followed by alpha (making it an identifier like "-3x")
                if let Some(b) = self.peek() {
                    if b.is_ascii_alphanumeric() {
                        while let Some(b) = self.peek() {
                            if is_symbol_char(b) {
                                text.push(b as char);
                                self.advance();
                            } else {
                                break;
                            }
                        }
                        let span = self.span_from(start);
                        let sym = self.interner.intern(&text);
                        return Ok(Token::Identifier(sym, span));
                    }
                }
                let span = self.span_from(start);
                let is_float = text.contains('.');
                let value: f64 = text.parse().map_err(|_| LexError {
                    message: format!("invalid number: {text}"),
                    span,
                })?;
                return Ok(Token::Number(value, is_float, span));
            }
        }

        while let Some(b) = self.peek() {
            if is_symbol_char(b) {
                text.push(b as char);
                self.advance();
            } else {
                break;
            }
        }

        let span = self.span_from(start);
        let sym = self.interner.intern(&text);
        Ok(Token::Identifier(sym, span))
    }
}

fn is_symbol_char(b: u8) -> bool {
    matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9'
        | b'+' | b'-' | b'*' | b'/' | b'<' | b'>' | b'=' | b'!' | b'?' | b'_' | b'#' | b':' | b'.')
}

/// Convenience: lex source into tokens, filtering nothing.
pub fn lex(source: &str, interner: &Interner) -> Result<Vec<Token>, LexError> {
    Lexer::new(source, interner).lex_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intern::Interner;

    fn lex_tokens(code: &str) -> Vec<Token> {
        let interner = Interner::new();
        lex(code, &interner).expect("lex error")
    }

    fn token_kinds(code: &str) -> Vec<&'static str> {
        lex_tokens(code)
            .iter()
            .map(|t| match t {
                Token::LParen(_) => "lparen",
                Token::RParen(_) => "rparen",
                Token::Identifier(_, _) => "identifier",
                Token::Number(_, _, _) => "number",
                Token::String(_, _) => "string",
                Token::Quote(_, _) => "quote",
                Token::Quasiquote(_, _) => "quasiquote",
                Token::Unquote(_, _) => "unquote",
                Token::UnquoteSplicing(_, _) => "unquote-splicing",
                Token::Eof(_) => "eof",
            })
            .collect()
    }

    // === Numbers ===

    #[test]
    fn integer() {
        let tokens = lex_tokens("42");
        assert_eq!(tokens.len(), 1);
        assert!(matches!(&tokens[0], Token::Number(v, is_float, _) if *v == 42.0 && !is_float));
    }

    #[test]
    fn float() {
        let tokens = lex_tokens("3.14");
        assert_eq!(tokens.len(), 1);
        assert!(
            matches!(&tokens[0], Token::Number(v, is_float, _) if (*v - 3.14).abs() < 1e-10 && *is_float)
        );
    }

    #[test]
    fn float_with_leading_zero() {
        let tokens = lex_tokens("0.5");
        assert_eq!(tokens.len(), 1);
        assert!(matches!(&tokens[0], Token::Number(v, is_float, _) if *v == 0.5 && *is_float));
    }

    #[test]
    fn multiple_numbers() {
        let tokens = lex_tokens("1 2 3");
        assert_eq!(tokens.len(), 3);
        assert!(matches!(&tokens[0], Token::Number(v, _, _) if *v == 1.0));
        assert!(matches!(&tokens[1], Token::Number(v, _, _) if *v == 2.0));
        assert!(matches!(&tokens[2], Token::Number(v, _, _) if *v == 3.0));
    }

    #[test]
    fn number_followed_by_alpha_becomes_identifier() {
        let interner = Interner::new();
        let tokens = lex("123abc", &interner).unwrap();
        assert_eq!(tokens.len(), 1);
        assert!(
            matches!(&tokens[0], Token::Identifier(sym, _) if interner.resolve(*sym) == "123abc")
        );
    }

    // === Strings ===

    #[test]
    fn simple_string() {
        let tokens = lex_tokens("\"hello\"");
        assert_eq!(tokens.len(), 1);
        assert!(matches!(&tokens[0], Token::String(s, _) if s.as_ref() == "hello"));
    }

    #[test]
    fn string_with_spaces() {
        let tokens = lex_tokens("\"hello world\"");
        assert_eq!(tokens.len(), 1);
        assert!(matches!(&tokens[0], Token::String(s, _) if s.as_ref() == "hello world"));
    }

    #[test]
    fn escape_newline() {
        let tokens = lex_tokens("\"line1\\nline2\"");
        assert_eq!(tokens.len(), 1);
        assert!(matches!(&tokens[0], Token::String(s, _) if s.as_ref() == "line1\nline2"));
    }

    #[test]
    fn escape_tab() {
        let tokens = lex_tokens("\"col1\\tcol2\"");
        assert_eq!(tokens.len(), 1);
        assert!(matches!(&tokens[0], Token::String(s, _) if s.as_ref() == "col1\tcol2"));
    }

    #[test]
    fn escape_backslash() {
        let tokens = lex_tokens("\"path\\\\file\"");
        assert_eq!(tokens.len(), 1);
        assert!(matches!(&tokens[0], Token::String(s, _) if s.as_ref() == "path\\file"));
    }

    #[test]
    fn escape_quote() {
        let tokens = lex_tokens("\"say \\\"hello\\\"\"");
        assert_eq!(tokens.len(), 1);
        assert!(matches!(&tokens[0], Token::String(s, _) if s.as_ref() == "say \"hello\""));
    }

    #[test]
    fn empty_string() {
        let tokens = lex_tokens("\"\"");
        assert_eq!(tokens.len(), 1);
        assert!(matches!(&tokens[0], Token::String(s, _) if s.as_ref() == ""));
    }

    #[test]
    fn unterminated_string() {
        let interner = Interner::new();
        let result = lex("\"hello", &interner);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .message
                .contains("Unterminated string literal")
        );
    }

    // === Identifiers ===

    #[test]
    fn simple_identifier() {
        let interner = Interner::new();
        let tokens = lex("foo", &interner).unwrap();
        assert_eq!(tokens.len(), 1);
        assert!(matches!(&tokens[0], Token::Identifier(sym, _) if interner.resolve(*sym) == "foo"));
    }

    #[test]
    fn identifier_with_numbers() {
        let interner = Interner::new();
        let tokens = lex("foo123", &interner).unwrap();
        assert_eq!(tokens.len(), 1);
        assert!(
            matches!(&tokens[0], Token::Identifier(sym, _) if interner.resolve(*sym) == "foo123")
        );
    }

    #[test]
    fn identifier_with_hyphens() {
        let interner = Interner::new();
        let tokens = lex("my-function", &interner).unwrap();
        assert_eq!(tokens.len(), 1);
        assert!(
            matches!(&tokens[0], Token::Identifier(sym, _) if interner.resolve(*sym) == "my-function")
        );
    }

    #[test]
    fn operators_as_identifiers() {
        let interner = Interner::new();
        for op in ["+", "-", "*", "/", "<=", ">="] {
            let tokens = lex(op, &interner).unwrap();
            assert_eq!(tokens.len(), 1);
            assert!(
                matches!(&tokens[0], Token::Identifier(sym, _) if interner.resolve(*sym) == op)
            );
        }
    }

    #[test]
    fn question_mark_identifier() {
        let interner = Interner::new();
        let tokens = lex("empty?", &interner).unwrap();
        assert!(
            matches!(&tokens[0], Token::Identifier(sym, _) if interner.resolve(*sym) == "empty?")
        );
    }

    #[test]
    fn exclamation_mark_identifier() {
        let interner = Interner::new();
        let tokens = lex("set!", &interner).unwrap();
        assert!(
            matches!(&tokens[0], Token::Identifier(sym, _) if interner.resolve(*sym) == "set!")
        );
    }

    // === Parentheses ===

    #[test]
    fn left_paren() {
        assert_eq!(token_kinds("("), vec!["lparen"]);
    }

    #[test]
    fn right_paren() {
        assert_eq!(token_kinds(")"), vec!["rparen"]);
    }

    #[test]
    fn balanced_parens() {
        let kinds = token_kinds("(+ 1 2)");
        assert_eq!(kinds[0], "lparen");
        assert_eq!(kinds[kinds.len() - 1], "rparen");
    }

    #[test]
    fn nested_parens() {
        let tokens = lex_tokens("((()))");
        let lparens = tokens
            .iter()
            .filter(|t| matches!(t, Token::LParen(_)))
            .count();
        let rparens = tokens
            .iter()
            .filter(|t| matches!(t, Token::RParen(_)))
            .count();
        assert_eq!(lparens, 3);
        assert_eq!(rparens, 3);
    }

    // === Comments ===

    #[test]
    fn comment_at_end_of_line() {
        let tokens = lex_tokens("42 ; this is a comment");
        assert_eq!(tokens.len(), 1);
        assert!(matches!(&tokens[0], Token::Number(v, _, _) if *v == 42.0));
    }

    #[test]
    fn comment_on_its_own_line() {
        let tokens = lex_tokens("; comment\n42");
        assert_eq!(tokens.len(), 1);
        assert!(matches!(&tokens[0], Token::Number(v, _, _) if *v == 42.0));
    }

    #[test]
    fn multiple_comments() {
        let tokens =
            lex_tokens("\n      ; first comment\n      42 ; inline\n      ; last comment\n    ");
        assert_eq!(tokens.len(), 1);
    }

    // === Whitespace ===

    #[test]
    fn spaces_are_skipped() {
        assert_eq!(lex_tokens("a   b   c").len(), 3);
    }

    #[test]
    fn tabs_are_handled() {
        assert_eq!(lex_tokens("a\tb\tc").len(), 3);
    }

    #[test]
    fn newlines_are_handled() {
        assert_eq!(lex_tokens("a\nb\nc").len(), 3);
    }

    // === Source Locations ===

    #[test]
    fn token_location_tracking() {
        let tokens = lex_tokens("foo");
        let span = tokens[0].span();
        assert_eq!(span.start, 0);
        assert_eq!(span.end, 3);
    }

    #[test]
    fn multiline_location_tracking() {
        // "a\nb" — 'a' at offset 0, 'b' at offset 2
        let tokens = lex_tokens("a\nb");
        assert_eq!(tokens[0].span().start, 0);
        assert_eq!(tokens[1].span().start, 2);
    }

    #[test]
    fn column_tracking_after_whitespace() {
        let tokens = lex_tokens("   foo");
        // 3 spaces then 'foo' starts at offset 3
        assert_eq!(tokens[0].span().start, 3);
    }

    // === Edge Cases ===

    #[test]
    fn empty_input() {
        assert_eq!(lex_tokens("").len(), 0);
    }

    #[test]
    fn only_whitespace() {
        assert_eq!(lex_tokens("   \n\t  ").len(), 0);
    }

    #[test]
    fn only_comments() {
        assert_eq!(lex_tokens("; just a comment").len(), 0);
    }

    // === Negative numbers ===

    #[test]
    fn negative_integer() {
        let tokens = lex_tokens("(- 3)");
        // This is lparen, -, 3, rparen — not a negative number
        assert_eq!(tokens.len(), 4);
    }

    #[test]
    fn negative_number_literal() {
        let tokens = lex_tokens("-7");
        assert_eq!(tokens.len(), 1);
        assert!(matches!(&tokens[0], Token::Number(v, _, _) if *v == -7.0));
    }
}
