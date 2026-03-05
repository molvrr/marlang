use crate::cmd::load_and_expand;
use marlang_core::ast::{expr_to_source, strip_marks_expr};
use std::path::Path;
use std::process::ExitCode;

pub fn cmd_expand(file: &Path, clean: bool) -> ExitCode {
    let ef = match load_and_expand(file) {
        Some(ef) => ef,
        None => return ExitCode::FAILURE,
    };

    // Print only the user's expanded expressions (skip the prelude)
    for expr in &ef.expanded.exprs[ef.expanded.prelude_count..] {
        let output_expr;
        let to_print = if clean {
            output_expr = strip_marks_expr(expr, &ef.interner);
            &output_expr
        } else {
            expr
        };
        println!("{}", expr_to_source(to_print, &ef.interner));
    }

    ExitCode::SUCCESS
}
