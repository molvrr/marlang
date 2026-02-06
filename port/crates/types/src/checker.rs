use marlang_core::ast::Expr;
use marlang_core::intern::Interner;

use crate::ast::{reset_effect_row_var_counter, reset_type_var_counter, Type};
use crate::builtins::create_builtin_env;
use crate::env::TypeEnv;
use crate::errors::TypeError;
use crate::infer::infer_program;

pub enum TypeCheckResult {
    Ok { env: TypeEnv, types: Vec<Type> },
    Err { error: TypeError },
}

pub fn type_check(exprs: &[Expr], interner: &Interner) -> TypeCheckResult {
    reset_type_var_counter();
    reset_effect_row_var_counter();

    let env = create_builtin_env();

    match infer_program(&env, exprs, interner) {
        Ok((env, types)) => TypeCheckResult::Ok { env, types },
        Err(error) => TypeCheckResult::Err { error },
    }
}
