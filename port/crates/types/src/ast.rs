use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::HashSet;

// ===== Effect System Types =====

#[derive(Debug, Clone, PartialEq)]
pub enum EffectRow {
    Closed(Vec<String>),
    Open {
        effects: Vec<String>,
        row_var: String,
    },
}

impl EffectRow {
    pub fn effects(&self) -> &[String] {
        match self {
            EffectRow::Closed(e) => e,
            EffectRow::Open { effects, .. } => effects,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EffectOp {
    pub name: String,
    pub type_params: Vec<String>,
    pub params: Vec<Type>,
    pub result: Type,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EffectDeclaration {
    pub name: String,
    pub type_params: Vec<String>,
    pub operations: Vec<EffectOp>,
}

// ===== FFI Foreign Declarations =====

#[derive(Debug, Clone, PartialEq)]
pub struct ForeignFnDecl {
    pub name: String,
    pub params: Vec<Type>,
    pub result: Type,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ForeignDeclaration {
    pub lib_name: String,
    pub functions: Vec<ForeignFnDecl>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FfiStructFieldInfo {
    pub name: String,
    pub marlang_type: Type,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FfiStructInfo {
    pub name: String,
    pub fields: Vec<FfiStructFieldInfo>,
}

// ===== Core Type Representation =====

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    Var(String),
    Const(String),
    App {
        constructor: Box<Type>,
        args: Vec<Type>,
    },
    Arrow {
        params: Vec<Type>,
        rest: Option<Box<Type>>,
        effects: Option<EffectRow>,
        result: Box<Type>,
    },
    Rec {
        var: String,
        body: Box<Type>,
    },
    /// Internal-only: wraps an EffectRow so it can be stored in a Substitution.
    /// Never appears in user-visible types or formatted output.
    EffectRowRef(EffectRow),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeScheme {
    pub quantified_vars: Vec<String>,
    pub ty: Type,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Constructor {
    pub name: String,
    pub params: Vec<Type>,
    pub result_type: Type,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DataType {
    pub name: String,
    pub type_params: Vec<String>,
    pub constructors: Vec<Constructor>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    Wildcard,
    Var(String),
    Literal(LiteralPattern),
    Constructor { name: String, args: Vec<Pattern> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum LiteralPattern {
    Number(f64),
    String(String),
    Bool(bool),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeAlias {
    pub name: String,
    pub type_params: Vec<String>,
    pub body: Type,
}

// ===== Constructors =====

pub fn type_var(name: &str) -> Type {
    Type::Var(name.to_string())
}

pub fn type_const(name: &str) -> Type {
    Type::Const(name.to_string())
}

pub fn type_app(constructor: Type, args: Vec<Type>) -> Type {
    Type::App {
        constructor: Box::new(constructor),
        args,
    }
}

pub fn type_arrow(params: Vec<Type>, result: Type) -> Type {
    Type::Arrow {
        params,
        rest: None,
        effects: None,
        result: Box::new(result),
    }
}

pub fn type_arrow_variadic(params: Vec<Type>, rest: Type, result: Type) -> Type {
    Type::Arrow {
        params,
        rest: Some(Box::new(rest)),
        effects: None,
        result: Box::new(result),
    }
}

pub fn type_arrow_with_effects(
    params: Vec<Type>,
    effects: Option<EffectRow>,
    result: Type,
) -> Type {
    Type::Arrow {
        params,
        rest: None,
        effects,
        result: Box::new(result),
    }
}

pub fn type_arrow_variadic_with_effects(
    params: Vec<Type>,
    rest: Type,
    effects: Option<EffectRow>,
    result: Type,
) -> Type {
    Type::Arrow {
        params,
        rest: Some(Box::new(rest)),
        effects,
        result: Box::new(result),
    }
}

pub fn type_rec(var: &str, body: Type) -> Type {
    Type::Rec {
        var: var.to_string(),
        body: Box::new(body),
    }
}

pub fn effect_row_closed(effects: Vec<String>) -> EffectRow {
    EffectRow::Closed(effects)
}

pub fn effect_row_open(effects: Vec<String>, row_var: String) -> EffectRow {
    EffectRow::Open { effects, row_var }
}

pub fn effect_row_empty() -> EffectRow {
    EffectRow::Closed(vec![])
}

// ===== Built-in Types =====

pub fn t_int() -> Type {
    type_const("Int")
}
pub fn t_float() -> Type {
    type_const("Float")
}
pub fn t_bool() -> Type {
    type_const("Bool")
}
pub fn t_string() -> Type {
    type_const("String")
}
pub fn t_char() -> Type {
    type_const("Char")
}
pub fn t_unit() -> Type {
    type_const("Unit")
}
pub fn t_list(elem: Type) -> Type {
    type_app(type_const("List"), vec![elem])
}
pub fn t_option(elem: Type) -> Type {
    type_app(type_const("Option"), vec![elem])
}
pub fn t_ffi_library() -> Type {
    type_const("FFILibrary")
}
pub fn t_ffi_struct_def() -> Type {
    type_const("FFIStructDef")
}
pub fn t_ffi_struct() -> Type {
    type_const("FFIStruct")
}
pub fn t_ffi_pointer() -> Type {
    type_const("FFIPointer")
}
pub fn t_symbol() -> Type {
    type_const("Symbol")
}
pub fn t_pid() -> Type {
    type_const("Pid")
}
pub fn t_channel() -> Type {
    type_const("Channel")
}
pub fn t_bytes() -> Type {
    type_const("Bytes")
}

// ===== Type Scheme Helpers =====

pub fn monotype(t: Type) -> TypeScheme {
    TypeScheme {
        quantified_vars: vec![],
        ty: t,
    }
}

pub fn polytype(vars: Vec<&str>, t: Type) -> TypeScheme {
    TypeScheme {
        quantified_vars: vars.into_iter().map(String::from).collect(),
        ty: t,
    }
}

// ===== Fresh Type Variable Generation =====

thread_local! {
    static TYPE_VAR_COUNTER: Cell<u32> = const { Cell::new(0) };
    static EFFECT_ROW_VAR_COUNTER: Cell<u32> = const { Cell::new(0) };
    /// Current binding level for level-based generalization.
    /// Incremented when entering a let/define scope, decremented on exit.
    static CURRENT_LEVEL: Cell<u32> = const { Cell::new(0) };
    /// Maps type variable names to the level at which they were created.
    /// Used by `generalize` to determine which vars to quantify.
    static VAR_LEVELS: RefCell<HashMap<String, u32>> = RefCell::new(HashMap::new());
}

pub fn fresh_type_var() -> Type {
    fresh_type_var_with_prefix("t")
}

pub fn fresh_type_var_with_prefix(prefix: &str) -> Type {
    TYPE_VAR_COUNTER.with(|c| {
        let n = c.get();
        c.set(n + 1);
        let name = format!("{prefix}{n}");
        // Stamp the variable with the current binding level
        let level = current_level();
        set_var_level(&name, level);
        Type::Var(name)
    })
}

pub fn reset_type_var_counter() {
    TYPE_VAR_COUNTER.with(|c| c.set(0));
}

pub fn fresh_effect_row_var() -> String {
    EFFECT_ROW_VAR_COUNTER.with(|c| {
        let n = c.get();
        c.set(n + 1);
        let name = format!("ε{n}");
        // Stamp effect row variables with the current level too
        let level = current_level();
        set_var_level(&name, level);
        name
    })
}

pub fn reset_effect_row_var_counter() {
    EFFECT_ROW_VAR_COUNTER.with(|c| c.set(0));
}

// ===== Level-Based Generalization =====

/// Get the current binding level.
pub fn current_level() -> u32 {
    CURRENT_LEVEL.with(|c| c.get())
}

/// Enter a new binding scope (increment level).
pub fn enter_level() {
    CURRENT_LEVEL.with(|c| c.set(c.get() + 1));
}

/// Exit a binding scope (decrement level).
pub fn exit_level() {
    CURRENT_LEVEL.with(|c| {
        let cur = c.get();
        debug_assert!(cur > 0, "exit_level called at level 0");
        c.set(cur - 1);
    });
}

/// Reset the level to 0 and clear the var_levels table.
pub fn reset_levels() {
    CURRENT_LEVEL.with(|c| c.set(0));
    VAR_LEVELS.with(|m| m.borrow_mut().clear());
}

/// Get the level of a type variable. Returns 0 if not found (treat as global).
pub fn get_var_level(name: &str) -> u32 {
    VAR_LEVELS.with(|m| m.borrow().get(name).copied().unwrap_or(0))
}

/// Set the level of a type variable.
pub fn set_var_level(name: &str, level: u32) {
    VAR_LEVELS.with(|m| {
        m.borrow_mut().insert(name.to_string(), level);
    });
}

/// Update a variable's level to be the minimum of its current level and the given level.
/// This is called during unification when `Var(a) -> Var(b)`: b's level should be
/// min(level_a, level_b) to prevent premature generalization.
pub fn lower_var_level(name: &str, level: u32) {
    VAR_LEVELS.with(|m| {
        let mut map = m.borrow_mut();
        let current = map.get(name).copied().unwrap_or(0);
        if level < current {
            map.insert(name.to_string(), level);
        }
    });
}

// ===== Free Type Variables =====

pub fn free_type_vars(t: &Type) -> HashSet<String> {
    match t {
        Type::Var(name) => {
            let mut s = HashSet::new();
            s.insert(name.clone());
            s
        }
        Type::Const(_) => HashSet::new(),
        Type::App { constructor, args } => {
            let mut result = free_type_vars(constructor);
            for arg in args {
                result.extend(free_type_vars(arg));
            }
            result
        }
        Type::Arrow {
            params,
            rest,
            effects,
            result,
        } => {
            let mut s = HashSet::new();
            for p in params {
                s.extend(free_type_vars(p));
            }
            if let Some(rest_ty) = rest {
                s.extend(free_type_vars(rest_ty));
            }
            s.extend(free_type_vars(result));
            if let Some(row) = effects {
                s.extend(free_effect_row_vars(row));
            }
            s
        }
        Type::Rec { var, body } => {
            let mut s = free_type_vars(body);
            s.remove(var);
            s
        }
        Type::EffectRowRef(row) => free_effect_row_vars(row),
    }
}

pub fn free_effect_row_vars(row: &EffectRow) -> HashSet<String> {
    match row {
        EffectRow::Closed(_) => HashSet::new(),
        EffectRow::Open { row_var, .. } => {
            let mut s = HashSet::new();
            s.insert(row_var.clone());
            s
        }
    }
}

pub fn free_type_vars_scheme(scheme: &TypeScheme) -> HashSet<String> {
    let mut s = free_type_vars(&scheme.ty);
    for v in &scheme.quantified_vars {
        s.remove(v);
    }
    s
}

// ===== Formatting =====

pub fn format_effect_row(row: &EffectRow) -> String {
    match row {
        EffectRow::Closed(effects) => {
            if effects.is_empty() {
                "{}".to_string()
            } else {
                format!("{{{}}}", effects.join(", "))
            }
        }
        EffectRow::Open { effects, row_var } => {
            if effects.is_empty() {
                format!("{{{row_var}}}")
            } else {
                format!("{{{} | {row_var}}}", effects.join(", "))
            }
        }
    }
}

pub fn format_type(t: &Type) -> String {
    match t {
        Type::Var(name) => name.clone(),
        Type::Const(name) => name.clone(),
        Type::App { constructor, args } => {
            if args.is_empty() {
                format_type(constructor)
            } else {
                let args_str: Vec<_> = args.iter().map(format_type).collect();
                format!("({} {})", format_type(constructor), args_str.join(" "))
            }
        }
        Type::Arrow {
            params,
            rest,
            effects,
            result,
        } => {
            let params_str: Vec<_> = params.iter().map(format_type).collect();
            let rest_str = match rest {
                Some(rest_ty) => format!(" (... {})", format_type(rest_ty)),
                None => String::new(),
            };
            let effects_str = match effects {
                Some(row) => format!(" {}", format_effect_row(row)),
                None => String::new(),
            };
            if params.is_empty() && rest.is_none() {
                format!("(->{effects_str} {})", format_type(result))
            } else {
                format!(
                    "(-> {}{rest_str}{effects_str} {})",
                    params_str.join(" "),
                    format_type(result)
                )
            }
        }
        Type::Rec { var, body } => {
            format!("(mu {var} {})", format_type(body))
        }
        Type::EffectRowRef(row) => {
            // Should not appear in user-facing output, but format for debugging
            format_effect_row(row)
        }
    }
}

pub fn format_scheme(scheme: &TypeScheme) -> String {
    let mut result = String::new();
    if !scheme.quantified_vars.is_empty() {
        result.push_str(&format!("(forall {} ", scheme.quantified_vars.join(" ")));
    }
    result.push_str(&format_type(&scheme.ty));
    if !scheme.quantified_vars.is_empty() {
        result.push(')');
    }
    result
}

pub fn format_pattern(p: &Pattern) -> String {
    match p {
        Pattern::Wildcard => "_".to_string(),
        Pattern::Var(name) => name.clone(),
        Pattern::Literal(lit) => match lit {
            LiteralPattern::Number(n) => format!("{n}"),
            LiteralPattern::String(s) => format!("\"{s}\""),
            LiteralPattern::Bool(b) => {
                if *b {
                    "#t".to_string()
                } else {
                    "#f".to_string()
                }
            }
        },
        Pattern::Constructor { name, args } => {
            if args.is_empty() {
                name.clone()
            } else {
                let args_str: Vec<_> = args.iter().map(format_pattern).collect();
                format!("({name} {})", args_str.join(" "))
            }
        }
    }
}
