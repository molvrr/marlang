pub mod disasm;
pub mod expand;
pub mod lsp;
pub mod repl;
pub mod run;
pub mod typecheck;

use marlang_core::ast::Expr;
use marlang_core::intern::Interner;
use marlang_core::lexer::lex;
use marlang_core::parser::parse_with_interner;
use marlang_core::span::LineMap;
use marlang_types::checker::TypeModuleLoader;
use marlang_types::errors::{TypeError, format_error_with_location};
use marlang_vm::modules::{ModuleLoader, expand_macros};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

// ===== CLI Module Loader for Type Checking =====

pub struct CliModuleLoader {
    stdlib_dir: Option<PathBuf>,
}

impl CliModuleLoader {
    pub fn new(vm_loader: &ModuleLoader) -> Self {
        CliModuleLoader {
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

impl TypeModuleLoader for CliModuleLoader {
    fn resolve_and_load(
        &self,
        import_path: &str,
        from_file: Option<&Path>,
        interner: &Interner,
    ) -> Result<(Vec<Expr>, PathBuf), TypeError> {
        let resolved = self.resolve_path(import_path, from_file)?;

        // Read source
        let source = fs::read_to_string(&resolved).map_err(|_| TypeError::InternalError {
            message: format!("Module not found: {}", resolved.display()),
            span: None,
        })?;

        // Lex + parse
        let tokens = lex(&source, interner).map_err(|e| TypeError::InternalError {
            message: format!("Lex error in module {}: {}", resolved.display(), e),
            span: None,
        })?;

        let exprs =
            parse_with_interner(&tokens, interner).map_err(|e| TypeError::InternalError {
                message: format!("Parse error in module {}: {}", resolved.display(), e),
                span: None,
            })?;

        // Macro expansion (using VM's ModuleLoader)
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

        // Return expressions WITHOUT the prelude (module's own code only)
        let module_exprs = expanded.exprs[expanded.prelude_count..].to_vec();

        Ok((module_exprs, resolved))
    }
}

// ===== Shared Pipeline =====

/// Result of the shared load-and-expand pipeline.
pub struct ExpandedFile {
    pub source: String,
    pub interner: Interner,
    pub loader: ModuleLoader,
    pub expanded: marlang_vm::modules::ExpandedProgram,
}

/// Read, lex, parse, and macro-expand a source file. Prints errors and returns None on failure.
pub fn load_and_expand(file: &Path) -> Option<ExpandedFile> {
    let source = fs::read_to_string(file)
        .map_err(|e| eprintln!("Error reading file: {}", e))
        .ok()?;
    let interner = Interner::new();
    let loader = ModuleLoader::new();
    let tokens = lex(&source, &interner)
        .map_err(|e| eprintln!("Lex error: {}", e))
        .ok()?;
    let exprs = parse_with_interner(&tokens, &interner)
        .map_err(|e| eprintln!("Parse error: {}", e))
        .ok()?;
    let expanded = expand_macros(&interner, &exprs, Some(file), &loader)
        .map_err(|e| eprintln!("Macro expansion error: {}", e))
        .ok()?;
    Some(ExpandedFile {
        source,
        interner,
        loader,
        expanded,
    })
}

pub fn fail(error: &TypeError, source: &str, file: &Path) -> ExitCode {
    let line_map = LineMap::new(source);
    let filename = file.display().to_string();
    eprintln!(
        "{}",
        format_error_with_location(error, &line_map, &filename)
    );
    ExitCode::FAILURE
}
