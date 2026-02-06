use crate::ast::{format_constraint, format_pattern, format_type, Constraint, Pattern, Type};
use marlang_core::span::Span;

#[derive(Debug, Clone)]
pub enum TypeError {
    UnificationError {
        t1: Type,
        t2: Type,
        context: Option<String>,
        span: Option<Span>,
    },
    OccursCheck {
        var_name: String,
        ty: Type,
        span: Option<Span>,
    },
    UndefinedVariable {
        name: String,
        span: Option<Span>,
    },
    UndefinedType {
        name: String,
        span: Option<Span>,
    },
    UndefinedConstructor {
        name: String,
        span: Option<Span>,
    },
    UndefinedClass {
        name: String,
        span: Option<Span>,
    },
    NoInstance {
        constraint: Constraint,
        span: Option<Span>,
    },
    DuplicateDefinition {
        name: String,
        what: String,
        span: Option<Span>,
    },
    ArityMismatch {
        expected: usize,
        got: usize,
        context: String,
        span: Option<Span>,
    },
    NotAFunction {
        ty: Type,
        span: Option<Span>,
    },
    PatternTypeMismatch {
        pattern: Pattern,
        expected: Type,
        got: Type,
        span: Option<Span>,
    },
    NonExhaustivePatterns {
        missing: Vec<String>,
        span: Option<Span>,
    },
    OverlappingInstances {
        class_name: String,
        ty: Type,
        span: Option<Span>,
    },
    MissingTypeAnnotation {
        name: String,
        span: Option<Span>,
    },
    InvalidTypeAnnotation {
        message: String,
        span: Option<Span>,
    },
    AmbiguousType {
        var_name: String,
        constraints: Vec<Constraint>,
        span: Option<Span>,
    },
    CyclicTypeAlias {
        name: String,
        span: Option<Span>,
    },
    KindError {
        message: String,
        span: Option<Span>,
    },
    InternalError {
        message: String,
        span: Option<Span>,
    },
}

pub fn format_error(err: &TypeError) -> String {
    match err {
        TypeError::UnificationError {
            t1,
            t2,
            context,
            ..
        } => {
            let mut msg = format!(
                "Type mismatch: cannot unify {} with {}",
                format_type(t1),
                format_type(t2)
            );
            if let Some(ctx) = context {
                msg.push_str(&format!("\n  in {ctx}"));
            }
            msg
        }
        TypeError::OccursCheck { var_name, ty, .. } => {
            format!(
                "Infinite type: {var_name} occurs in {}",
                format_type(ty)
            )
        }
        TypeError::UndefinedVariable { name, .. } => {
            format!("Undefined variable: {name}")
        }
        TypeError::UndefinedType { name, .. } => {
            format!("Undefined type: {name}")
        }
        TypeError::UndefinedConstructor { name, .. } => {
            format!("Undefined constructor: {name}")
        }
        TypeError::UndefinedClass { name, .. } => {
            format!("Undefined typeclass: {name}")
        }
        TypeError::NoInstance { constraint, .. } => {
            format!("No instance for {}", format_constraint(constraint))
        }
        TypeError::DuplicateDefinition { name, what, .. } => {
            format!("Duplicate {what}: {name}")
        }
        TypeError::ArityMismatch {
            expected,
            got,
            context,
            ..
        } => {
            format!("{context}: expected {expected} arguments, got {got}")
        }
        TypeError::NotAFunction { ty, .. } => {
            format!("Cannot apply non-function type: {}", format_type(ty))
        }
        TypeError::PatternTypeMismatch {
            pattern,
            expected,
            got,
            ..
        } => {
            format!(
                "Pattern {} has type {}, expected {}",
                format_pattern(pattern),
                format_type(got),
                format_type(expected)
            )
        }
        TypeError::NonExhaustivePatterns { missing, .. } => {
            format!("Non-exhaustive patterns. Missing: {}", missing.join(", "))
        }
        TypeError::OverlappingInstances {
            class_name, ty, ..
        } => {
            format!(
                "Overlapping instances for {class_name} {}",
                format_type(ty)
            )
        }
        TypeError::MissingTypeAnnotation { name, .. } => {
            format!("Missing type annotation for top-level definition: {name}")
        }
        TypeError::InvalidTypeAnnotation { message, .. } => {
            format!("Invalid type annotation: {message}")
        }
        TypeError::AmbiguousType {
            var_name,
            constraints,
            ..
        } => {
            let cs: Vec<_> = constraints.iter().map(format_constraint).collect();
            format!(
                "Ambiguous type variable {var_name} with constraints: {}",
                cs.join(", ")
            )
        }
        TypeError::CyclicTypeAlias { name, .. } => {
            format!("Cyclic type alias: {name}")
        }
        TypeError::KindError { message, .. } => {
            format!("Kind error: {message}")
        }
        TypeError::InternalError { message, .. } => {
            format!("Internal type error: {message}")
        }
    }
}
