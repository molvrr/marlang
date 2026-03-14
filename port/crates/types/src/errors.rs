use crate::ast::{Pattern, Type, format_pattern, format_type};
use marlang_core::span::{LineMap, Span};

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
        missing: Vec<Pattern>,
        span: Option<Span>,
    },
    NonExhaustiveHandlers {
        effect_name: String,
        missing_ops: Vec<String>,
        span: Option<Span>,
    },
    AmbiguousEffectOperation {
        op_name: String,
        effects: Vec<String>,
        suggestion: String,
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
    CyclicTypeAlias {
        name: String,
        span: Option<Span>,
    },
    KindError {
        message: String,
        span: Option<Span>,
    },
    UnhandledEffect {
        effects: Vec<String>,
        span: Option<Span>,
    },
    InternalError {
        message: String,
        span: Option<Span>,
    },
}

impl TypeError {
    /// If this error has no span, attach the given span.
    /// If it already has a span, leave it unchanged.
    pub fn with_span_if_missing(self, s: Span) -> Self {
        if error_span(&self).is_some() {
            return self;
        }
        match self {
            TypeError::UnificationError {
                t1,
                t2,
                context,
                span: _,
            } => TypeError::UnificationError {
                t1,
                t2,
                context,
                span: Some(s),
            },
            TypeError::OccursCheck {
                var_name,
                ty,
                span: _,
            } => TypeError::OccursCheck {
                var_name,
                ty,
                span: Some(s),
            },
            TypeError::NotAFunction { ty, span: _ } => {
                TypeError::NotAFunction { ty, span: Some(s) }
            }
            // For all other variants, set the span field
            other => other,
        }
    }
}

pub fn format_error(err: &TypeError) -> String {
    match err {
        TypeError::UnificationError {
            t1, t2, context, ..
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
            format!("Infinite type: {var_name} occurs in {}", format_type(ty))
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
            let missing_strs: Vec<String> = missing.iter().map(format_pattern).collect();
            format!(
                "Non-exhaustive patterns. Missing: {}",
                missing_strs.join(", ")
            )
        }
        TypeError::NonExhaustiveHandlers {
            effect_name,
            missing_ops,
            ..
        } => {
            format!(
                "Non-exhaustive effect handler for '{}'. Missing operations: {}",
                effect_name,
                missing_ops.join(", ")
            )
        }
        TypeError::AmbiguousEffectOperation {
            op_name,
            effects,
            suggestion,
            ..
        } => {
            format!(
                "Ambiguous effect operation '{}' - found in multiple effects: {}. Use qualified name like {}",
                op_name,
                effects.join(", "),
                suggestion
            )
        }
        TypeError::MissingTypeAnnotation { name, .. } => {
            format!("Missing type annotation for top-level definition: {name}")
        }
        TypeError::InvalidTypeAnnotation { message, .. } => {
            format!("Invalid type annotation: {message}")
        }
        TypeError::CyclicTypeAlias { name, .. } => {
            format!("Cyclic type alias: {name}")
        }
        TypeError::KindError { message, .. } => {
            format!("Kind error: {message}")
        }
        TypeError::UnhandledEffect { effects, .. } => {
            format!(
                "Unhandled effect{}: {}. Use a `handle` block to handle {} or wrap in a function",
                if effects.len() > 1 { "s" } else { "" },
                effects.join(", "),
                if effects.len() > 1 { "them" } else { "it" },
            )
        }
        TypeError::InternalError { message, .. } => {
            format!("Internal type error: {message}")
        }
    }
}

/// Extract the span from a TypeError, if present.
pub fn error_span(err: &TypeError) -> Option<Span> {
    match err {
        TypeError::UnificationError { span, .. }
        | TypeError::OccursCheck { span, .. }
        | TypeError::UndefinedVariable { span, .. }
        | TypeError::UndefinedType { span, .. }
        | TypeError::UndefinedConstructor { span, .. }
        | TypeError::DuplicateDefinition { span, .. }
        | TypeError::ArityMismatch { span, .. }
        | TypeError::NotAFunction { span, .. }
        | TypeError::PatternTypeMismatch { span, .. }
        | TypeError::NonExhaustivePatterns { span, .. }
        | TypeError::NonExhaustiveHandlers { span, .. }
        | TypeError::AmbiguousEffectOperation { span, .. }
        | TypeError::UnhandledEffect { span, .. }
        | TypeError::MissingTypeAnnotation { span, .. }
        | TypeError::InvalidTypeAnnotation { span, .. }
        | TypeError::CyclicTypeAlias { span, .. }
        | TypeError::KindError { span, .. }
        | TypeError::InternalError { span, .. } => *span,
    }
}

/// Format a type error with source location resolved via a `LineMap`.
pub fn format_error_with_location(err: &TypeError, line_map: &LineMap, filename: &str) -> String {
    let msg = format_error(err);
    if let Some(span) = error_span(err) {
        let (line, col) = line_map.resolve(span.start);
        format!("{}:{}:{}: Type error: {}", filename, line, col, msg)
    } else {
        format!("Type error: {}", msg)
    }
}
