use marlang_core::ast::Expr;
use marlang_core::intern::Interner;
use marlang_core::lexer::{LexError, lex};
use marlang_core::parser::{ParseError, parse_with_interner};
use marlang_core::span::{LineMap, Span};
use marlang_types::checker::{TypeModuleLoader, type_check_with_context};
use marlang_types::errors::{TypeError, error_span, format_error};
use marlang_types::infer::InferContext;
use marlang_vm::errors::VMError;
use marlang_vm::modules::{ModuleLoader, expand_macros};
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};

// ===== Span-to-LSP conversion =====

fn span_to_range(line_map: &LineMap, span: Span) -> Range {
    let (start_line, start_col) = line_map.resolve(span.start);
    let (end_line, end_col) = line_map.resolve(span.end);
    Range {
        start: Position {
            line: start_line.saturating_sub(1),
            character: start_col.saturating_sub(1),
        },
        end: Position {
            line: end_line.saturating_sub(1),
            character: end_col.saturating_sub(1),
        },
    }
}

/// A zero-width range at the start of the document, used when no span is available.
fn document_start_range() -> Range {
    Range {
        start: Position {
            line: 0,
            character: 0,
        },
        end: Position {
            line: 0,
            character: 0,
        },
    }
}

fn make_diagnostic(range: Range, message: String, source: &str) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some(source.to_string()),
        message,
        ..Default::default()
    }
}

// ===== Error-to-Diagnostic conversions =====

fn lex_error_to_diagnostic(line_map: &LineMap, err: &LexError) -> Diagnostic {
    let range = span_to_range(line_map, err.span);
    make_diagnostic(range, err.message.clone(), "lex")
}

fn parse_error_to_diagnostic(line_map: &LineMap, err: &ParseError) -> Diagnostic {
    let range = span_to_range(line_map, err.span);
    make_diagnostic(range, err.message.clone(), "parse")
}

fn vm_error_to_diagnostic(line_map: &LineMap, err: &VMError) -> Diagnostic {
    let range = if err.line > 0 {
        // VMError.line is a byte offset, not a line number
        let span = Span::new(err.line, err.line);
        span_to_range(line_map, span)
    } else {
        document_start_range()
    };
    make_diagnostic(range, err.message.clone(), "macro")
}

fn type_error_to_diagnostic(line_map: &LineMap, err: &TypeError) -> Diagnostic {
    let range = match error_span(err) {
        Some(span) => span_to_range(line_map, span),
        None => document_start_range(),
    };
    make_diagnostic(range, format_error(err), "type")
}

// ===== Module loader (adapted from CliModuleLoader) =====

struct LspModuleLoader {
    stdlib_dir: Option<PathBuf>,
}

impl LspModuleLoader {
    fn new(vm_loader: &ModuleLoader) -> Self {
        LspModuleLoader {
            stdlib_dir: vm_loader.stdlib_dir.clone(),
        }
    }

    #[allow(clippy::result_large_err)]
    fn resolve_path(
        &self,
        import_path: &str,
        from_file: Option<&Path>,
    ) -> Result<PathBuf, TypeError> {
        if let Some(module_name) = import_path.strip_prefix("std:") {
            let stdlib = self
                .stdlib_dir
                .as_ref()
                .ok_or_else(|| TypeError::InternalError {
                    message: format!(
                        "Cannot resolve stdlib module '{}': stdlib directory not found",
                        module_name
                    ),
                    span: None,
                })?;
            let mut path = stdlib.join(module_name);
            if path.extension().is_none() {
                path.set_extension("mar");
            }
            Ok(path)
        } else if import_path.starts_with("./") || import_path.starts_with("../") {
            let dir = from_file.and_then(|f| f.parent()).unwrap_or(Path::new("."));
            let mut path = dir.join(import_path);
            if path.extension().is_none() {
                path.set_extension("mar");
            }
            if let Ok(canonical) = path.canonicalize() {
                Ok(canonical)
            } else {
                Ok(path)
            }
        } else {
            let dir = from_file.and_then(|f| f.parent()).unwrap_or(Path::new("."));
            let mut path = dir.join(import_path);
            if path.extension().is_none() {
                path.set_extension("mar");
            }
            if let Ok(canonical) = path.canonicalize() {
                Ok(canonical)
            } else {
                Ok(path)
            }
        }
    }
}

impl TypeModuleLoader for LspModuleLoader {
    fn resolve_and_load(
        &self,
        import_path: &str,
        from_file: Option<&Path>,
        interner: &Interner,
    ) -> Result<(Vec<Expr>, PathBuf), TypeError> {
        let resolved = self.resolve_path(import_path, from_file)?;

        let source = fs::read_to_string(&resolved).map_err(|_| TypeError::InternalError {
            message: format!("Module not found: {}", resolved.display()),
            span: None,
        })?;

        let tokens = lex(&source, interner).map_err(|e| TypeError::InternalError {
            message: format!("Lex error in module {}: {}", resolved.display(), e),
            span: None,
        })?;

        let exprs =
            parse_with_interner(&tokens, interner).map_err(|e| TypeError::InternalError {
                message: format!("Parse error in module {}: {}", resolved.display(), e),
                span: None,
            })?;

        let vm_loader = match &self.stdlib_dir {
            Some(dir) => ModuleLoader::with_stdlib_dir(dir.clone()),
            None => ModuleLoader::new(),
        };

        let expanded = expand_macros(interner, &exprs, Some(resolved.as_path()), &vm_loader)
            .map_err(|e| TypeError::InternalError {
                message: format!(
                    "Macro expansion error in module {}: {}",
                    resolved.display(),
                    e
                ),
                span: None,
            })?;

        let module_exprs = expanded.exprs[expanded.prelude_count..].to_vec();
        Ok((module_exprs, resolved))
    }
}

// ===== Check Result =====

/// The result of a full document check, including diagnostics and optional context.
pub struct CheckResult {
    /// LSP diagnostics (errors) found during checking.
    pub diagnostics: Vec<Diagnostic>,
    /// InferContext populated during type checking (only when type check succeeds).
    /// Contains type_map, definitions, and references for hover/go-to-definition.
    pub context: Option<InferContext>,
}

// ===== Main check pipeline =====

/// Run the full diagnostics pipeline on a document's source text.
///
/// Returns a `CheckResult` with diagnostics and, on success, the `InferContext`
/// for hover and go-to-definition.
pub fn check_document(source: &str, file_path: Option<&Path>) -> CheckResult {
    let line_map = LineMap::new(source);
    let interner = Interner::new();
    let loader = ModuleLoader::new();

    // Stage 1: Lex
    let tokens = match lex(source, &interner) {
        Ok(t) => t,
        Err(e) => {
            return CheckResult {
                diagnostics: vec![lex_error_to_diagnostic(&line_map, &e)],
                context: None,
            };
        }
    };

    // Stage 2: Parse
    let exprs = match parse_with_interner(&tokens, &interner) {
        Ok(e) => e,
        Err(e) => {
            return CheckResult {
                diagnostics: vec![parse_error_to_diagnostic(&line_map, &e)],
                context: None,
            };
        }
    };

    // Stage 3: Macro expand
    let expanded = match expand_macros(&interner, &exprs, file_path, &loader) {
        Ok(e) => e,
        Err(e) => {
            return CheckResult {
                diagnostics: vec![vm_error_to_diagnostic(&line_map, &e)],
                context: None,
            };
        }
    };

    // Stage 4: Type check with context collection
    let context = RefCell::new(InferContext::new());
    let type_loader = LspModuleLoader::new(&loader);
    match type_check_with_context(
        &expanded.exprs,
        &interner,
        file_path,
        &type_loader,
        &context,
    ) {
        Ok(_) => {
            let context = context.into_inner();
            CheckResult {
                diagnostics: vec![],
                context: Some(context),
            }
        }
        Err(e) => {
            // Even on type error, we have partial context from before the error
            let context = context.into_inner();
            CheckResult {
                diagnostics: vec![type_error_to_diagnostic(&line_map, &e)],
                context: Some(context),
            }
        }
    }
}
