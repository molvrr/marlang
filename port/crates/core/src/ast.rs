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
