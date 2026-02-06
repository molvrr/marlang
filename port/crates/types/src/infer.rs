use std::collections::{HashMap, HashSet};

use marlang_core::ast::{self, Expr, LispValue};
use marlang_core::intern::Interner;

use crate::ast::*;
use crate::builtins::resolve_constraints;
use crate::env::*;
use crate::errors::TypeError;
use crate::unify::*;

// ===== Inference Result =====

pub struct InferResult {
    pub ty: Type,
    pub subst: Substitution,
    pub constraints: Vec<Constraint>,
    pub effects: EffectRow,
}

fn pure_result(ty: Type) -> InferResult {
    InferResult {
        ty,
        subst: empty_subst(),
        constraints: vec![],
        effects: effect_row_empty(),
    }
}

fn pure_result_with(ty: Type, subst: Substitution, constraints: Vec<Constraint>) -> InferResult {
    InferResult {
        ty,
        subst,
        constraints,
        effects: effect_row_empty(),
    }
}

// ===== AST Helpers =====

fn list_to_array<'a>(expr: &'a Expr) -> Vec<&'a Expr> {
    ast::list_to_array(expr)
}

fn get_symbol<'a>(expr: &'a Expr, interner: &'a Interner) -> Option<&'a str> {
    ast::get_symbol_name(expr, interner)
}

fn is_symbol(expr: &Expr, interner: &Interner, name: &str) -> bool {
    get_symbol(expr, interner) == Some(name)
}

// ===== Variadic Arithmetic =====

struct VariadicArithInfo {
    min_args: usize,
}

fn variadic_arith_ops() -> HashMap<&'static str, VariadicArithInfo> {
    let mut m = HashMap::new();
    m.insert("+", VariadicArithInfo { min_args: 0 });
    m.insert("*", VariadicArithInfo { min_args: 0 });
    m.insert("-", VariadicArithInfo { min_args: 1 });
    m.insert("/", VariadicArithInfo { min_args: 2 });
    m
}

const FFI_VARIADIC_FUNCTIONS: &[&str] = &["ffi-call", "struct-new", "struct-get", "struct-set", "list"];

// ===== Type Expression Parsing =====

pub fn parse_type_expr(expr: &Expr, env: &TypeEnv, interner: &Interner) -> Result<Type, TypeError> {
    match expr {
        Expr::Atom(LispValue::Symbol(sym), _) => {
            let name = interner.resolve(*sym);
            match name {
                "Int" => Ok(t_int()),
                "Float" => Ok(t_float()),
                "Bool" => Ok(t_bool()),
                "String" => Ok(t_string()),
                "Unit" => Ok(t_unit()),
                _ => {
                    // Check for user-defined data type
                    if lookup_data_type(env, name).is_some() {
                        return Ok(type_const(name));
                    }
                    // Lowercase = type variable, uppercase = type constant
                    if name.starts_with(|c: char| c.is_lowercase()) {
                        Ok(type_var(name))
                    } else {
                        Ok(type_const(name))
                    }
                }
            }
        }
        Expr::Atom(_, _) => Err(TypeError::InvalidTypeAnnotation {
            message: "Invalid type expression".into(),
            span: None,
        }),
        Expr::List { .. } => {
            let elements = list_to_array(expr);
            if elements.is_empty() {
                return Ok(t_unit());
            }

            let first_sym = get_symbol(elements[0], interner);

            // Arrow type: (-> a b c)
            if first_sym == Some("->") {
                if elements.len() < 2 {
                    return Err(TypeError::InvalidTypeAnnotation {
                        message: "Arrow type needs at least a result type".into(),
                        span: None,
                    });
                }
                let mut types = Vec::new();
                for e in &elements[1..] {
                    types.push(parse_type_expr(e, env, interner)?);
                }
                if types.len() == 1 {
                    return Ok(type_arrow(vec![], types.pop().unwrap()));
                }
                let result = types.pop().unwrap();
                return Ok(type_arrow(types, result));
            }

            // Constrained type: (=> constraints type)
            if first_sym == Some("=>") {
                if elements.len() < 2 {
                    return Err(TypeError::InvalidTypeAnnotation {
                        message: "Constrained type needs a body type".into(),
                        span: None,
                    });
                }
                return parse_type_expr(elements[elements.len() - 1], env, interner);
            }

            // Recursive type: (mu a T)
            if first_sym == Some("mu") {
                if elements.len() != 3 {
                    return Err(TypeError::InvalidTypeAnnotation {
                        message: "mu type needs: (mu var body)".into(),
                        span: None,
                    });
                }
                let var_name = get_symbol(elements[1], interner).ok_or(TypeError::InvalidTypeAnnotation {
                    message: "mu variable must be a symbol".into(),
                    span: None,
                })?;
                let body = parse_type_expr(elements[2], env, interner)?;
                return Ok(type_rec(var_name, body));
            }

            // Type application: (List a), (Option Int)
            let constructor = parse_type_expr(elements[0], env, interner)?;
            let mut args = Vec::new();
            for e in &elements[1..] {
                args.push(parse_type_expr(e, env, interner)?);
            }
            if args.is_empty() {
                Ok(constructor)
            } else {
                Ok(type_app(constructor, args))
            }
        }
        Expr::Nil(_) => Ok(t_unit()),
    }
}

fn parse_constraint(expr: &Expr, env: &TypeEnv, interner: &Interner) -> Result<Constraint, TypeError> {
    let elements = list_to_array(expr);
    if elements.len() != 2 {
        return Err(TypeError::InvalidTypeAnnotation {
            message: "Constraint must be (ClassName typevar)".into(),
            span: None,
        });
    }
    let class_name = get_symbol(elements[0], interner).ok_or(TypeError::InvalidTypeAnnotation {
        message: "Class name must be a symbol".into(),
        span: None,
    })?;
    let type_arg = parse_type_expr(elements[1], env, interner)?;
    Ok(Constraint {
        class: class_name.to_string(),
        type_arg,
    })
}

fn parse_type_annotation(
    expr: &Expr,
    env: &TypeEnv,
    interner: &Interner,
) -> Result<(Type, Vec<Constraint>), TypeError> {
    if let Expr::List { .. } = expr {
        let elements = list_to_array(expr);
        if !elements.is_empty() {
            if let Some("=>") = get_symbol(elements[0], interner) {
                if elements.len() < 3 {
                    return Err(TypeError::InvalidTypeAnnotation {
                        message: "Constrained type needs constraints and a body".into(),
                        span: None,
                    });
                }
                let mut constraints = Vec::new();
                for e in &elements[1..elements.len() - 1] {
                    constraints.push(parse_constraint(e, env, interner)?);
                }
                let ty = parse_type_expr(elements[elements.len() - 1], env, interner)?;
                return Ok((ty, constraints));
            }
        }
    }
    let ty = parse_type_expr(expr, env, interner)?;
    Ok((ty, vec![]))
}

// ===== Pattern Parsing =====

fn parse_pattern(expr: &Expr, env: &TypeEnv, interner: &Interner) -> Result<Pattern, TypeError> {
    match expr {
        Expr::Atom(LispValue::Symbol(sym), _) => {
            let name = interner.resolve(*sym);
            if name == "_" {
                return Ok(Pattern::Wildcard);
            }
            // Check if it's a constructor
            if let Some((_, ctor)) = lookup_constructor(env, name) {
                if ctor.params.is_empty() {
                    return Ok(Pattern::Constructor {
                        name: name.to_string(),
                        args: vec![],
                    });
                }
            }
            Ok(Pattern::Var(name.to_string()))
        }
        Expr::Atom(LispValue::Number(v, _), _) => {
            Ok(Pattern::Literal(LiteralPattern::Number(*v)))
        }
        Expr::Atom(LispValue::String(s), _) => {
            Ok(Pattern::Literal(LiteralPattern::String(s.to_string())))
        }
        Expr::List { .. } => {
            let elements = list_to_array(expr);
            if elements.is_empty() {
                return Err(TypeError::InvalidTypeAnnotation {
                    message: "Empty pattern".into(),
                    span: None,
                });
            }
            let ctor_name = get_symbol(elements[0], interner).ok_or(TypeError::InvalidTypeAnnotation {
                message: "Pattern constructor must be a symbol".into(),
                span: None,
            })?;
            let mut args = Vec::new();
            for e in &elements[1..] {
                args.push(parse_pattern(e, env, interner)?);
            }
            Ok(Pattern::Constructor {
                name: ctor_name.to_string(),
                args,
            })
        }
        Expr::Nil(_) => Ok(Pattern::Constructor {
            name: "Nil".to_string(),
            args: vec![],
        }),
    }
}

// ===== Main Inference =====

pub fn infer_expr(
    env: &TypeEnv,
    expr: &Expr,
    interner: &Interner,
) -> Result<InferResult, TypeError> {
    match expr {
        Expr::Nil(_) => Ok(pure_result(t_unit())),
        Expr::Atom(value, _) => infer_atom(env, value, interner),
        Expr::List { .. } => {
            let elements = list_to_array(expr);
            if elements.is_empty() {
                return Ok(pure_result(t_unit()));
            }

            let first_sym = get_symbol(elements[0], interner);

            if let Some(sym) = first_sym {
                match sym {
                    "define" => return infer_define(env, &elements, interner),
                    "lambda" => return infer_lambda(env, &elements, interner),
                    "if" => return infer_if(env, &elements, interner),
                    "let" => return infer_let(env, &elements, interner),
                    "begin" => return infer_begin(env, &elements, interner),
                    "cond" => return infer_cond(env, &elements, interner),
                    "match" => return infer_match(env, &elements, interner),
                    "data" => return infer_data_def(env, &elements, interner),
                    "class" | "instance" | "type" => return Ok(pure_result(t_unit())),
                    "effect" => return infer_effect_def(env, &elements, interner),
                    "perform" => return infer_perform(env, &elements, interner),
                    "handle" => return infer_handle(env, &elements, interner),
                    "resume" => return infer_resume(env, &elements, interner),
                    "import" => return Ok(pure_result(t_unit())),
                    _ => {}
                }
            }

            infer_application(env, &elements, interner)
        }
    }
}

fn infer_atom(
    env: &TypeEnv,
    value: &LispValue,
    interner: &Interner,
) -> Result<InferResult, TypeError> {
    match value {
        LispValue::Number(_, is_float) => {
            let ty = if *is_float { t_float() } else { t_int() };
            Ok(pure_result(ty))
        }
        LispValue::String(_) => Ok(pure_result(t_string())),
        LispValue::Symbol(sym) => {
            let name = interner.resolve(*sym);
            // Boolean literals
            if name == "#t" || name == "#f" {
                return Ok(pure_result(t_bool()));
            }
            let scheme = lookup_var(env, name).ok_or(TypeError::UndefinedVariable {
                name: name.to_string(),
                span: None,
            })?;
            let (ty, constraints) = instantiate(&scheme);
            Ok(pure_result_with(ty, empty_subst(), constraints))
        }
    }
}

// ===== Define =====

fn infer_define(
    env: &TypeEnv,
    elements: &[&Expr],
    interner: &Interner,
) -> Result<InferResult, TypeError> {
    if elements.len() < 3 {
        return Err(TypeError::ArityMismatch {
            expected: 2,
            got: elements.len() - 1,
            context: "define".into(),
            span: None,
        });
    }

    let name_expr = elements[1];
    let value_expr = elements[2];

    // Check for type annotation: (define (name : Type) value)
    if let Expr::List { .. } = name_expr {
        let name_elements = list_to_array(name_expr);
        if name_elements.len() == 3 && is_symbol(name_elements[1], interner, ":") {
            let name = get_symbol(name_elements[0], interner).ok_or(TypeError::InvalidTypeAnnotation {
                message: "Expected symbol before :".into(),
                span: None,
            })?;

            let (annotated_type, annotated_constraints) = parse_type_annotation(name_elements[2], env, interner)?;
            let value_result = infer_expr(env, value_expr, interner)?;
            let unify_subst = unify_with_context(&annotated_type, &value_result.ty, &format!("definition of {name}"))?;
            let final_subst = compose_subst(&unify_subst, &value_result.subst);
            let mut all_constraints = annotated_constraints;
            all_constraints.extend(value_result.constraints);

            return Ok(InferResult {
                ty: t_unit(),
                subst: final_subst,
                constraints: resolve_constraints(&all_constraints),
                effects: value_result.effects,
            });
        }
    }

    // Regular define
    let name = get_symbol(name_expr, interner).ok_or(TypeError::InvalidTypeAnnotation {
        message: "define expects a symbol name".into(),
        span: None,
    })?;
    let _ = name; // suppress unused warning

    let result = infer_expr(env, value_expr, interner)?;
    Ok(InferResult {
        ty: t_unit(),
        subst: result.subst,
        constraints: result.constraints,
        effects: result.effects,
    })
}

// ===== Lambda =====

fn infer_lambda(
    env: &TypeEnv,
    elements: &[&Expr],
    interner: &Interner,
) -> Result<InferResult, TypeError> {
    if elements.len() < 3 {
        return Err(TypeError::ArityMismatch {
            expected: 2,
            got: elements.len() - 1,
            context: "lambda".into(),
            span: None,
        });
    }

    let params_expr = elements[1];
    let body_exprs = &elements[2..];

    let param_list = list_to_array(params_expr);
    let mut params: Vec<(String, Type)> = Vec::new();
    let mut param_env = child_env(env);

    for param in &param_list {
        // Check for annotated param: (x : Int)
        if let Expr::List { .. } = param {
            let param_elements = list_to_array(param);
            if param_elements.len() == 3 && is_symbol(param_elements[1], interner, ":") {
                let name = get_symbol(param_elements[0], interner).ok_or(TypeError::InvalidTypeAnnotation {
                    message: "Parameter name must be a symbol".into(),
                    span: None,
                })?;
                let ty = parse_type_expr(param_elements[2], env, interner)?;
                params.push((name.to_string(), ty.clone()));
                param_env = extend_env(&param_env, name, TypeScheme {
                    quantified_vars: vec![],
                    qual_type: QualType { constraints: vec![], ty },
                });
                continue;
            }
        }

        let name = get_symbol(param, interner).ok_or(TypeError::InvalidTypeAnnotation {
            message: "Parameter must be a symbol".into(),
            span: None,
        })?;
        let param_type = fresh_type_var();
        params.push((name.to_string(), param_type.clone()));
        param_env = extend_env(&param_env, name, TypeScheme {
            quantified_vars: vec![],
            qual_type: QualType { constraints: vec![], ty: param_type },
        });
    }

    // Infer body
    let mut body_subst = empty_subst();
    let mut body_type = t_unit();
    let mut all_constraints = Vec::new();
    let mut body_effects = effect_row_empty();

    for (i, body_expr) in body_exprs.iter().enumerate() {
        let env_for_body = apply_subst_env(&body_subst, &param_env);
        let result = infer_expr(&env_for_body, body_expr, interner)?;

        body_subst = compose_subst(&result.subst, &body_subst);
        all_constraints.extend(result.constraints);
        body_effects = combine_effect_rows(&body_effects, &result.effects);

        if i == body_exprs.len() - 1 {
            body_type = result.ty;
        }
    }

    let param_types: Vec<Type> = params.iter().map(|(_, t)| apply_subst(&body_subst, t)).collect();
    let result_type = apply_subst(&body_subst, &body_type);
    let fn_effects = if !body_effects.effects().is_empty() || matches!(body_effects, EffectRow::Open { .. }) {
        Some(body_effects)
    } else {
        None
    };
    let fn_type = type_arrow_with_effects(param_types, fn_effects, result_type);

    Ok(InferResult {
        ty: fn_type,
        subst: body_subst,
        constraints: all_constraints,
        effects: effect_row_empty(),
    })
}

// ===== If =====

fn infer_if(
    env: &TypeEnv,
    elements: &[&Expr],
    interner: &Interner,
) -> Result<InferResult, TypeError> {
    if elements.len() < 3 || elements.len() > 4 {
        return Err(TypeError::ArityMismatch {
            expected: 3,
            got: elements.len() - 1,
            context: "if".into(),
            span: None,
        });
    }

    let cond_result = infer_expr(env, elements[1], interner)?;
    let cond_unify = unify_with_context(&cond_result.ty, &t_bool(), "if condition")?;
    let s1 = compose_subst(&cond_unify, &cond_result.subst);
    let env1 = apply_subst_env(&s1, env);

    let then_result = infer_expr(&env1, elements[2], interner)?;
    let s2 = compose_subst(&then_result.subst, &s1);

    if elements.len() == 4 {
        let env2 = apply_subst_env(&s2, env);
        let else_result = infer_expr(&env2, elements[3], interner)?;
        let s3 = compose_subst(&else_result.subst, &s2);

        let then_type = apply_subst(&s3, &then_result.ty);
        let else_type = apply_subst(&s3, &else_result.ty);

        let branch_unify = unify_with_context(&then_type, &else_type, "if branches")?;
        let final_subst = compose_subst(&branch_unify, &s3);
        let all_effects = combine_effect_rows(
            &combine_effect_rows(&cond_result.effects, &then_result.effects),
            &else_result.effects,
        );
        let mut constraints = cond_result.constraints;
        constraints.extend(then_result.constraints);
        constraints.extend(else_result.constraints);

        Ok(InferResult {
            ty: apply_subst(&final_subst, &then_type),
            subst: final_subst,
            constraints,
            effects: all_effects,
        })
    } else {
        let all_effects = combine_effect_rows(&cond_result.effects, &then_result.effects);
        let mut constraints = cond_result.constraints;
        constraints.extend(then_result.constraints);
        Ok(InferResult {
            ty: t_unit(),
            subst: s2,
            constraints,
            effects: all_effects,
        })
    }
}

// ===== Let =====

fn infer_let(
    env: &TypeEnv,
    elements: &[&Expr],
    interner: &Interner,
) -> Result<InferResult, TypeError> {
    if elements.len() < 3 {
        return Err(TypeError::ArityMismatch {
            expected: 2,
            got: elements.len() - 1,
            context: "let".into(),
            span: None,
        });
    }

    let bindings_expr = elements[1];
    let body_exprs = &elements[2..];

    let bindings = list_to_array(bindings_expr);
    let mut let_env = child_env(env);
    let mut subst = empty_subst();
    let mut all_constraints: Vec<Constraint> = Vec::new();
    let mut all_effects = effect_row_empty();

    for binding in &bindings {
        let pair = list_to_array(binding);
        if pair.len() != 2 {
            return Err(TypeError::InvalidTypeAnnotation {
                message: "let binding must be (name value)".into(),
                span: None,
            });
        }

        let name_expr = pair[0];
        let value_expr = pair[1];

        let name: String;
        let mut annotated_type: Option<Type> = None;

        if let Expr::List { .. } = name_expr {
            let name_elements = list_to_array(name_expr);
            if name_elements.len() == 3 && is_symbol(name_elements[1], interner, ":") {
                let n = get_symbol(name_elements[0], interner).ok_or(TypeError::InvalidTypeAnnotation {
                    message: "Expected symbol before :".into(),
                    span: None,
                })?;
                name = n.to_string();
                annotated_type = Some(parse_type_expr(name_elements[2], env, interner)?);
            } else {
                return Err(TypeError::InvalidTypeAnnotation {
                    message: "Invalid let binding name".into(),
                    span: None,
                });
            }
        } else {
            let n = get_symbol(name_expr, interner).ok_or(TypeError::InvalidTypeAnnotation {
                message: "let binding name must be a symbol".into(),
                span: None,
            })?;
            name = n.to_string();
        }

        let env_for_value = apply_subst_env(&subst, &let_env);
        let value_result = infer_expr(&env_for_value, value_expr, interner)?;

        subst = compose_subst(&value_result.subst, &subst);
        all_constraints.extend(value_result.constraints);
        all_effects = combine_effect_rows(&all_effects, &value_result.effects);

        let mut binding_type = value_result.ty;

        if let Some(ann_type) = annotated_type {
            let unify_result = unify_with_context(&ann_type, &binding_type, &format!("let binding {name}"))?;
            subst = compose_subst(&unify_result, &subst);
            binding_type = apply_subst(&subst, &ann_type);
        }

        let env_for_gen = apply_subst_env(&subst, &let_env);
        let scheme = generalize(&env_for_gen, &all_constraints, &apply_subst(&subst, &binding_type));
        let_env = extend_env(&let_env, &name, scheme);
    }

    // Infer body
    let mut body_type = t_unit();
    for (i, body_expr) in body_exprs.iter().enumerate() {
        let env_for_body = apply_subst_env(&subst, &let_env);
        let result = infer_expr(&env_for_body, body_expr, interner)?;

        subst = compose_subst(&result.subst, &subst);
        all_constraints.extend(result.constraints);
        all_effects = combine_effect_rows(&all_effects, &result.effects);

        if i == body_exprs.len() - 1 {
            body_type = result.ty;
        }
    }

    Ok(InferResult {
        ty: apply_subst(&subst, &body_type),
        subst,
        constraints: all_constraints,
        effects: all_effects,
    })
}

// ===== Begin =====

fn infer_begin(
    env: &TypeEnv,
    elements: &[&Expr],
    interner: &Interner,
) -> Result<InferResult, TypeError> {
    let body_exprs = &elements[1..];

    if body_exprs.is_empty() {
        return Ok(pure_result(t_unit()));
    }

    let mut subst = empty_subst();
    let mut last_type = t_unit();
    let mut all_constraints = Vec::new();
    let mut all_effects = effect_row_empty();

    for body_expr in body_exprs {
        let env_for_body = apply_subst_env(&subst, env);
        let result = infer_expr(&env_for_body, body_expr, interner)?;
        subst = compose_subst(&result.subst, &subst);
        all_constraints.extend(result.constraints);
        all_effects = combine_effect_rows(&all_effects, &result.effects);
        last_type = result.ty;
    }

    Ok(InferResult {
        ty: apply_subst(&subst, &last_type),
        subst,
        constraints: all_constraints,
        effects: all_effects,
    })
}

// ===== Cond =====

fn infer_cond(
    env: &TypeEnv,
    elements: &[&Expr],
    interner: &Interner,
) -> Result<InferResult, TypeError> {
    let clauses = &elements[1..];
    if clauses.is_empty() {
        return Ok(pure_result(t_unit()));
    }

    let mut subst = empty_subst();
    let mut result_type: Option<Type> = None;
    let mut all_constraints = Vec::new();
    let mut all_effects = effect_row_empty();

    for clause_expr in clauses {
        let clause = list_to_array(clause_expr);
        if clause.len() != 2 {
            return Err(TypeError::InvalidTypeAnnotation {
                message: "cond clause must be (test result)".into(),
                span: None,
            });
        }

        let test_expr = clause[0];
        let result_expr = clause[1];

        if !is_symbol(test_expr, interner, "else") {
            let test_env = apply_subst_env(&subst, env);
            let test_result = infer_expr(&test_env, test_expr, interner)?;
            let test_unify = unify_with_context(&test_result.ty, &t_bool(), "cond test")?;
            subst = compose_subst(&test_unify, &compose_subst(&test_result.subst, &subst));
            all_constraints.extend(test_result.constraints);
            all_effects = combine_effect_rows(&all_effects, &test_result.effects);
        }

        let result_env = apply_subst_env(&subst, env);
        let expr_result = infer_expr(&result_env, result_expr, interner)?;
        subst = compose_subst(&expr_result.subst, &subst);
        all_constraints.extend(expr_result.constraints);
        all_effects = combine_effect_rows(&all_effects, &expr_result.effects);

        let clause_type = apply_subst(&subst, &expr_result.ty);

        match &result_type {
            None => result_type = Some(clause_type),
            Some(rt) => {
                let branch_unify = unify_with_context(rt, &clause_type, "cond branches")?;
                subst = compose_subst(&branch_unify, &subst);
                result_type = Some(apply_subst(&subst, rt));
            }
        }
    }

    Ok(InferResult {
        ty: result_type.unwrap_or_else(t_unit),
        subst,
        constraints: all_constraints,
        effects: all_effects,
    })
}

// ===== Match =====

fn get_data_type_from_type(env: &TypeEnv, t: &Type) -> Option<DataType> {
    match t {
        Type::Const(name) => lookup_data_type(env, name),
        Type::App { constructor, .. } => {
            if let Type::Const(name) = constructor.as_ref() {
                lookup_data_type(env, name)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn check_exhaustiveness(
    env: &TypeEnv,
    scrutinee_type: &Type,
    patterns: &[Pattern],
) -> Result<(), TypeError> {
    let data_type = match get_data_type_from_type(env, scrutinee_type) {
        Some(dt) => dt,
        None => return Ok(()),
    };

    for p in patterns {
        if matches!(p, Pattern::Wildcard | Pattern::Var(_)) {
            return Ok(());
        }
    }

    let covered: HashSet<String> = patterns
        .iter()
        .filter_map(|p| {
            if let Pattern::Constructor { name, .. } = p {
                Some(name.clone())
            } else {
                None
            }
        })
        .collect();

    let missing: Vec<String> = data_type
        .constructors
        .iter()
        .filter(|c| !covered.contains(&c.name))
        .map(|c| c.name.clone())
        .collect();

    if !missing.is_empty() {
        return Err(TypeError::NonExhaustivePatterns {
            missing,
            span: None,
        });
    }

    Ok(())
}

fn infer_match(
    env: &TypeEnv,
    elements: &[&Expr],
    interner: &Interner,
) -> Result<InferResult, TypeError> {
    if elements.len() < 3 {
        return Err(TypeError::ArityMismatch {
            expected: 2,
            got: elements.len() - 1,
            context: "match".into(),
            span: None,
        });
    }

    let scrutinee_expr = elements[1];
    let branches = &elements[2..];

    let scrutinee_result = infer_expr(env, scrutinee_expr, interner)?;
    let mut subst = scrutinee_result.subst;
    let mut scrutinee_type = scrutinee_result.ty;
    let mut result_type: Option<Type> = None;
    let mut all_constraints = scrutinee_result.constraints;
    let mut all_effects = scrutinee_result.effects;
    let mut parsed_patterns: Vec<Pattern> = Vec::new();

    for branch_expr in branches {
        let branch_elements = list_to_array(branch_expr);
        if branch_elements.len() != 2 {
            return Err(TypeError::InvalidTypeAnnotation {
                message: "match branch must be (pattern body)".into(),
                span: None,
            });
        }

        let pattern_expr = branch_elements[0];
        let body_expr = branch_elements[1];

        let pattern = parse_pattern(pattern_expr, env, interner)?;
        parsed_patterns.push(pattern.clone());

        let pattern_check = infer_pattern(env, &pattern, &apply_subst(&subst, &scrutinee_type))?;

        let pattern_unify = unify_with_context(
            &apply_subst(&subst, &scrutinee_type),
            &pattern_check.ty,
            "match pattern",
        )?;
        subst = compose_subst(&pattern_unify, &compose_subst(&pattern_check.subst, &subst));
        scrutinee_type = apply_subst(&subst, &scrutinee_type);

        // Extend env with pattern bindings
        let mut branch_env = apply_subst_env(&subst, env);
        for (name, ty) in &pattern_check.bindings {
            branch_env = extend_env(&branch_env, name, TypeScheme {
                quantified_vars: vec![],
                qual_type: QualType {
                    constraints: vec![],
                    ty: apply_subst(&subst, ty),
                },
            });
        }

        let body_result = infer_expr(&branch_env, body_expr, interner)?;
        subst = compose_subst(&body_result.subst, &subst);
        all_constraints.extend(body_result.constraints);
        all_effects = combine_effect_rows(&all_effects, &body_result.effects);

        let branch_type = apply_subst(&subst, &body_result.ty);

        match &result_type {
            None => result_type = Some(branch_type),
            Some(rt) => {
                let branch_unify = unify_with_context(rt, &branch_type, "match branches")?;
                subst = compose_subst(&branch_unify, &subst);
                result_type = Some(apply_subst(&subst, rt));
            }
        }
    }

    // Check exhaustiveness
    check_exhaustiveness(env, &apply_subst(&subst, &scrutinee_type), &parsed_patterns)?;

    Ok(InferResult {
        ty: result_type.unwrap_or_else(t_unit),
        subst,
        constraints: all_constraints,
        effects: all_effects,
    })
}

struct PatternResult {
    ty: Type,
    subst: Substitution,
    bindings: Vec<(String, Type)>,
}

fn infer_pattern(
    env: &TypeEnv,
    pattern: &Pattern,
    expected_type: &Type,
) -> Result<PatternResult, TypeError> {
    match pattern {
        Pattern::Wildcard => Ok(PatternResult {
            ty: expected_type.clone(),
            subst: empty_subst(),
            bindings: vec![],
        }),
        Pattern::Var(name) => Ok(PatternResult {
            ty: expected_type.clone(),
            subst: empty_subst(),
            bindings: vec![(name.clone(), expected_type.clone())],
        }),
        Pattern::Literal(lit) => {
            let literal_type = match lit {
                LiteralPattern::Number(n) => {
                    if n.fract() == 0.0 && n.abs() < i64::MAX as f64 {
                        t_int()
                    } else {
                        t_float()
                    }
                }
                LiteralPattern::String(_) => t_string(),
                LiteralPattern::Bool(_) => t_bool(),
            };
            let s = unify(expected_type, &literal_type)?;
            Ok(PatternResult {
                ty: literal_type,
                subst: s,
                bindings: vec![],
            })
        }
        Pattern::Constructor { name, args } => {
            let (data_type, constructor) = lookup_constructor(env, name).ok_or(TypeError::UndefinedConstructor {
                name: name.clone(),
                span: None,
            })?;

            if args.len() != constructor.params.len() {
                return Err(TypeError::ArityMismatch {
                    expected: constructor.params.len(),
                    got: args.len(),
                    context: format!("constructor {name}"),
                    span: None,
                });
            }

            // Instantiate with fresh type vars
            let mut type_var_map: HashMap<String, Type> = HashMap::new();
            for tv in &data_type.type_params {
                type_var_map.insert(tv.clone(), fresh_type_var());
            }

            let instantiated_result = if data_type.type_params.is_empty() {
                type_const(&data_type.name)
            } else {
                type_app(
                    type_const(&data_type.name),
                    data_type.type_params.iter().map(|tv| type_var_map[tv].clone()).collect(),
                )
            };

            let s1 = unify(expected_type, &instantiated_result)?;
            let mut subst = s1;
            let mut all_bindings = Vec::new();

            for (i, arg_pattern) in args.iter().enumerate() {
                let mut param_type = constructor.params[i].clone();
                // Apply type variable substitution
                let tv_subst: Substitution = type_var_map.clone();
                param_type = apply_subst(&tv_subst, &param_type);
                param_type = apply_subst(&subst, &param_type);

                let arg_result = infer_pattern(env, arg_pattern, &param_type)?;
                subst = compose_subst(&arg_result.subst, &subst);
                all_bindings.extend(arg_result.bindings);
            }

            Ok(PatternResult {
                ty: apply_subst(&subst, &instantiated_result),
                subst,
                bindings: all_bindings,
            })
        }
    }
}

// ===== Application =====

fn infer_variadic_arith(
    env: &TypeEnv,
    op_name: &str,
    arg_exprs: &[&Expr],
    interner: &Interner,
) -> Result<InferResult, TypeError> {
    let ops = variadic_arith_ops();
    let info = &ops[op_name];

    if arg_exprs.len() < info.min_args {
        return Err(TypeError::ArityMismatch {
            expected: info.min_args,
            got: arg_exprs.len(),
            context: op_name.to_string(),
            span: None,
        });
    }

    if arg_exprs.is_empty() {
        return Ok(pure_result(t_int()));
    }

    let mut subst = empty_subst();
    let mut all_constraints = Vec::new();
    let mut all_effects = effect_row_empty();
    let mut arg_types = Vec::new();

    for arg_expr in arg_exprs {
        let env_for_arg = apply_subst_env(&subst, env);
        let arg_result = infer_expr(&env_for_arg, arg_expr, interner)?;
        subst = compose_subst(&arg_result.subst, &subst);
        all_constraints.extend(arg_result.constraints);
        all_effects = combine_effect_rows(&all_effects, &arg_result.effects);
        arg_types.push(arg_result.ty);
    }

    let mut unified_type = apply_subst(&subst, &arg_types[0]);
    for (i, at) in arg_types.iter().enumerate().skip(1) {
        let arg_type = apply_subst(&subst, at);
        let unify_result = unify_with_context(&unified_type, &arg_type, &format!("{op_name} argument {}", i + 1))?;
        subst = compose_subst(&unify_result, &subst);
        unified_type = apply_subst(&subst, &unified_type);
    }

    all_constraints.push(Constraint {
        class: "Num".into(),
        type_arg: unified_type.clone(),
    });

    Ok(InferResult {
        ty: unified_type,
        subst,
        constraints: all_constraints,
        effects: all_effects,
    })
}

fn infer_ffi_application(
    env: &TypeEnv,
    fn_name: &str,
    arg_exprs: &[&Expr],
    interner: &Interner,
) -> Result<InferResult, TypeError> {
    let mut subst = empty_subst();
    let mut all_constraints = Vec::new();
    let mut all_effects = effect_row_empty();

    for arg_expr in arg_exprs {
        let env_for_arg = apply_subst_env(&subst, env);
        let arg_result = infer_expr(&env_for_arg, arg_expr, interner)?;
        subst = compose_subst(&arg_result.subst, &subst);
        all_constraints.extend(arg_result.constraints);
        all_effects = combine_effect_rows(&all_effects, &arg_result.effects);
    }

    let result_type = match fn_name {
        "ffi-call" | "struct-get" => fresh_type_var(),
        "struct-new" => t_ffi_struct(),
        "struct-set" => t_unit(),
        "list" => type_app(type_const("List"), vec![fresh_type_var()]),
        _ => fresh_type_var(),
    };

    Ok(InferResult {
        ty: result_type,
        subst,
        constraints: all_constraints,
        effects: all_effects,
    })
}

fn infer_application(
    env: &TypeEnv,
    elements: &[&Expr],
    interner: &Interner,
) -> Result<InferResult, TypeError> {
    let fn_expr = elements[0];
    let arg_exprs = &elements[1..];

    // Check for variadic arithmetic
    if let Some(fn_name) = get_symbol(fn_expr, interner) {
        if variadic_arith_ops().contains_key(fn_name) {
            return infer_variadic_arith(env, fn_name, arg_exprs, interner);
        }
        if FFI_VARIADIC_FUNCTIONS.contains(&fn_name) {
            return infer_ffi_application(env, fn_name, arg_exprs, interner);
        }
    }

    let fn_result = infer_expr(env, fn_expr, interner)?;
    let mut subst = fn_result.subst;
    let mut all_constraints = fn_result.constraints;
    let mut all_effects = fn_result.effects;
    let mut arg_types = Vec::new();

    for arg_expr in arg_exprs {
        let env_for_arg = apply_subst_env(&subst, env);
        let arg_result = infer_expr(&env_for_arg, arg_expr, interner)?;
        subst = compose_subst(&arg_result.subst, &subst);
        all_constraints.extend(arg_result.constraints);
        all_effects = combine_effect_rows(&all_effects, &arg_result.effects);
        arg_types.push(arg_result.ty);
    }

    let result_type = fresh_type_var();
    let expected_fn_type = type_arrow(
        arg_types.iter().map(|t| apply_subst(&subst, t)).collect(),
        result_type.clone(),
    );

    let fn_type = apply_subst(&subst, &fn_result.ty);
    let app_subst = unify_with_context(&fn_type, &expected_fn_type, "function application")?;
    let final_subst = compose_subst(&app_subst, &subst);

    // Propagate function effects
    let applied_fn_type = apply_subst(&final_subst, &fn_result.ty);
    if let Type::Arrow { effects: Some(eff), .. } = &applied_fn_type {
        all_effects = combine_effect_rows(&all_effects, eff);
    }

    Ok(InferResult {
        ty: apply_subst(&final_subst, &result_type),
        subst: final_subst,
        constraints: all_constraints,
        effects: all_effects,
    })
}

// ===== Data Definitions =====

fn infer_data_def(
    _env: &TypeEnv,
    _elements: &[&Expr],
    _interner: &Interner,
) -> Result<InferResult, TypeError> {
    Ok(pure_result(t_unit()))
}

// ===== Effect System =====

fn parse_effect_op(
    expr: &Expr,
    env: &TypeEnv,
    interner: &Interner,
) -> Result<EffectOp, TypeError> {
    let elements = list_to_array(expr);
    if elements.len() != 2 {
        return Err(TypeError::InvalidTypeAnnotation {
            message: "Effect operation must be (name (-> ParamType ResultType))".into(),
            span: None,
        });
    }

    let op_name = get_symbol(elements[0], interner).ok_or(TypeError::InvalidTypeAnnotation {
        message: "Effect operation name must be a symbol".into(),
        span: None,
    })?;

    let type_expr = elements[1];
    let mut constraints: Vec<Constraint> = Vec::new();
    let mut arrow_expr = type_expr;

    // Check for constrained type: (=> constraints... arrow-type)
    if let Expr::List { .. } = type_expr {
        let type_elements = list_to_array(type_expr);
        if !type_elements.is_empty() && is_symbol(type_elements[0], interner, "=>") {
            if type_elements.len() < 3 {
                return Err(TypeError::InvalidTypeAnnotation {
                    message: "Constrained type needs constraints and a body".into(),
                    span: None,
                });
            }
            for e in &type_elements[1..type_elements.len() - 1] {
                constraints.push(parse_constraint(e, env, interner)?);
            }
            arrow_expr = type_elements[type_elements.len() - 1];
        }
    }

    let fn_type = parse_type_expr(arrow_expr, env, interner)?;
    let (params, result) = match &fn_type {
        Type::Arrow { params, result, .. } => (params.clone(), result.as_ref().clone()),
        _ => {
            return Err(TypeError::InvalidTypeAnnotation {
                message: "Effect operation type must be a function type (-> ...)".into(),
                span: None,
            });
        }
    };

    // Collect type parameters
    let mut type_params = Vec::new();
    let mut seen = HashSet::new();
    for c in &constraints {
        for v in free_type_vars(&c.type_arg) {
            if seen.insert(v.clone()) {
                type_params.push(v);
            }
        }
    }
    for p in &params {
        for v in free_type_vars(p) {
            if seen.insert(v.clone()) {
                type_params.push(v);
            }
        }
    }
    for v in free_type_vars(&result) {
        if seen.insert(v.clone()) {
            type_params.push(v);
        }
    }

    Ok(EffectOp {
        name: op_name.to_string(),
        type_params,
        constraints,
        params,
        result,
    })
}

fn infer_effect_def(
    _env: &TypeEnv,
    _elements: &[&Expr],
    _interner: &Interner,
) -> Result<InferResult, TypeError> {
    Ok(pure_result(t_unit()))
}

fn infer_perform(
    env: &TypeEnv,
    elements: &[&Expr],
    interner: &Interner,
) -> Result<InferResult, TypeError> {
    if elements.len() < 2 {
        return Err(TypeError::ArityMismatch {
            expected: 1,
            got: elements.len() - 1,
            context: "perform".into(),
            span: None,
        });
    }

    let op_name = get_symbol(elements[1], interner).ok_or(TypeError::InvalidTypeAnnotation {
        message: "perform operation name must be a symbol".into(),
        span: None,
    })?;

    let (effect, op) = lookup_effect_op(env, op_name).ok_or(TypeError::UndefinedVariable {
        name: format!("effect operation '{op_name}' (declare with (effect EffectName ({op_name} : ArgType -> ResultType)))"),
        span: None,
    })?;

    let arg_exprs = &elements[2..];
    let mut subst = empty_subst();
    let mut all_constraints = Vec::new();
    let mut all_effects = effect_row_empty();
    let mut arg_types = Vec::new();

    for arg_expr in arg_exprs {
        let env_for_arg = apply_subst_env(&subst, env);
        let arg_result = infer_expr(&env_for_arg, arg_expr, interner)?;
        subst = compose_subst(&arg_result.subst, &subst);
        all_constraints.extend(arg_result.constraints);
        all_effects = combine_effect_rows(&all_effects, &arg_result.effects);
        arg_types.push(arg_result.ty);
    }

    if arg_types.len() != op.params.len() {
        return Err(TypeError::ArityMismatch {
            expected: op.params.len(),
            got: arg_types.len(),
            context: format!("effect operation {op_name}"),
            span: None,
        });
    }

    // Instantiate type params
    let mut tp_subst = HashMap::new();
    for tv in &op.type_params {
        tp_subst.insert(tv.clone(), fresh_type_var());
    }

    let instantiated_params: Vec<Type> = op.params.iter().map(|p| apply_subst(&tp_subst, p)).collect();
    let instantiated_result = apply_subst(&tp_subst, &op.result);

    for c in &op.constraints {
        all_constraints.push(apply_subst_constraint(&tp_subst, c));
    }

    for (i, arg_type) in arg_types.iter().enumerate() {
        let at = apply_subst(&subst, arg_type);
        let pt = &instantiated_params[i];
        let unify_result = unify_with_context(&at, pt, &format!("argument {} of {op_name}", i + 1))?;
        subst = compose_subst(&unify_result, &subst);
    }

    let result_type = apply_subst(&subst, &instantiated_result);
    all_effects = combine_effect_rows(&all_effects, &effect_row_closed(vec![effect.name.clone()]));

    Ok(InferResult {
        ty: result_type,
        subst,
        constraints: all_constraints,
        effects: all_effects,
    })
}

fn infer_handle(
    env: &TypeEnv,
    elements: &[&Expr],
    interner: &Interner,
) -> Result<InferResult, TypeError> {
    if elements.len() < 2 {
        return Err(TypeError::ArityMismatch {
            expected: 1,
            got: elements.len() - 1,
            context: "handle".into(),
            span: None,
        });
    }

    let body_expr = elements[1];
    let body_result = infer_expr(env, body_expr, interner)?;

    let mut subst = body_result.subst.clone();
    let mut all_constraints = body_result.constraints.clone();
    let mut result_type: Option<Type> = None;
    let mut handled_effects: Vec<String> = Vec::new();

    let handler_clauses = &elements[2..];
    for clause in handler_clauses {
        let clause_elements = list_to_array(clause);
        if clause_elements.len() != 3 {
            return Err(TypeError::InvalidTypeAnnotation {
                message: "handle clause must be ((pattern) -> body)".into(),
                span: None,
            });
        }
        if !is_symbol(clause_elements[1], interner, "->") {
            return Err(TypeError::InvalidTypeAnnotation {
                message: "handle clause must be ((pattern) -> body)".into(),
                span: None,
            });
        }

        let pattern = clause_elements[0];
        let body = clause_elements[2];

        if let Expr::List { .. } = pattern {
            let pattern_elements = list_to_array(pattern);
            if pattern_elements.is_empty() {
                return Err(TypeError::InvalidTypeAnnotation {
                    message: "Handler pattern must have operation name".into(),
                    span: None,
                });
            }

            if let Some(op_name) = get_symbol(pattern_elements[0], interner) {
                if op_name == "return" {
                    // Return handler
                    let mut handler_env = apply_subst_env(&subst, env);
                    if pattern_elements.len() >= 2 {
                        if let Some(return_var_name) = get_symbol(pattern_elements[1], interner) {
                            let return_type = apply_subst(&subst, &body_result.ty);
                            handler_env = extend_env(&handler_env, return_var_name, TypeScheme {
                                quantified_vars: vec![],
                                qual_type: QualType { constraints: vec![], ty: return_type },
                            });
                        }
                    }

                    let handler_result = infer_expr(&handler_env, body, interner)?;
                    subst = compose_subst(&handler_result.subst, &subst);
                    all_constraints.extend(handler_result.constraints);

                    let handler_type = apply_subst(&subst, &handler_result.ty);
                    match &result_type {
                        None => result_type = Some(handler_type),
                        Some(rt) => {
                            let u = unify_with_context(rt, &handler_type, "handler return types")?;
                            subst = compose_subst(&u, &subst);
                            result_type = Some(apply_subst(&subst, rt));
                        }
                    }
                    continue;
                }

                // Effect handler
                handled_effects.push(op_name.to_string());
                let op_info = lookup_effect_op(env, op_name).ok_or(TypeError::UndefinedVariable {
                    name: format!("effect operation '{op_name}' in handler (declare with (effect EffectName ({op_name} : ArgType -> ResultType)))"),
                    span: None,
                })?;

                let mut handler_env = apply_subst_env(&subst, env);
                let arg_patterns = &pattern_elements[1..pattern_elements.len().saturating_sub(1)];
                let continuation_pattern = pattern_elements.last();

                // Bind arguments
                for (i, arg_p) in arg_patterns.iter().enumerate() {
                    if let Some(arg_name) = get_symbol(arg_p, interner) {
                        if i < op_info.1.params.len() {
                            handler_env = extend_env(&handler_env, arg_name, TypeScheme {
                                quantified_vars: vec![],
                                qual_type: QualType { constraints: vec![], ty: op_info.1.params[i].clone() },
                            });
                        }
                    }
                }

                // Bind continuation
                if let Some(cont_p) = continuation_pattern {
                    if let Some(k_name) = get_symbol(cont_p, interner) {
                        let k_type = type_arrow(
                            vec![op_info.1.result.clone()],
                            apply_subst(&subst, &body_result.ty),
                        );
                        handler_env = extend_env(&handler_env, k_name, TypeScheme {
                            quantified_vars: vec![],
                            qual_type: QualType { constraints: vec![], ty: k_type },
                        });
                    }
                }

                let handler_result = infer_expr(&handler_env, body, interner)?;
                subst = compose_subst(&handler_result.subst, &subst);
                all_constraints.extend(handler_result.constraints);

                let handler_type = apply_subst(&subst, &handler_result.ty);
                match &result_type {
                    None => result_type = Some(handler_type),
                    Some(rt) => {
                        let u = unify_with_context(rt, &handler_type, "handler return types")?;
                        subst = compose_subst(&u, &subst);
                        result_type = Some(apply_subst(&subst, rt));
                    }
                }
            }
        }
    }

    let final_type = result_type.unwrap_or_else(|| body_result.ty.clone());

    // Remove handled effects
    let mut remaining_effects = body_result.effects.clone();
    if !handled_effects.is_empty() {
        let remaining: Vec<String> = remaining_effects
            .effects()
            .iter()
            .filter(|e| !handled_effects.contains(e))
            .cloned()
            .collect();
        remaining_effects = match &remaining_effects {
            EffectRow::Open { row_var, .. } => effect_row_open(remaining, row_var.clone()),
            EffectRow::Closed(_) => effect_row_closed(remaining),
        };
    }

    Ok(InferResult {
        ty: apply_subst(&subst, &final_type),
        subst,
        constraints: all_constraints,
        effects: remaining_effects,
    })
}

fn infer_resume(
    env: &TypeEnv,
    elements: &[&Expr],
    interner: &Interner,
) -> Result<InferResult, TypeError> {
    if elements.len() != 3 {
        return Err(TypeError::ArityMismatch {
            expected: 2,
            got: elements.len() - 1,
            context: "resume".into(),
            span: None,
        });
    }

    let cont_result = infer_expr(env, elements[1], interner)?;
    let mut subst = cont_result.subst;
    let mut all_constraints = cont_result.constraints;
    let mut all_effects = cont_result.effects;

    let value_env = apply_subst_env(&subst, env);
    let value_result = infer_expr(&value_env, elements[2], interner)?;
    subst = compose_subst(&value_result.subst, &subst);
    all_constraints.extend(value_result.constraints);
    all_effects = combine_effect_rows(&all_effects, &value_result.effects);

    let return_type = fresh_type_var();
    let value_type = apply_subst(&subst, &value_result.ty);
    let cont_type = apply_subst(&subst, &cont_result.ty);

    let expected_cont_type = type_arrow(vec![value_type], return_type.clone());
    let unify_result = unify_with_context(&cont_type, &expected_cont_type, "resume continuation")?;
    subst = compose_subst(&unify_result, &subst);

    Ok(InferResult {
        ty: apply_subst(&subst, &return_type),
        subst,
        constraints: all_constraints,
        effects: all_effects,
    })
}

// ===== Program-Level Inference =====

fn process_data_def(
    env: &TypeEnv,
    elements: &[&Expr],
    interner: &Interner,
) -> Result<TypeEnv, TypeError> {
    let type_expr = elements[1];
    let ctor_exprs = &elements[2..];

    let type_name: String;
    let mut type_params: Vec<String> = Vec::new();

    if let Expr::List { .. } = type_expr {
        let type_elements = list_to_array(type_expr);
        type_name = get_symbol(type_elements[0], interner)
            .ok_or(TypeError::InvalidTypeAnnotation {
                message: "data type name must be a symbol".into(),
                span: None,
            })?
            .to_string();
        for te in &type_elements[1..] {
            type_params.push(
                get_symbol(te, interner)
                    .ok_or(TypeError::InvalidTypeAnnotation {
                        message: "type parameter must be a symbol".into(),
                        span: None,
                    })?
                    .to_string(),
            );
        }
    } else {
        type_name = get_symbol(type_expr, interner)
            .ok_or(TypeError::InvalidTypeAnnotation {
                message: "data type name must be a symbol".into(),
                span: None,
            })?
            .to_string();
    }

    let result_type = if type_params.is_empty() {
        type_const(&type_name)
    } else {
        type_app(
            type_const(&type_name),
            type_params.iter().map(|p| type_var(p)).collect(),
        )
    };

    let mut constructors = Vec::new();

    for ctor_expr in ctor_exprs {
        match ctor_expr {
            Expr::Atom(LispValue::Symbol(sym), _) => {
                let ctor_name = interner.resolve(*sym).to_string();
                constructors.push(Constructor {
                    name: ctor_name,
                    params: vec![],
                    result_type: result_type.clone(),
                });
            }
            Expr::List { .. } => {
                let ctor_elements = list_to_array(ctor_expr);
                let ctor_name = get_symbol(ctor_elements[0], interner)
                    .ok_or(TypeError::InvalidTypeAnnotation {
                        message: "constructor name must be a symbol".into(),
                        span: None,
                    })?
                    .to_string();

                let mut params = Vec::new();
                for ce in &ctor_elements[1..] {
                    params.push(parse_type_expr(ce, env, interner)?);
                }

                constructors.push(Constructor {
                    name: ctor_name,
                    params,
                    result_type: result_type.clone(),
                });
            }
            _ => {}
        }
    }

    let data_type = DataType {
        name: type_name,
        type_params: type_params.clone(),
        constructors: constructors.clone(),
    };

    let mut new_env = define_data_type(env, data_type);

    for ctor in &constructors {
        let ctor_type = if ctor.params.is_empty() {
            ctor.result_type.clone()
        } else {
            type_arrow(ctor.params.clone(), ctor.result_type.clone())
        };

        let scheme = TypeScheme {
            quantified_vars: type_params.clone(),
            qual_type: QualType {
                constraints: vec![],
                ty: ctor_type,
            },
        };

        new_env = extend_env(&new_env, &ctor.name, scheme);
    }

    Ok(new_env)
}

fn process_define(
    env: &TypeEnv,
    elements: &[&Expr],
    current_subst: &Substitution,
    interner: &Interner,
) -> Result<(TypeEnv, Substitution), TypeError> {
    let name_expr = elements[1];
    let value_expr = elements[2];

    let name: String;
    let mut annotated_type: Option<Type> = None;
    let mut annotated_constraints: Vec<Constraint> = Vec::new();

    if let Expr::List { .. } = name_expr {
        let name_elements = list_to_array(name_expr);
        if name_elements.len() == 3 && is_symbol(name_elements[1], interner, ":") {
            name = get_symbol(name_elements[0], interner)
                .ok_or(TypeError::InvalidTypeAnnotation {
                    message: "Expected symbol before :".into(),
                    span: None,
                })?
                .to_string();

            let (ty, cs) = parse_type_annotation(name_elements[2], env, interner)?;
            annotated_type = Some(ty);
            annotated_constraints = cs;
        } else {
            return Err(TypeError::InvalidTypeAnnotation {
                message: "Invalid define syntax".into(),
                span: None,
            });
        }
    } else {
        name = get_symbol(name_expr, interner)
            .ok_or(TypeError::InvalidTypeAnnotation {
                message: "define expects a symbol name".into(),
                span: None,
            })?
            .to_string();
    }

    // Add placeholder for recursive functions
    let placeholder_type = annotated_type.clone().unwrap_or_else(fresh_type_var);
    let infer_env = extend_env(env, &name, TypeScheme {
        quantified_vars: vec![],
        qual_type: QualType {
            constraints: annotated_constraints.clone(),
            ty: placeholder_type,
        },
    });

    let applied_env = apply_subst_env(current_subst, &infer_env);
    let value_result = infer_expr(&applied_env, value_expr, interner)?;

    let mut subst = compose_subst(&value_result.subst, current_subst);
    let mut value_type = value_result.ty;

    if let Some(ref ann_type) = annotated_type {
        let unify_result = unify_with_context(ann_type, &value_type, &format!("definition of {name}"))?;
        subst = compose_subst(&unify_result, &subst);
        value_type = apply_subst(&subst, ann_type);
    }

    let mut all_constraints = annotated_constraints;
    all_constraints.extend(value_result.constraints);
    let gen_env = apply_subst_env(&subst, env);
    let scheme = generalize(&gen_env, &all_constraints, &apply_subst(&subst, &value_type));

    let new_env = extend_env(env, &name, scheme);
    Ok((new_env, subst))
}

fn process_effect_def(
    env: &TypeEnv,
    elements: &[&Expr],
    interner: &Interner,
) -> Result<TypeEnv, TypeError> {
    if elements.len() < 2 {
        return Err(TypeError::ArityMismatch {
            expected: 1,
            got: elements.len() - 1,
            context: "effect".into(),
            span: None,
        });
    }

    let name_expr = elements[1];
    let effect_name = match name_expr {
        Expr::Atom(LispValue::Symbol(sym), _) => interner.resolve(*sym).to_string(),
        _ => {
            return Err(TypeError::InvalidTypeAnnotation {
                message: "effect name must be a symbol".into(),
                span: None,
            });
        }
    };

    let mut operations = Vec::new();
    for e in &elements[2..] {
        operations.push(parse_effect_op(e, env, interner)?);
    }

    let effect_decl = EffectDeclaration {
        name: effect_name,
        type_params: vec![],
        operations,
    };

    Ok(define_effect(env, effect_decl))
}

pub fn infer_program(
    env: &TypeEnv,
    exprs: &[Expr],
    interner: &Interner,
) -> Result<(TypeEnv, Vec<Type>), TypeError> {
    let mut current_env = env.clone();
    let mut types = Vec::new();
    let mut global_subst = empty_subst();

    for expr in exprs {
        if let Expr::List { .. } = expr {
            let elements = list_to_array(expr);
            if !elements.is_empty() {
                let first_sym = get_symbol(elements[0], interner);

                if first_sym == Some("data") {
                    current_env = process_data_def(&current_env, &elements, interner)?;
                    types.push(t_unit());
                    continue;
                }

                if first_sym == Some("define") && elements.len() >= 3 {
                    let (new_env, new_subst) =
                        process_define(&current_env, &elements, &global_subst, interner)?;
                    current_env = new_env;
                    global_subst = new_subst;
                    types.push(t_unit());
                    continue;
                }

                if first_sym == Some("effect") && elements.len() >= 2 {
                    current_env = process_effect_def(&current_env, &elements, interner)?;
                    types.push(t_unit());
                    continue;
                }

                if first_sym == Some("import") {
                    // Import stubbed — skip
                    types.push(t_unit());
                    continue;
                }
            }
        }

        // Regular expression
        let applied_env = apply_subst_env(&global_subst, &current_env);
        let result = infer_expr(&applied_env, expr, interner)?;

        global_subst = compose_subst(&result.subst, &global_subst);
        types.push(apply_subst(&global_subst, &result.ty));

        // Check constraints
        let substituted_constraints: Vec<Constraint> = result
            .constraints
            .iter()
            .map(|c| Constraint {
                class: c.class.clone(),
                type_arg: apply_subst(&global_subst, &c.type_arg),
            })
            .collect();

        let resolved = resolve_constraints(&substituted_constraints);
        for c in &resolved {
            if !matches!(c.type_arg, Type::Var(_)) {
                return Err(TypeError::NoInstance {
                    constraint: c.clone(),
                    span: None,
                });
            }
        }
    }

    Ok((current_env, types))
}
