use crate::ast::{Expr, LispValue};
use crate::lexer::Token;
use crate::span::Span;

#[derive(Debug)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ParseError {}

pub struct Parser<'t> {
    tokens: &'t [Token],
    pos: usize,
}

impl<'t> Parser<'t> {
    pub fn new(tokens: &'t [Token]) -> Self {
        Parser { tokens, pos: 0 }
    }

    pub fn parse_all(&mut self) -> Result<Vec<Expr>, ParseError> {
        let mut exprs = Vec::new();
        while !self.at_eof() {
            exprs.push(self.parse_expr()?);
        }
        Ok(exprs)
    }

    fn at_eof(&self) -> bool {
        self.pos >= self.tokens.len()
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn advance(&mut self) -> &Token {
        let tok = &self.tokens[self.pos];
        self.pos += 1;
        tok
    }

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        let Some(tok) = self.peek() else {
            return Ok(Expr::Nil(Span::DUMMY));
        };

        match tok {
            Token::LParen(_) => self.parse_list(),
            Token::RParen(span) => Err(ParseError {
                message: "Unexpected ')'".to_string(),
                span: *span,
            }),
            Token::Quote(sym, span)
            | Token::Quasiquote(sym, span)
            | Token::Unquote(sym, span)
            | Token::UnquoteSplicing(sym, span) => {
                let sym = *sym;
                let start_span = *span;
                self.advance();
                let inner = self.parse_expr()?;
                let end_span = inner.span();
                let full_span = Span::new(start_span.start, end_span.end);
                Ok(Self::build_sugar_list(sym, inner, full_span))
            }
            Token::Number(value, is_float, span) => {
                let expr = Expr::Atom(LispValue::Number(*value, *is_float), *span);
                self.advance();
                Ok(expr)
            }
            Token::Identifier(sym, span) => {
                let expr = Expr::Atom(LispValue::Symbol(*sym), *span);
                self.advance();
                Ok(expr)
            }
            Token::String(s, span) => {
                let expr = Expr::Atom(LispValue::String(s.clone()), *span);
                self.advance();
                Ok(expr)
            }
            Token::Eof(span) => Ok(Expr::Nil(*span)),
        }
    }

    /// Build a 2-element cons list `(sym inner)` for reader sugar.
    fn build_sugar_list(sym: crate::intern::Symbol, inner: Expr, span: Span) -> Expr {
        let sym_atom = Expr::Atom(LispValue::Symbol(sym), span);
        Expr::List {
            head: Box::new(sym_atom),
            tail: Box::new(Expr::List {
                head: Box::new(inner),
                tail: Box::new(Expr::Nil(span)),
                span,
            }),
            span,
        }
    }

    fn parse_list(&mut self) -> Result<Expr, ParseError> {
        let lparen = self.advance(); // consume '('
        let start_span = lparen.span();

        let mut elements = Vec::new();
        loop {
            if self.at_eof() {
                return Err(ParseError {
                    message: "Unexpected end of input, expected )".to_string(),
                    span: start_span,
                });
            }
            if matches!(self.peek(), Some(Token::RParen(_))) {
                break;
            }
            elements.push(self.parse_expr()?);
        }

        let rparen = self.advance(); // consume ')'
        let end_span = rparen.span();
        let list_span = Span::new(start_span.start, end_span.end);

        // Empty list → Nil
        if elements.is_empty() {
            return Ok(Expr::Nil(list_span));
        }

        // Build cons-list from elements (right fold)
        let mut result = Expr::Nil(list_span);
        for elem in elements.into_iter().rev() {
            result = Expr::List {
                head: Box::new(elem),
                tail: Box::new(result),
                span: list_span,
            };
        }
        Ok(result)
    }
}

/// Convenience: parse tokens into a list of top-level expressions.
pub fn parse(tokens: &[Token]) -> Result<Vec<Expr>, ParseError> {
    Parser::new(tokens).parse_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::list_to_array;
    use crate::intern::Interner;
    use crate::lexer;

    fn parse_code(code: &str) -> (Vec<Expr>, Interner) {
        let interner = Interner::new();
        let tokens = lexer::lex(code, &interner).expect("lex error");
        let exprs = parse(&tokens).expect("parse error");
        (exprs, interner)
    }

    fn atom_value(expr: &Expr, interner: &Interner) -> Option<String> {
        match expr {
            Expr::Atom(LispValue::Symbol(sym), _) => Some(interner.resolve(*sym).to_string()),
            Expr::Atom(LispValue::Number(v, _), _) => Some(format!("{v}")),
            Expr::Atom(LispValue::String(s), _) => Some(s.to_string()),
            _ => None,
        }
    }

    fn atom_number(expr: &Expr) -> Option<f64> {
        match expr {
            Expr::Atom(LispValue::Number(v, _), _) => Some(*v),
            _ => None,
        }
    }

    // === Atoms ===

    #[test]
    fn number_atom() {
        let (exprs, _) = parse_code("42");
        assert_eq!(exprs.len(), 1);
        assert!(matches!(&exprs[0], Expr::Atom(LispValue::Number(v, _), _) if *v == 42.0));
    }

    #[test]
    fn float_atom() {
        let (exprs, _) = parse_code("3.14");
        assert_eq!(exprs.len(), 1);
        assert!(
            matches!(&exprs[0], Expr::Atom(LispValue::Number(v, _), _) if (*v - 3.14).abs() < 1e-10)
        );
    }

    #[test]
    fn symbol_atom() {
        let (exprs, interner) = parse_code("foo");
        assert_eq!(exprs.len(), 1);
        assert_eq!(atom_value(&exprs[0], &interner).unwrap(), "foo");
    }

    #[test]
    fn string_atom() {
        let (exprs, interner) = parse_code("\"hello\"");
        assert_eq!(exprs.len(), 1);
        assert_eq!(atom_value(&exprs[0], &interner).unwrap(), "hello");
    }

    #[test]
    fn multiple_atoms() {
        let (exprs, _) = parse_code("1 2 3");
        assert_eq!(exprs.len(), 3);
    }

    // === Lists ===

    #[test]
    fn empty_list() {
        let (exprs, _) = parse_code("()");
        assert_eq!(exprs.len(), 1);
        assert!(matches!(&exprs[0], Expr::Nil(_)));
    }

    #[test]
    fn simple_list() {
        let (exprs, interner) = parse_code("(a b c)");
        assert_eq!(exprs.len(), 1);
        let elements = list_to_array(&exprs[0]);
        assert_eq!(elements.len(), 3);
        assert_eq!(atom_value(elements[0], &interner).unwrap(), "a");
        assert_eq!(atom_value(elements[1], &interner).unwrap(), "b");
        assert_eq!(atom_value(elements[2], &interner).unwrap(), "c");
    }

    #[test]
    fn nested_list() {
        let (exprs, _) = parse_code("(a (b c) d)");
        assert_eq!(exprs.len(), 1);
        let outer = list_to_array(&exprs[0]);
        assert_eq!(outer.len(), 3);
        assert!(matches!(outer[1], Expr::List { .. }));
        let inner = list_to_array(outer[1]);
        assert_eq!(inner.len(), 2);
    }

    #[test]
    fn deeply_nested_list() {
        let (exprs, interner) = parse_code("(((a)))");
        let l1 = list_to_array(&exprs[0]);
        assert_eq!(l1.len(), 1);
        let l2 = list_to_array(l1[0]);
        assert_eq!(l2.len(), 1);
        let l3 = list_to_array(l2[0]);
        assert_eq!(l3.len(), 1);
        assert_eq!(atom_value(l3[0], &interner).unwrap(), "a");
    }

    #[test]
    fn list_with_numbers() {
        let (exprs, _) = parse_code("(1 2 3)");
        let elements = list_to_array(&exprs[0]);
        assert_eq!(elements.len(), 3);
        assert_eq!(atom_number(elements[0]).unwrap(), 1.0);
        assert_eq!(atom_number(elements[1]).unwrap(), 2.0);
        assert_eq!(atom_number(elements[2]).unwrap(), 3.0);
    }

    #[test]
    fn mixed_list() {
        let (exprs, interner) = parse_code("(foo 42 \"bar\")");
        let elements = list_to_array(&exprs[0]);
        assert_eq!(elements.len(), 3);
        assert_eq!(atom_value(elements[0], &interner).unwrap(), "foo");
        assert_eq!(atom_value(elements[1], &interner).unwrap(), "42");
        assert_eq!(atom_value(elements[2], &interner).unwrap(), "bar");
    }

    // === S-expressions ===

    #[test]
    fn function_call_syntax() {
        let (exprs, interner) = parse_code("(+ 1 2)");
        let elements = list_to_array(&exprs[0]);
        assert_eq!(elements.len(), 3);
        assert_eq!(atom_value(elements[0], &interner).unwrap(), "+");
    }

    #[test]
    fn define_expression() {
        let (exprs, interner) = parse_code("(define x 42)");
        let elements = list_to_array(&exprs[0]);
        assert_eq!(atom_value(elements[0], &interner).unwrap(), "define");
        assert_eq!(atom_value(elements[1], &interner).unwrap(), "x");
        assert_eq!(atom_value(elements[2], &interner).unwrap(), "42");
    }

    #[test]
    fn lambda_expression() {
        let (exprs, interner) = parse_code("(lambda (x) x)");
        let elements = list_to_array(&exprs[0]);
        assert_eq!(atom_value(elements[0], &interner).unwrap(), "lambda");
        assert!(matches!(elements[1], Expr::List { .. }));
    }

    #[test]
    fn if_expression() {
        let (exprs, interner) = parse_code("(if cond then else)");
        let elements = list_to_array(&exprs[0]);
        assert_eq!(elements.len(), 4);
        assert_eq!(atom_value(elements[0], &interner).unwrap(), "if");
    }

    #[test]
    fn let_expression() {
        let (exprs, interner) = parse_code("(let ((x 1)) x)");
        let elements = list_to_array(&exprs[0]);
        assert_eq!(atom_value(elements[0], &interner).unwrap(), "let");
    }

    // === Error Handling ===

    #[test]
    fn unbalanced_missing_close() {
        let interner = Interner::new();
        let tokens = lexer::lex("(+ 1 2", &interner).unwrap();
        assert!(parse(&tokens).is_err());
    }

    #[test]
    fn unbalanced_extra_close() {
        let interner = Interner::new();
        let tokens = lexer::lex("(+ 1 2))", &interner).unwrap();
        assert!(parse(&tokens).is_err());
    }

    #[test]
    fn nested_missing_close() {
        let interner = Interner::new();
        let tokens = lexer::lex("(a (b c)", &interner).unwrap();
        assert!(parse(&tokens).is_err());
    }

    #[test]
    fn unexpected_close_at_start() {
        let interner = Interner::new();
        let tokens = lexer::lex(")", &interner).unwrap();
        assert!(parse(&tokens).is_err());
    }

    // === Edge Cases ===

    #[test]
    fn empty_input() {
        let (exprs, _) = parse_code("");
        assert_eq!(exprs.len(), 0);
    }

    #[test]
    fn only_whitespace() {
        let (exprs, _) = parse_code("   \n\t  ");
        assert_eq!(exprs.len(), 0);
    }

    #[test]
    fn only_comments() {
        let (exprs, _) = parse_code("; just a comment");
        assert_eq!(exprs.len(), 0);
    }

    #[test]
    fn expression_after_comment() {
        let (exprs, _) = parse_code("; comment\n42");
        assert_eq!(exprs.len(), 1);
        assert_eq!(atom_number(&exprs[0]).unwrap(), 42.0);
    }

    #[test]
    fn multiple_top_level_expressions() {
        let (exprs, _) = parse_code("(define a 1) (define b 2) (+ a b)");
        assert_eq!(exprs.len(), 3);
    }

    #[test]
    fn multiline_expression() {
        let (exprs, _) = parse_code(
            "
            (define factorial
                (lambda (n)
                    (if (= n 0)
                        1
                        (* n (factorial (- n 1))))))
        ",
        );
        assert_eq!(exprs.len(), 1);
    }

    // === Source Locations ===

    #[test]
    fn atom_has_location() {
        let (exprs, _) = parse_code("foo");
        let span = exprs[0].span();
        assert_eq!(span.start, 0);
    }

    #[test]
    fn list_has_location() {
        let (exprs, _) = parse_code("(a b)");
        let span = exprs[0].span();
        assert_eq!(span.start, 0);
    }

    #[test]
    fn nested_expr_has_location() {
        let (exprs, _) = parse_code("(a (b c))");
        let elements = list_to_array(&exprs[0]);
        let span = elements[1].span();
        assert!(span.start > 0);
    }

    // === Complex Expressions ===

    #[test]
    fn data_definition() {
        let (exprs, interner) = parse_code("(data (Option a) None (Some a))");
        assert_eq!(exprs.len(), 1);
        let elements = list_to_array(&exprs[0]);
        assert_eq!(atom_value(elements[0], &interner).unwrap(), "data");
    }

    #[test]
    fn match_expression() {
        let (exprs, interner) = parse_code(
            "
            (match x
                (None 0)
                ((Some v) v))
        ",
        );
        assert_eq!(exprs.len(), 1);
        let elements = list_to_array(&exprs[0]);
        assert_eq!(atom_value(elements[0], &interner).unwrap(), "match");
    }

    #[test]
    fn type_annotated_define() {
        let (exprs, interner) = parse_code("(define (x : Int) 42)");
        assert_eq!(exprs.len(), 1);
        let elements = list_to_array(&exprs[0]);
        assert_eq!(atom_value(elements[0], &interner).unwrap(), "define");
        assert!(matches!(elements[1], Expr::List { .. }));
    }

    #[test]
    fn cond_expression() {
        let (exprs, interner) = parse_code(
            "
            (cond
                ((< x 0) \"negative\")
                ((= x 0) \"zero\")
                (else \"positive\"))
        ",
        );
        assert_eq!(exprs.len(), 1);
        let elements = list_to_array(&exprs[0]);
        assert_eq!(atom_value(elements[0], &interner).unwrap(), "cond");
        assert_eq!(elements.len(), 4); // cond + 3 clauses
    }
}
