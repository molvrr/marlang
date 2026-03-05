use crate::cmd::{CliModuleLoader, fail, load_and_expand};
use marlang_core::ast::{expr_to_source, get_symbol_name, list_to_array};
use marlang_types::ast::format_type;
use marlang_types::checker::type_check_with_modules;
use std::path::Path;
use std::process::ExitCode;

pub fn cmd_typecheck(file: &Path) -> ExitCode {
    let ef = match load_and_expand(file) {
        Some(ef) => ef,
        None => return ExitCode::FAILURE,
    };

    let type_loader = CliModuleLoader::new(&ef.loader);
    match type_check_with_modules(&ef.expanded.exprs, &ef.interner, Some(file), &type_loader) {
        Ok(output) => {
            let user_start = ef.expanded.prelude_count;
            let user_exprs = &ef.expanded.exprs[user_start..];
            let user_types = &output.types[user_start..];
            if let Some(last_expr) = user_exprs.last() {
                let elements = list_to_array(last_expr);
                let name = if !elements.is_empty()
                    && get_symbol_name(elements[0], &ef.interner) == Some("define")
                    && elements.len() >= 2
                {
                    get_symbol_name(elements[1], &ef.interner)
                } else {
                    None
                };
                match name {
                    Some(n) => {
                        let ty = output
                            .env
                            .bindings
                            .get(n)
                            .map(|s| format_type(&s.ty))
                            .unwrap_or_else(|| "Unit".to_string());
                        println!("{} : {}", n, ty);
                    }
                    None => {
                        if let Some(last_ty) = user_types.last() {
                            println!(
                                "{} : {}",
                                expr_to_source(last_expr, &ef.interner),
                                format_type(last_ty)
                            );
                        }
                    }
                }
            }
            ExitCode::SUCCESS
        }
        Err(error) => fail(&error, &ef.source, file),
    }
}
