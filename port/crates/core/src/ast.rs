use std::sync::Arc;

use crate::intern::Symbol;
use crate::span::Span;

#[derive(Debug, Clone)]
pub enum LispValue {
    Symbol(Symbol),
    Number(f64, bool), // (value, is_float)
    String(Arc<str>),
}

#[derive(Debug, Clone)]
pub enum Expr {
    Atom(LispValue, Span),
    List {
        head: Box<Expr>,
        tail: Box<Expr>,
        span: Span,
    },
    Nil(Span),
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Atom(_, s) | Expr::List { span: s, .. } | Expr::Nil(s) => *s,
        }
    }
}

/// Convert a cons-list Expr to a Vec of references.
pub fn list_to_array(expr: &Expr) -> Vec<&Expr> {
    let mut result = Vec::new();
    let mut current = expr;
    while let Expr::List { head, tail, .. } = current {
        result.push(head.as_ref());
        current = tail.as_ref();
    }
    result
}

/// Get the symbol name from an Expr::Atom(Symbol), if it is one.
pub fn get_symbol_name<'a>(
    expr: &'a Expr,
    interner: &'a crate::intern::Interner,
) -> Option<&'a str> {
    match expr {
        Expr::Atom(LispValue::Symbol(sym), _) => Some(interner.resolve(*sym)),
        _ => None,
    }
}

/// Convert an Expr back to Marlang source text.
pub fn expr_to_source(expr: &Expr, interner: &crate::intern::Interner) -> String {
    match expr {
        Expr::Atom(LispValue::Symbol(sym), _) => interner.resolve(*sym).to_string(),
        Expr::Atom(LispValue::Number(n, is_float), _) => {
            if *is_float {
                format!("{}", n)
            } else {
                let i = *n as i64;
                if (i as f64 - n).abs() < 1e-10 {
                    format!("{}", i)
                } else {
                    format!("{}", n)
                }
            }
        }
        Expr::Atom(LispValue::String(s), _) => format!("\"{}\"", s),
        Expr::Nil(_) => "()".to_string(),
        Expr::List { .. } => {
            let elems = list_to_array(expr);
            let parts: Vec<String> = elems.iter().map(|e| expr_to_source(e, interner)).collect();
            format!("({})", parts.join(" "))
        }
    }
}

/// Strip all hygiene marks (`~N` suffixes) from a symbol name.
///
/// Returns the portion of the name before the first `~` if the remainder
/// consists only of digits and `~` characters (i.e. valid mark suffixes).
pub fn strip_marks(name: &str) -> &str {
    match name.find('~') {
        Some(pos) => {
            let rest = &name[pos + 1..];
            if !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit() || b == b'~') {
                &name[..pos]
            } else {
                name
            }
        }
        None => name,
    }
}

/// Walk an expression tree and strip all hygiene marks from symbols.
pub fn strip_marks_expr(expr: &Expr, interner: &crate::intern::Interner) -> Expr {
    match expr {
        Expr::Atom(LispValue::Symbol(sym), span) => {
            let name = interner.resolve(*sym);
            let stripped = strip_marks(name);
            if stripped == name {
                expr.clone()
            } else {
                let new_sym = interner.intern(stripped);
                Expr::Atom(LispValue::Symbol(new_sym), *span)
            }
        }
        Expr::Atom(_, _) | Expr::Nil(_) => expr.clone(),
        Expr::List { head, tail, span } => Expr::List {
            head: Box::new(strip_marks_expr(head, interner)),
            tail: Box::new(strip_marks_expr(tail, interner)),
            span: *span,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intern::Interner;
    use crate::span::Span;

    fn span() -> Span {
        Span::DUMMY
    }

    fn sym(interner: &Interner, name: &str) -> Expr {
        Expr::Atom(LispValue::Symbol(interner.intern(name)), span())
    }

    fn num(n: f64) -> Expr {
        Expr::Atom(LispValue::Number(n, false), span())
    }

    fn float(n: f64) -> Expr {
        Expr::Atom(LispValue::Number(n, true), span())
    }

    fn string(s: &str) -> Expr {
        Expr::Atom(LispValue::String(Arc::from(s)), span())
    }

    fn cons(head: Expr, tail: Expr) -> Expr {
        Expr::List {
            head: Box::new(head),
            tail: Box::new(tail),
            span: span(),
        }
    }

    // --- expr_to_source ---

    #[test]
    fn source_symbol() {
        let i = Interner::new();
        assert_eq!(expr_to_source(&sym(&i, "foo"), &i), "foo");
    }

    #[test]
    fn source_integer() {
        let i = Interner::new();
        assert_eq!(expr_to_source(&num(42.0), &i), "42");
    }

    #[test]
    fn source_float() {
        let i = Interner::new();
        assert_eq!(expr_to_source(&float(3.14), &i), "3.14");
    }

    #[test]
    fn source_string() {
        let i = Interner::new();
        assert_eq!(expr_to_source(&string("hello"), &i), "\"hello\"");
    }

    #[test]
    fn source_nil() {
        let i = Interner::new();
        assert_eq!(expr_to_source(&Expr::Nil(span()), &i), "()");
    }

    #[test]
    fn source_list() {
        let i = Interner::new();
        let expr = cons(
            sym(&i, "+"),
            cons(num(1.0), cons(num(2.0), Expr::Nil(span()))),
        );
        assert_eq!(expr_to_source(&expr, &i), "(+ 1 2)");
    }

    #[test]
    fn source_nested_list() {
        let i = Interner::new();
        let inner = cons(
            sym(&i, "+"),
            cons(num(1.0), cons(num(2.0), Expr::Nil(span()))),
        );
        let outer = cons(sym(&i, "*"), cons(inner, cons(num(3.0), Expr::Nil(span()))));
        assert_eq!(expr_to_source(&outer, &i), "(* (+ 1 2) 3)");
    }

    // --- strip_marks ---

    #[test]
    fn strip_no_marks() {
        assert_eq!(strip_marks("foo"), "foo");
    }

    #[test]
    fn strip_single_mark() {
        assert_eq!(strip_marks("tmp~1"), "tmp");
    }

    #[test]
    fn strip_multiple_marks() {
        assert_eq!(strip_marks("x~1~2~3"), "x");
    }

    #[test]
    fn strip_preserves_tilde_in_name() {
        // A tilde followed by non-digits is not a hygiene mark
        assert_eq!(strip_marks("foo~bar"), "foo~bar");
    }

    // --- strip_marks_expr ---

    #[test]
    fn strip_marks_in_list() {
        let i = Interner::new();
        let expr = cons(
            sym(&i, "+~1"),
            cons(sym(&i, "x~2"), cons(sym(&i, "y"), Expr::Nil(span()))),
        );
        let cleaned = strip_marks_expr(&expr, &i);
        assert_eq!(expr_to_source(&cleaned, &i), "(+ x y)");
    }
}
