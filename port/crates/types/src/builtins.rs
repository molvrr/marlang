use crate::ast::*;
use crate::env::*;

// ===== Built-in Typeclasses =====

fn num_class() -> TypeClass {
    TypeClass {
        name: "Num".into(),
        type_param: "a".into(),
        superclasses: vec![],
        methods: vec![
            (
                "+".into(),
                type_arrow(vec![type_var("a"), type_var("a")], type_var("a")),
            ),
            (
                "-".into(),
                type_arrow(vec![type_var("a"), type_var("a")], type_var("a")),
            ),
            (
                "*".into(),
                type_arrow(vec![type_var("a"), type_var("a")], type_var("a")),
            ),
            (
                "/".into(),
                type_arrow(vec![type_var("a"), type_var("a")], type_var("a")),
            ),
            (
                "negate".into(),
                type_arrow(vec![type_var("a")], type_var("a")),
            ),
        ],
    }
}

fn eq_class() -> TypeClass {
    TypeClass {
        name: "Eq".into(),
        type_param: "a".into(),
        superclasses: vec![],
        methods: vec![(
            "=".into(),
            type_arrow(vec![type_var("a"), type_var("a")], t_bool()),
        )],
    }
}

fn ord_class() -> TypeClass {
    TypeClass {
        name: "Ord".into(),
        type_param: "a".into(),
        superclasses: vec!["Eq".into()],
        methods: vec![
            (
                "<".into(),
                type_arrow(vec![type_var("a"), type_var("a")], t_bool()),
            ),
            (
                ">".into(),
                type_arrow(vec![type_var("a"), type_var("a")], t_bool()),
            ),
            (
                "<=".into(),
                type_arrow(vec![type_var("a"), type_var("a")], t_bool()),
            ),
            (
                ">=".into(),
                type_arrow(vec![type_var("a"), type_var("a")], t_bool()),
            ),
        ],
    }
}

fn show_class() -> TypeClass {
    TypeClass {
        name: "Show".into(),
        type_param: "a".into(),
        superclasses: vec![],
        methods: vec![("show".into(), type_arrow(vec![type_var("a")], t_string()))],
    }
}

// ===== Helper Type Scheme Constructors =====

fn num_bin_op() -> TypeScheme {
    constrained_type(
        vec!["a"],
        vec![Constraint {
            class: "Num".into(),
            type_arg: type_var("a"),
        }],
        type_arrow(vec![type_var("a"), type_var("a")], type_var("a")),
    )
}

fn num_unary_op() -> TypeScheme {
    constrained_type(
        vec!["a"],
        vec![Constraint {
            class: "Num".into(),
            type_arg: type_var("a"),
        }],
        type_arrow(vec![type_var("a")], type_var("a")),
    )
}

fn eq_compare() -> TypeScheme {
    constrained_type(
        vec!["a"],
        vec![Constraint {
            class: "Eq".into(),
            type_arg: type_var("a"),
        }],
        type_arrow(vec![type_var("a"), type_var("a")], t_bool()),
    )
}

fn ord_compare() -> TypeScheme {
    constrained_type(
        vec!["a"],
        vec![Constraint {
            class: "Ord".into(),
            type_arg: type_var("a"),
        }],
        type_arrow(vec![type_var("a"), type_var("a")], t_bool()),
    )
}

// ===== Create Initial Type Environment =====

pub fn create_builtin_env() -> TypeEnv {
    let mut env = empty_env();

    // Add typeclasses
    env = define_class(&env, num_class());
    env = define_class(&env, eq_class());
    env = define_class(&env, ord_class());
    env = define_class(&env, show_class());

    // Arithmetic operators
    env = extend_env(&env, "+", num_bin_op());
    env = extend_env(&env, "-", num_bin_op());
    env = extend_env(&env, "*", num_bin_op());
    env = extend_env(&env, "/", num_bin_op());
    env = extend_env(&env, "negate", num_unary_op());

    // Comparison operators
    env = extend_env(&env, "=", eq_compare());
    env = extend_env(&env, "<", ord_compare());
    env = extend_env(&env, ">", ord_compare());
    env = extend_env(&env, "<=", ord_compare());
    env = extend_env(&env, ">=", ord_compare());

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

    // show
    env = extend_env(
        &env,
        "show",
        constrained_type(
            vec!["a"],
            vec![Constraint {
                class: "Show".into(),
                type_arg: type_var("a"),
            }],
            type_arrow(vec![type_var("a")], t_string()),
        ),
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
    env = extend_env(&env, "abs", num_unary_op());
    env = extend_env(&env, "rem", num_bin_op());
    env = extend_env(&env, "mod", num_bin_op());
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
                constraints: vec![],
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
                constraints: vec![],
                params: vec![],
                result: t_unit(),
            },
            EffectOp {
                name: "fiber-self".into(),
                type_params: vec![],
                constraints: vec![],
                params: vec![],
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
                constraints: vec![],
                params: vec![t_int()],
                result: t_channel(),
            },
            EffectOp {
                name: "chan-send".into(),
                type_params: vec!["a".into()],
                constraints: vec![],
                params: vec![t_channel(), type_var("a")],
                result: type_app(type_const("Result"), vec![t_string(), t_unit()]),
            },
            EffectOp {
                name: "chan-recv".into(),
                type_params: vec!["a".into()],
                constraints: vec![],
                params: vec![t_channel()],
                result: t_option(type_var("a")),
            },
            EffectOp {
                name: "chan-close".into(),
                type_params: vec![],
                constraints: vec![],
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
                constraints: vec![],
                params: vec![t_pid(), type_var("a")],
                result: t_unit(),
            },
            EffectOp {
                name: "actor-receive".into(),
                type_params: vec!["a".into()],
                constraints: vec![],
                params: vec![],
                result: type_var("a"),
            },
            EffectOp {
                name: "actor-receive-match".into(),
                type_params: vec!["a".into()],
                constraints: vec![],
                params: vec![type_arrow(vec![type_var("a")], t_bool())],
                result: type_var("a"),
            },
            EffectOp {
                name: "actor-monitor".into(),
                type_params: vec![],
                constraints: vec![],
                params: vec![t_pid()],
                result: t_unit(),
            },
            EffectOp {
                name: "actor-link".into(),
                type_params: vec![],
                constraints: vec![],
                params: vec![t_pid()],
                result: t_unit(),
            },
            EffectOp {
                name: "actor-unlink".into(),
                type_params: vec![],
                constraints: vec![],
                params: vec![t_pid()],
                result: t_unit(),
            },
            EffectOp {
                name: "actor-trap-exit".into(),
                type_params: vec![],
                constraints: vec![],
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
                constraints: vec![Constraint {
                    class: "Show".into(),
                    type_arg: type_var("a"),
                }],
                params: vec![type_var("a")],
                result: t_unit(),
            },
            EffectOp {
                name: "println".into(),
                type_params: vec!["a".into()],
                constraints: vec![Constraint {
                    class: "Show".into(),
                    type_arg: type_var("a"),
                }],
                params: vec![type_var("a")],
                result: t_unit(),
            },
            EffectOp {
                name: "read-line".into(),
                type_params: vec![],
                constraints: vec![],
                params: vec![],
                result: t_string(),
            },
            EffectOp {
                name: "read-file".into(),
                type_params: vec![],
                constraints: vec![],
                params: vec![t_string()],
                result: type_app(type_const("Result"), vec![t_string(), t_string()]),
            },
            EffectOp {
                name: "write-file".into(),
                type_params: vec![],
                constraints: vec![],
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
    env = extend_env(&env, "gensym", monotype(type_arrow(vec![], t_symbol())));
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

// ===== Instance Checking =====

pub fn has_instance(class_name: &str, ty: &Type) -> (bool, Vec<Constraint>) {
    match class_name {
        "Num" => match ty {
            Type::Const(n) if n == "Int" || n == "Float" => (true, vec![]),
            Type::Var(_) => (
                true,
                vec![Constraint {
                    class: "Num".into(),
                    type_arg: ty.clone(),
                }],
            ),
            _ => (false, vec![]),
        },
        "Eq" => match ty {
            Type::Const(n)
                if matches!(n.as_str(), "Int" | "Float" | "Bool" | "String" | "Unit") =>
            {
                (true, vec![])
            }
            Type::Var(_) => (
                true,
                vec![Constraint {
                    class: "Eq".into(),
                    type_arg: ty.clone(),
                }],
            ),
            Type::App { constructor, args } => {
                if let Type::Const(n) = constructor.as_ref() {
                    if n == "List" {
                        if let Some(elem) = args.first() {
                            let (has, needed) = has_instance("Eq", elem);
                            return (has, needed);
                        }
                    }
                }
                (false, vec![])
            }
            _ => (false, vec![]),
        },
        "Ord" => match ty {
            Type::Const(n) if matches!(n.as_str(), "Int" | "Float" | "String") => (true, vec![]),
            Type::Var(_) => (
                true,
                vec![Constraint {
                    class: "Ord".into(),
                    type_arg: ty.clone(),
                }],
            ),
            _ => (false, vec![]),
        },
        "Show" => match ty {
            Type::Const(n)
                if matches!(n.as_str(), "Int" | "Float" | "Bool" | "String" | "Unit") =>
            {
                (true, vec![])
            }
            Type::Const(_) => (true, vec![]), // User-defined ADTs automatically have Show
            Type::Var(_) => (
                true,
                vec![Constraint {
                    class: "Show".into(),
                    type_arg: ty.clone(),
                }],
            ),
            Type::App { args, .. } => {
                let mut needed = vec![];
                for arg in args {
                    let (has, n) = has_instance("Show", arg);
                    if !has {
                        return (false, vec![]);
                    }
                    needed.extend(n);
                }
                (true, needed)
            }
            _ => (false, vec![]),
        },
        _ => (false, vec![]),
    }
}

pub fn resolve_constraints(constraints: &[Constraint]) -> Vec<Constraint> {
    let mut residual = Vec::new();

    for c in constraints {
        let (has, needed) = has_instance(&c.class, &c.type_arg);
        if !has {
            residual.push(c.clone());
        } else {
            for nc in needed {
                let already = residual.iter().any(|r| {
                    r.class == nc.class
                        && matches!((&r.type_arg, &nc.type_arg), (Type::Var(a), Type::Var(b)) if a == b)
                });
                if !already {
                    residual.push(nc);
                }
            }
        }
    }

    // Filter out constraints on concrete types that have instances
    residual
        .into_iter()
        .filter(|c| {
            let (has, _) = has_instance(&c.class, &c.type_arg);
            matches!(c.type_arg, Type::Var(_)) || !has
        })
        .collect()
}
