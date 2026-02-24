use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;

use marlang_core::ast::Expr;
use marlang_core::intern::Interner;
use marlang_core::lexer::lex;
use marlang_core::parser::parse;
use marlang_types::ast::format_type;
use marlang_types::checker::{type_check_with_modules, TypeCheckResult, TypeModuleLoader};
use marlang_types::errors::{format_error, TypeError};
use marlang_vm::builtins::register_builtins;
use marlang_vm::compiler::compile_program;
use marlang_vm::debug::disassemble;
use marlang_vm::modules::{expand_macros, run_program, run_program_from_exprs, ModuleLoader};
use marlang_vm::value::format_value;
use marlang_vm::vm::VM;

#[derive(Parser)]
#[command(name = "mar", about = "The Marlang programming language", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run a Marlang file
    Run {
        /// Path to the .mar file
        file: PathBuf,
        /// Skip type checking
        #[arg(long)]
        no_typecheck: bool,
    },
    /// Start interactive REPL
    Repl,
    /// Type check a file without running
    Typecheck {
        /// Path to the .mar file
        file: PathBuf,
        /// Print inferred types for each top-level expression
        #[arg(long)]
        verbose: bool,
    },
    /// Disassemble bytecode
    Disasm {
        /// Path to the .mar file
        file: PathBuf,
    },
}

// ===== CLI Module Loader for Type Checking =====

struct CliModuleLoader {
    stdlib_dir: Option<PathBuf>,
}

impl CliModuleLoader {
    fn new(vm_loader: &ModuleLoader) -> Self {
        CliModuleLoader {
            stdlib_dir: vm_loader.stdlib_dir.clone(),
        }
    }

    fn resolve_path(&self, import_path: &str, from_file: Option<&Path>) -> Result<PathBuf, TypeError> {
        if import_path.starts_with("std:") {
            let module_name = &import_path[4..];
            let stdlib = self.stdlib_dir.as_ref().ok_or_else(|| TypeError::InternalError {
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
            let dir = from_file
                .and_then(|f| f.parent())
                .unwrap_or(Path::new("."));
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
            let dir = from_file
                .and_then(|f| f.parent())
                .unwrap_or(Path::new("."));
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

        let exprs = parse(&tokens).map_err(|e| TypeError::InternalError {
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
                message: format!("Macro expansion error in module {}: {}", resolved.display(), e),
                span: None,
            })?;

        // Return expressions WITHOUT the prelude (module's own code only)
        let module_exprs = expanded.exprs[expanded.prelude_count..].to_vec();

        Ok((module_exprs, resolved))
    }
}

// ===== Commands =====

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Command::Run { file, no_typecheck } => cmd_run(&file, no_typecheck),
        Command::Repl => cmd_repl(),
        Command::Typecheck { file, verbose } => cmd_typecheck(&file, verbose),
        Command::Disasm { file } => cmd_disasm(&file),
    }
}

fn cmd_run(file: &PathBuf, no_typecheck: bool) -> ExitCode {
    let source = match fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error reading file: {}", e);
            return ExitCode::FAILURE;
        }
    };

    let interner = Interner::new();
    let loader = ModuleLoader::new();

    // Parse
    let tokens = match lex(&source, &interner) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Lex error: {}", e);
            return ExitCode::FAILURE;
        }
    };

    let exprs = match parse(&tokens) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Parse error: {}", e);
            return ExitCode::FAILURE;
        }
    };

    // Macro expansion (before type checking)
    let expanded = match expand_macros(&interner, &exprs, Some(file.as_path()), &loader) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Macro expansion error: {}", e);
            return ExitCode::FAILURE;
        }
    };

    // Type check the expanded AST
    if !no_typecheck {
        let type_loader = CliModuleLoader::new(&loader);
        match type_check_with_modules(&expanded.exprs, &interner, Some(file.as_path()), &type_loader) {
            TypeCheckResult::Ok { .. } => {}
            TypeCheckResult::Err { error } => {
                eprintln!("Type error: {}", format_error(&error));
                return ExitCode::FAILURE;
            }
        }
    }

    // Run from expanded expressions
    let mut vm = VM::new();
    register_builtins(&mut vm);
    let mut loader = ModuleLoader::new();

    match run_program_from_exprs(
        &mut vm,
        &expanded.exprs,
        expanded.prelude_count,
        Some(file.as_path()),
        &mut loader,
        &interner,
    ) {
        Ok(value) => {
            println!("{}", format_value(&value));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("Runtime error: {}", e);
            ExitCode::FAILURE
        }
    }
}

fn cmd_typecheck(file: &PathBuf, verbose: bool) -> ExitCode {
    let source = match fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error reading file: {}", e);
            return ExitCode::FAILURE;
        }
    };

    let interner = Interner::new();
    let loader = ModuleLoader::new();

    let tokens = match lex(&source, &interner) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Lex error: {}", e);
            return ExitCode::FAILURE;
        }
    };

    let exprs = match parse(&tokens) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Parse error: {}", e);
            return ExitCode::FAILURE;
        }
    };

    // Macro expansion before type checking
    let expanded = match expand_macros(&interner, &exprs, Some(file.as_path()), &loader) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Macro expansion error: {}", e);
            return ExitCode::FAILURE;
        }
    };

    let type_loader = CliModuleLoader::new(&loader);
    match type_check_with_modules(&expanded.exprs, &interner, Some(file.as_path()), &type_loader) {
        TypeCheckResult::Ok { types, .. } => {
            if verbose {
                for (i, ty) in types.iter().enumerate() {
                    println!("expr {}: {}", i + 1, format_type(ty));
                }
            }
            println!("Type check passed.");
            ExitCode::SUCCESS
        }
        TypeCheckResult::Err { error } => {
            eprintln!("Type error: {}", format_error(&error));
            ExitCode::FAILURE
        }
    }
}

fn cmd_disasm(file: &PathBuf) -> ExitCode {
    let source = match fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error reading file: {}", e);
            return ExitCode::FAILURE;
        }
    };

    let interner = Interner::new();
    let loader = ModuleLoader::new();

    let tokens = match lex(&source, &interner) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Lex error: {}", e);
            return ExitCode::FAILURE;
        }
    };

    let exprs = match parse(&tokens) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Parse error: {}", e);
            return ExitCode::FAILURE;
        }
    };

    // Macro expansion before compilation
    let expanded = match expand_macros(&interner, &exprs, Some(file.as_path()), &loader) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Macro expansion error: {}", e);
            return ExitCode::FAILURE;
        }
    };

    let chunk = match compile_program(&expanded.exprs, &interner) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Compile error: {}", e);
            return ExitCode::FAILURE;
        }
    };

    let name = file.file_stem().and_then(|s| s.to_str());
    println!("{}", disassemble(&chunk, name));

    ExitCode::SUCCESS
}

fn cmd_repl() -> ExitCode {
    println!("Marlang REPL v{}", env!("CARGO_PKG_VERSION"));
    println!("Type .exit or Ctrl+D to exit\n");

    let mut rl = match DefaultEditor::new() {
        Ok(editor) => editor,
        Err(e) => {
            eprintln!("Failed to initialize REPL: {}", e);
            return ExitCode::FAILURE;
        }
    };

    // Try to load history
    let history_path = dirs::home_dir()
        .map(|h| h.join(".marlang_history"))
        .unwrap_or_else(|| PathBuf::from(".marlang_history"));
    let _ = rl.load_history(&history_path);

    let interner = Interner::new();
    let mut vm = VM::new();
    register_builtins(&mut vm);
    let mut loader = ModuleLoader::new();

    loop {
        match rl.readline("mar> ") {
            Ok(line) => {
                let trimmed = line.trim();

                // Handle special commands
                if trimmed == ".exit" || trimmed == "(quit)" {
                    println!("Goodbye!");
                    break;
                }

                if trimmed.is_empty() {
                    continue;
                }

                let _ = rl.add_history_entry(&line);

                // Run the expression
                match run_program(&mut vm, &line, None, &mut loader, &interner) {
                    Ok(value) => {
                        println!("=> {}", format_value(&value));
                    }
                    Err(e) => {
                        eprintln!("Error: {}", e);
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                println!("^C");
                continue;
            }
            Err(ReadlineError::Eof) => {
                println!("Goodbye!");
                break;
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                break;
            }
        }
    }

    // Save history
    let _ = rl.save_history(&history_path);

    ExitCode::SUCCESS
}
