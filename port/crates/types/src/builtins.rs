use crate::ast::*;
use crate::env::*;

// ===== Helper Type Scheme Constructors =====

fn int_bin_op() -> TypeScheme {
    monotype(type_arrow(vec![t_int(), t_int()], t_int()))
}

fn int_unary_op() -> TypeScheme {
    monotype(type_arrow(vec![t_int()], t_int()))
}

fn float_bin_op() -> TypeScheme {
    monotype(type_arrow(vec![t_float(), t_float()], t_float()))
}

fn float_unary_op() -> TypeScheme {
    monotype(type_arrow(vec![t_float()], t_float()))
}

fn int_compare() -> TypeScheme {
    monotype(type_arrow(vec![t_int(), t_int()], t_bool()))
}

fn float_compare() -> TypeScheme {
    monotype(type_arrow(vec![t_float(), t_float()], t_bool()))
}

fn string_compare() -> TypeScheme {
    monotype(type_arrow(vec![t_string(), t_string()], t_bool()))
}

// ===== Create Initial Type Environment =====

pub fn create_builtin_env() -> TypeEnv {
    let mut env = empty_env();

    // Int arithmetic operators
    env = extend_env(&env, "+", int_bin_op());
    env = extend_env(&env, "-", int_bin_op());
    env = extend_env(&env, "*", int_bin_op());
    env = extend_env(&env, "/", int_bin_op());
    env = extend_env(&env, "negate", int_unary_op());

    // Float arithmetic operators
    env = extend_env(&env, "+.", float_bin_op());
    env = extend_env(&env, "-.", float_bin_op());
    env = extend_env(&env, "*.", float_bin_op());
    env = extend_env(&env, "/.", float_bin_op());
    env = extend_env(&env, "negate.", float_unary_op());

    // Int comparison operators
    env = extend_env(&env, "=", int_compare());
    env = extend_env(&env, "<", int_compare());
    env = extend_env(&env, ">", int_compare());
    env = extend_env(&env, "<=", int_compare());
    env = extend_env(&env, ">=", int_compare());

    // Float comparison operators
    env = extend_env(&env, "=.", float_compare());
    env = extend_env(&env, "<.", float_compare());
    env = extend_env(&env, ">.", float_compare());
    env = extend_env(&env, "<=.", float_compare());
    env = extend_env(&env, ">=.", float_compare());

    // String comparison operators
    env = extend_env(&env, "string=", string_compare());
    env = extend_env(&env, "string<", string_compare());
    env = extend_env(&env, "string>", string_compare());
    env = extend_env(&env, "string<=", string_compare());
    env = extend_env(&env, "string>=", string_compare());

    // Polymorphic structural equality
    env = extend_env(
        &env,
        "equal?",
        polytype(
            vec!["a"],
            type_arrow(vec![type_var("a"), type_var("a")], t_bool()),
        ),
    );

    // nil constant
    env = extend_env(&env, "nil", monotype(t_unit()));

    // list constructor
    env = extend_env(
        &env,
        "list",
        polytype(
            vec!["a"],
            type_arrow_variadic(vec![], type_var("a"), t_list(type_var("a"))),
        ),
    );

    // show — polymorphic, VM handles all types
    env = extend_env(
        &env,
        "show",
        polytype(vec!["a"], type_arrow(vec![type_var("a")], t_string())),
    );

    // FFI functions
    env = extend_env(
        &env,
        "ffi-open",
        polytype(
            vec!["a"],
            type_arrow(vec![t_string(), t_list(type_var("a"))], t_ffi_library()),
        ),
    );
    env = extend_env(
        &env,
        "ffi-call",
        polytype(
            vec!["a"],
            type_arrow(vec![t_ffi_library(), t_string()], type_var("a")),
        ),
    );
    env = extend_env(
        &env,
        "ffi-close",
        monotype(type_arrow(vec![t_ffi_library()], t_unit())),
    );
    env = extend_env(
        &env,
        "ffi-struct",
        polytype(
            vec!["a"],
            type_arrow(vec![t_string(), t_list(type_var("a"))], t_ffi_struct_def()),
        ),
    );
    env = extend_env(
        &env,
        "struct-new",
        polytype(
            vec!["a"],
            type_arrow(vec![t_ffi_struct_def(), type_var("a")], t_ffi_struct()),
        ),
    );
    env = extend_env(
        &env,
        "struct-get",
        polytype(
            vec!["a"],
            type_arrow(vec![t_ffi_struct(), t_string()], type_var("a")),
        ),
    );
    env = extend_env(
        &env,
        "struct-set",
        polytype(
            vec!["a"],
            type_arrow(vec![t_ffi_struct(), t_string(), type_var("a")], t_unit()),
        ),
    );

    // Math builtins
    env = extend_env(&env, "abs", int_unary_op());
    env = extend_env(&env, "abs.", float_unary_op());
    env = extend_env(&env, "rem", int_bin_op());
    env = extend_env(&env, "rem.", float_bin_op());
    env = extend_env(&env, "mod", int_bin_op());
    env = extend_env(&env, "mod.", float_bin_op());

    // Conversion
    env = extend_env(
        &env,
        "int->float",
        monotype(type_arrow(vec![t_int()], t_float())),
    );
    env = extend_env(
        &env,
        "floor",
        monotype(type_arrow(vec![t_float()], t_int())),
    );
    env = extend_env(&env, "ceil", monotype(type_arrow(vec![t_float()], t_int())));
    env = extend_env(
        &env,
        "round",
        monotype(type_arrow(vec![t_float()], t_int())),
    );
    env = extend_env(
        &env,
        "truncate",
        monotype(type_arrow(vec![t_float()], t_int())),
    );
    env = extend_env(
        &env,
        "pow",
        monotype(type_arrow(vec![t_float(), t_float()], t_float())),
    );
    env = extend_env(
        &env,
        "sqrt",
        monotype(type_arrow(vec![t_float()], t_float())),
    );
    env = extend_env(
        &env,
        "exp",
        monotype(type_arrow(vec![t_float()], t_float())),
    );
    env = extend_env(
        &env,
        "log",
        monotype(type_arrow(vec![t_float()], t_float())),
    );
    env = extend_env(
        &env,
        "log10",
        monotype(type_arrow(vec![t_float()], t_float())),
    );
    env = extend_env(
        &env,
        "log2",
        monotype(type_arrow(vec![t_float()], t_float())),
    );
    for name in &[
        "sin", "cos", "tan", "asin", "acos", "atan", "sinh", "cosh", "tanh",
    ] {
        env = extend_env(&env, name, monotype(type_arrow(vec![t_float()], t_float())));
    }
    env = extend_env(
        &env,
        "atan2",
        monotype(type_arrow(vec![t_float(), t_float()], t_float())),
    );
    for name in &["is-nan", "is-infinite", "is-finite"] {
        env = extend_env(&env, name, monotype(type_arrow(vec![t_float()], t_bool())));
    }

    // String builtins
    env = extend_env(
        &env,
        "string-length",
        monotype(type_arrow(vec![t_string()], t_int())),
    );
    env = extend_env(
        &env,
        "string-concat",
        monotype(type_arrow_variadic(vec![], t_string(), t_string())),
    );
    env = extend_env(
        &env,
        "string-append",
        monotype(type_arrow_variadic(vec![], t_string(), t_string())),
    );
    env = extend_env(
        &env,
        "string-is-empty",
        monotype(type_arrow(vec![t_string()], t_bool())),
    );
    env = extend_env(
        &env,
        "string-reverse",
        monotype(type_arrow(vec![t_string()], t_string())),
    );
    env = extend_env(
        &env,
        "string-repeat",
        monotype(type_arrow(vec![t_string(), t_int()], t_string())),
    );
    for name in &[
        "string-to-upper",
        "string-to-lower",
        "string-trim",
        "string-trim-start",
        "string-trim-end",
    ] {
        env = extend_env(
            &env,
            name,
            monotype(type_arrow(vec![t_string()], t_string())),
        );
    }
    for name in &["string-starts-with", "string-ends-with", "string-contains"] {
        env = extend_env(
            &env,
            name,
            monotype(type_arrow(vec![t_string(), t_string()], t_bool())),
        );
    }
    env = extend_env(
        &env,
        "string-index-of",
        monotype(type_arrow(
            vec![t_string(), t_string()],
            type_app(type_const("Option"), vec![t_int()]),
        )),
    );
    env = extend_env(
        &env,
        "string-substring",
        monotype(type_arrow(vec![t_string(), t_int(), t_int()], t_string())),
    );
    env = extend_env(
        &env,
        "string-char-at",
        monotype(type_arrow(
            vec![t_string(), t_int()],
            type_app(type_const("Option"), vec![t_string()]),
        )),
    );
    env = extend_env(
        &env,
        "string-split",
        monotype(type_arrow(vec![t_string(), t_string()], t_list(t_string()))),
    );
    env = extend_env(
        &env,
        "string-join",
        monotype(type_arrow(vec![t_string(), t_list(t_string())], t_string())),
    );
    env = extend_env(
        &env,
        "string-replace",
        monotype(type_arrow(
            vec![t_string(), t_string(), t_string()],
            t_string(),
        )),
    );
    env = extend_env(
        &env,
        "string-replace-all",
        monotype(type_arrow(
            vec![t_string(), t_string(), t_string()],
            t_string(),
        )),
    );
    env = extend_env(
        &env,
        "string-pad-start",
        monotype(type_arrow(
            vec![t_string(), t_int(), t_string()],
            t_string(),
        )),
    );
    env = extend_env(
        &env,
        "string-pad-end",
        monotype(type_arrow(
            vec![t_string(), t_int(), t_string()],
            t_string(),
        )),
    );
    env = extend_env(
        &env,
        "char-code",
        monotype(type_arrow(vec![t_string()], t_int())),
    );
    env = extend_env(
        &env,
        "char-from-code",
        monotype(type_arrow(vec![t_int()], t_string())),
    );

    // number/string conversion builtins
    env = extend_env(
        &env,
        "number->string",
        monotype(type_arrow(vec![t_float()], t_string())),
    );
    env = extend_env(
        &env,
        "__try-string->number",
        monotype(type_arrow(vec![t_string()], t_option(t_float()))),
    );

    // Concurrency builtins
    env = extend_env(
        &env,
        "run-fibers",
        polytype(
            vec!["a", "__rf_eff"],
            type_arrow(
                vec![type_arrow_with_effects(
                    vec![],
                    Some(effect_row_open(vec![], "__rf_eff".into())),
                    type_var("a"),
                )],
                type_var("a"),
            ),
        ),
    );
    env = extend_env(
        &env,
        "run-fibers-parallel",
        polytype(
            vec!["a", "__rfp_eff"],
            type_arrow(
                vec![
                    type_arrow_with_effects(
                        vec![],
                        Some(effect_row_open(vec![], "__rfp_eff".into())),
                        type_var("a"),
                    ),
                    t_int(),
                ],
                type_var("a"),
            ),
        ),
    );
    env = extend_env(
        &env,
        "error",
        polytype(vec!["a"], type_arrow(vec![t_string()], type_var("a"))),
    );

    // Concurrency effects
    let fiber_effect = EffectDeclaration {
        name: "Fiber".into(),
        type_params: vec![],
        operations: vec![
            EffectOp {
                name: "fiber-spawn".into(),
                type_params: vec!["__fiber_eff".into()],
                params: vec![type_arrow_with_effects(
                    vec![],
                    Some(effect_row_open(vec![], "__fiber_eff".into())),
                    t_unit(),
                )],
                result: t_pid(),
            },
            EffectOp {
                name: "fiber-yield".into(),
                type_params: vec![],

                params: vec![t_unit()],
                result: t_unit(),
            },
            EffectOp {
                name: "fiber-self".into(),
                type_params: vec![],

                params: vec![t_unit()],
                result: t_pid(),
            },
        ],
    };
    env = define_effect(&env, fiber_effect);

    let channel_effect = EffectDeclaration {
        name: "Channel".into(),
        type_params: vec![],
        operations: vec![
            EffectOp {
                name: "chan-new".into(),
                type_params: vec![],

                params: vec![t_int()],
                result: t_channel(),
            },
            EffectOp {
                name: "chan-send".into(),
                type_params: vec!["a".into()],

                params: vec![t_channel(), type_var("a")],
                result: type_app(type_const("Result"), vec![t_string(), t_unit()]),
            },
            EffectOp {
                name: "chan-recv".into(),
                type_params: vec!["a".into()],

                params: vec![t_channel()],
                result: t_option(type_var("a")),
            },
            EffectOp {
                name: "chan-close".into(),
                type_params: vec![],

                params: vec![t_channel()],
                result: t_unit(),
            },
        ],
    };
    env = define_effect(&env, channel_effect);

    let actor_effect = EffectDeclaration {
        name: "Actor".into(),
        type_params: vec![],
        operations: vec![
            EffectOp {
                name: "actor-send".into(),
                type_params: vec!["a".into()],

                params: vec![t_pid(), type_var("a")],
                result: t_unit(),
            },
            EffectOp {
                name: "actor-receive".into(),
                type_params: vec!["a".into()],

                params: vec![t_unit()],
                result: type_var("a"),
            },
            EffectOp {
                name: "actor-receive-match".into(),
                type_params: vec!["a".into()],

                params: vec![type_arrow(vec![type_var("a")], t_bool())],
                result: type_var("a"),
            },
            EffectOp {
                name: "actor-monitor".into(),
                type_params: vec![],

                params: vec![t_pid()],
                result: t_unit(),
            },
            EffectOp {
                name: "actor-link".into(),
                type_params: vec![],

                params: vec![t_pid()],
                result: t_unit(),
            },
            EffectOp {
                name: "actor-unlink".into(),
                type_params: vec![],

                params: vec![t_pid()],
                result: t_unit(),
            },
            EffectOp {
                name: "actor-trap-exit".into(),
                type_params: vec![],

                params: vec![t_bool()],
                result: t_unit(),
            },
        ],
    };
    env = define_effect(&env, actor_effect);

    let io_effect = EffectDeclaration {
        name: "IO".into(),
        type_params: vec![],
        operations: vec![
            EffectOp {
                name: "print".into(),
                type_params: vec!["a".into()],
                params: vec![type_var("a")],
                result: t_unit(),
            },
            EffectOp {
                name: "println".into(),
                type_params: vec!["a".into()],
                params: vec![type_var("a")],
                result: t_unit(),
            },
            EffectOp {
                name: "read-line".into(),
                type_params: vec![],

                params: vec![t_unit()],
                result: t_string(),
            },
            EffectOp {
                name: "read-file".into(),
                type_params: vec![],

                params: vec![t_string()],
                result: type_app(type_const("Result"), vec![t_string(), t_string()]),
            },
            EffectOp {
                name: "write-file".into(),
                type_params: vec![],

                params: vec![t_string(), t_string()],
                result: type_app(type_const("Result"), vec![t_string(), t_unit()]),
            },
        ],
    };
    env = define_effect(&env, io_effect);

    // Profiling builtins
    // time-it : ((-> {} a) | e) -> Pair a Float  — accepts effectful callbacks
    env = extend_env(
        &env,
        "time-it",
        polytype(
            vec!["a", "__ti_eff"],
            type_arrow(
                vec![type_arrow_with_effects(
                    vec![],
                    Some(effect_row_open(vec![], "__ti_eff".into())),
                    type_var("a"),
                )],
                type_app(type_const("Pair"), vec![type_var("a"), t_float()]),
            ),
        ),
    );

    // Macro/quote builtins
    env = extend_env(
        &env,
        "make-symbol",
        monotype(type_arrow(vec![t_string()], t_symbol())),
    );
    env = extend_env(
        &env,
        "gensym",
        monotype(type_arrow(vec![t_unit()], t_symbol())),
    );
    env = extend_env(
        &env,
        "symbol->string",
        monotype(type_arrow(vec![t_symbol()], t_string())),
    );
    env = extend_env(
        &env,
        "string->symbol",
        monotype(type_arrow(vec![t_string()], t_symbol())),
    );
    // Type predicates — polymorphic, return Bool
    for name in &["symbol?", "pair?", "nil?", "number?", "string?", "list?"] {
        env = extend_env(
            &env,
            name,
            polytype(vec!["a"], type_arrow(vec![type_var("a")], t_bool())),
        );
    }
    env
}
