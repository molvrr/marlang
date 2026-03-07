#![no_main]

use arbitrary::{Arbitrary, Unstructured};
use libfuzzer_sys::fuzz_target;
use marlang_types::ast::*;
use marlang_types::unify::{match_types, unify, unify_effect_rows_into_subst, unify_many};

/// Maximum depth for recursive Type generation.
const MAX_DEPTH: usize = 5;

/// Maximum number of params in an Arrow type.
const MAX_PARAMS: usize = 4;

/// Maximum number of args in a Type::App.
const MAX_ARGS: usize = 3;

/// Pool of type constant names.
const TYPE_CONSTS: &[&str] = &[
    "Int",
    "Float",
    "Bool",
    "String",
    "Char",
    "Unit",
    "Option",
    "List",
    "Pair",
    "Result",
    "FFILibrary",
    "FFIStruct",
    "FFIStructDef",
    "FFIPointer",
    "Symbol",
    "Pid",
    "Channel",
];

/// Pool of type variable names.
const TYPE_VARS: &[&str] = &["a", "b", "c", "d", "t0", "t1", "t2", "t3"];

/// Pool of effect names.
const EFFECT_NAMES: &[&str] = &["IO", "Exception", "State", "Reader", "Writer"];

/// Pool of effect row variable names.
const ROW_VARS: &[&str] = &["e", "e0", "e1", "e2"];

/// Generate a random Type.
fn arbitrary_type(u: &mut Unstructured, depth: usize) -> arbitrary::Result<Type> {
    if depth >= MAX_DEPTH {
        // At max depth, only generate leaves
        let choice: u8 = u.int_in_range(0..=1)?;
        return match choice {
            0 => {
                let idx = u.choose_index(TYPE_CONSTS.len())?;
                Ok(type_const(TYPE_CONSTS[idx]))
            }
            1 => {
                let idx = u.choose_index(TYPE_VARS.len())?;
                Ok(type_var(TYPE_VARS[idx]))
            }
            _ => unreachable!(),
        };
    }

    let choice: u8 = u.int_in_range(0..=5)?;
    match choice {
        // Type constant
        0 => {
            let idx = u.choose_index(TYPE_CONSTS.len())?;
            Ok(type_const(TYPE_CONSTS[idx]))
        }
        // Type variable
        1 => {
            let idx = u.choose_index(TYPE_VARS.len())?;
            Ok(type_var(TYPE_VARS[idx]))
        }
        // Type application
        2 => {
            let constructor = arbitrary_type(u, depth + 1)?;
            let num_args = u.int_in_range(1..=MAX_ARGS)?;
            let mut args = Vec::with_capacity(num_args);
            for _ in 0..num_args {
                args.push(arbitrary_type(u, depth + 1)?);
            }
            Ok(type_app(constructor, args))
        }
        // Arrow type (non-variadic)
        3 => {
            let num_params = u.int_in_range(0..=MAX_PARAMS)?;
            let mut params = Vec::with_capacity(num_params);
            for _ in 0..num_params {
                params.push(arbitrary_type(u, depth + 1)?);
            }
            let result = arbitrary_type(u, depth + 1)?;
            let has_effects: bool = u.arbitrary()?;
            if has_effects {
                let effects = arbitrary_effect_row(u)?;
                Ok(type_arrow_with_effects(params, Some(effects), result))
            } else {
                Ok(type_arrow(params, result))
            }
        }
        // Arrow type (variadic)
        4 => {
            let num_params = u.int_in_range(0..=MAX_PARAMS)?;
            let mut params = Vec::with_capacity(num_params);
            for _ in 0..num_params {
                params.push(arbitrary_type(u, depth + 1)?);
            }
            let rest = arbitrary_type(u, depth + 1)?;
            let result = arbitrary_type(u, depth + 1)?;
            let has_effects: bool = u.arbitrary()?;
            if has_effects {
                let effects = arbitrary_effect_row(u)?;
                Ok(type_arrow_variadic_with_effects(
                    params,
                    rest,
                    Some(effects),
                    result,
                ))
            } else {
                Ok(type_arrow_variadic(params, rest, result))
            }
        }
        // Recursive (mu) type
        5 => {
            let idx = u.choose_index(TYPE_VARS.len())?;
            let var = TYPE_VARS[idx];
            let body = arbitrary_type(u, depth + 1)?;
            Ok(type_rec(var, body))
        }
        _ => unreachable!(),
    }
}

/// Generate a random EffectRow.
fn arbitrary_effect_row(u: &mut Unstructured) -> arbitrary::Result<EffectRow> {
    let is_open: bool = u.arbitrary()?;
    let num_effects = u.int_in_range(0..=3_usize)?;
    let mut effects = Vec::with_capacity(num_effects);
    for _ in 0..num_effects {
        let idx = u.choose_index(EFFECT_NAMES.len())?;
        let name = EFFECT_NAMES[idx].to_string();
        if !effects.contains(&name) {
            effects.push(name);
        }
    }

    if is_open {
        let idx = u.choose_index(ROW_VARS.len())?;
        Ok(effect_row_open(effects, ROW_VARS[idx].to_string()))
    } else {
        Ok(effect_row_closed(effects))
    }
}

/// Input for the unification fuzzer.
#[derive(Debug)]
enum UnifyInput {
    /// Unify two types.
    Unify(Type, Type),
    /// Match two types (one-directional).
    Match(Type, Type),
    /// Unify a list of type pairs.
    UnifyMany(Vec<(Type, Type)>),
    /// Unify two effect rows.
    UnifyRows(EffectRow, EffectRow),
}

impl<'a> Arbitrary<'a> for UnifyInput {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        let choice: u8 = u.int_in_range(0..=3)?;
        match choice {
            0 => {
                let t1 = arbitrary_type(u, 0)?;
                let t2 = arbitrary_type(u, 0)?;
                Ok(UnifyInput::Unify(t1, t2))
            }
            1 => {
                let t1 = arbitrary_type(u, 0)?;
                let t2 = arbitrary_type(u, 0)?;
                Ok(UnifyInput::Match(t1, t2))
            }
            2 => {
                let count = u.int_in_range(1..=5_usize)?;
                let mut pairs = Vec::with_capacity(count);
                for _ in 0..count {
                    let t1 = arbitrary_type(u, 0)?;
                    let t2 = arbitrary_type(u, 0)?;
                    pairs.push((t1, t2));
                }
                Ok(UnifyInput::UnifyMany(pairs))
            }
            3 => {
                let r1 = arbitrary_effect_row(u)?;
                let r2 = arbitrary_effect_row(u)?;
                Ok(UnifyInput::UnifyRows(r1, r2))
            }
            _ => unreachable!(),
        }
    }
}

fuzz_target!(|input: UnifyInput| {
    // Reset thread-local state before each iteration
    reset_type_var_counter();
    reset_effect_row_var_counter();
    reset_levels();

    match input {
        UnifyInput::Unify(t1, t2) => {
            // Should never panic — only Ok or Err
            let _ = unify(&t1, &t2);
        }
        UnifyInput::Match(t1, t2) => {
            let _ = match_types(&t1, &t2);
        }
        UnifyInput::UnifyMany(pairs) => {
            let _ = unify_many(&pairs);
        }
        UnifyInput::UnifyRows(r1, r2) => {
            let _ = unify_effect_rows_into_subst(&r1, &r2);
        }
    }
});
