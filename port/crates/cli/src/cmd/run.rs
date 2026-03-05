use crate::cmd::{CliModuleLoader, fail, load_and_expand};
use marlang_core::span::LineMap;
use marlang_types::checker::type_check_with_modules;
use marlang_vm::builtins::register_builtins;
use marlang_vm::modules::{ModuleLoader, run_program_from_exprs};
use marlang_vm::vm::VM;
use std::path::Path;
use std::process::ExitCode;

pub fn cmd_run(file: &Path) -> ExitCode {
    let ef = match load_and_expand(file) {
        Some(ef) => ef,
        None => return ExitCode::FAILURE,
    };

    // Type check the expanded AST
    let type_loader = CliModuleLoader::new(&ef.loader);
    if let Err(error) =
        type_check_with_modules(&ef.expanded.exprs, &ef.interner, Some(file), &type_loader)
    {
        return fail(&error, &ef.source, file);
    }

    // Run from expanded expressions
    let mut vm = VM::new();
    register_builtins(&mut vm);
    let mut loader = ModuleLoader::new();

    match run_program_from_exprs(
        &mut vm,
        &ef.expanded.exprs,
        ef.expanded.prelude_count,
        Some(file),
        &mut loader,
        &ef.interner,
    ) {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            let line_map = LineMap::new(&ef.source);
            let filename = file.display().to_string();
            eprintln!("{}", e.format_with_location(&line_map, &filename));
            ExitCode::FAILURE
        }
    }
}
