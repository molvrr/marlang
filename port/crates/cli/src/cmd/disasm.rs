use crate::cmd::load_and_expand;
use marlang_core::span::LineMap;
use marlang_vm::compiler::compile_program;
use marlang_vm::debug::disassemble;
use std::path::Path;
use std::process::ExitCode;

pub fn cmd_disasm(file: &Path) -> ExitCode {
    let ef = match load_and_expand(file) {
        Some(ef) => ef,
        None => return ExitCode::FAILURE,
    };

    let chunk = match compile_program(&ef.expanded.exprs, &ef.interner) {
        Ok(c) => c,
        Err(e) => {
            let line_map = LineMap::new(&ef.source);
            let filename = file.display().to_string();
            eprintln!("{}", e.format_with_location(&line_map, &filename));
            return ExitCode::FAILURE;
        }
    };

    let name = file.file_stem().and_then(|s| s.to_str());
    println!("{}", disassemble(&chunk, name));

    ExitCode::SUCCESS
}
