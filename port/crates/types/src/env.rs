use std::collections::HashMap;
use std::collections::HashSet;

use crate::ast::*;

// ===== Substitution =====

pub type Substitution = HashMap<String, Type>;
pub type EffectRowSubstitution = HashMap<String, EffectRow>;

pub fn empty_subst() -> Substitution {
    HashMap::new()
}

pub fn single_subst(name: &str, t: Type) -> Substitution {
    let mut s = HashMap::new();
    s.insert(name.to_string(), t);
    s
}

// ===== Apply Substitution =====

pub fn apply_subst(subst: &Substitution, t: &Type) -> Type {
    match t {
        Type::Var(name) => match subst.get(name) {
            Some(replacement) => apply_subst(subst, replacement),
            None => t.clone(),
        },
        Type::Const(_) => t.clone(),
        Type::App { constructor, args } => Type::App {
            constructor: Box::new(apply_subst(subst, constructor)),
            args: args.iter().map(|a| apply_subst(subst, a)).collect(),
        },
        Type::Arrow {
            params,
            effects,
            result,
        } => Type::Arrow {
            params: params.iter().map(|p| apply_subst(subst, p)).collect(),
            effects: effects.clone(),
            result: Box::new(apply_subst(subst, result)),
        },
        Type::Rec { var, body } => {
            let mut filtered = subst.clone();
            filtered.remove(var);
            Type::Rec {
                var: var.clone(),
                body: Box::new(apply_subst(&filtered, body)),
            }
        }
    }
}

pub fn apply_subst_constraint(subst: &Substitution, c: &Constraint) -> Constraint {
    Constraint {
        class: c.class.clone(),
        type_arg: apply_subst(subst, &c.type_arg),
    }
}

pub fn apply_subst_scheme(subst: &Substitution, scheme: &TypeScheme) -> TypeScheme {
    let mut filtered = subst.clone();
    for v in &scheme.quantified_vars {
        filtered.remove(v);
    }
    TypeScheme {
        quantified_vars: scheme.quantified_vars.clone(),
        qual_type: QualType {
            constraints: scheme
                .qual_type
                .constraints
                .iter()
                .map(|c| apply_subst_constraint(&filtered, c))
                .collect(),
            ty: apply_subst(&filtered, &scheme.qual_type.ty),
        },
    }
}

pub fn apply_subst_env(subst: &Substitution, env: &TypeEnv) -> TypeEnv {
    let mut new_bindings = HashMap::new();
    for (name, scheme) in &env.bindings {
        new_bindings.insert(name.clone(), apply_subst_scheme(subst, scheme));
    }
    let parent = env
        .parent
        .as_ref()
        .map(|p| Box::new(apply_subst_env(subst, p)));
    TypeEnv {
        bindings: new_bindings,
        data_types: env.data_types.clone(),
        constructors: env.constructors.clone(),
        classes: env.classes.clone(),
        instances: env.instances.clone(),
        aliases: env.aliases.clone(),
        effects: env.effects.clone(),
        foreign_libs: env.foreign_libs.clone(),
        ffi_struct_types: env.ffi_struct_types.clone(),
        parent,
    }
}

pub fn apply_effect_row_subst(subst: &EffectRowSubstitution, row: &EffectRow) -> EffectRow {
    match row {
        EffectRow::Closed(_) => row.clone(),
        EffectRow::Open { effects, row_var } => match subst.get(row_var) {
            None => row.clone(),
            Some(replacement) => {
                let mut combined = effects.clone();
                match replacement {
                    EffectRow::Closed(r_effects) => {
                        combined.extend(r_effects.iter().cloned());
                        EffectRow::Closed(combined)
                    }
                    EffectRow::Open {
                        effects: r_effects,
                        row_var: r_var,
                    } => {
                        combined.extend(r_effects.iter().cloned());
                        EffectRow::Open {
                            effects: combined,
                            row_var: r_var.clone(),
                        }
                    }
                }
            }
        },
    }
}

// ===== Compose Substitutions =====

pub fn compose_subst(s1: &Substitution, s2: &Substitution) -> Substitution {
    let mut result = HashMap::new();
    // Apply s1 to all values in s2
    for (name, t) in s2 {
        result.insert(name.clone(), apply_subst(s1, t));
    }
    // Add all bindings from s1 that aren't in s2
    for (name, t) in s1 {
        if !result.contains_key(name) {
            result.insert(name.clone(), t.clone());
        }
    }
    result
}

// ===== Type Environment =====

#[derive(Debug, Clone)]
pub struct TypeEnv {
    pub bindings: HashMap<String, TypeScheme>,
    pub data_types: HashMap<String, DataType>,
    pub constructors: HashMap<String, (DataType, Constructor)>,
    pub classes: HashMap<String, TypeClass>,
    pub instances: HashMap<String, Vec<ClassInstance>>,
    pub aliases: HashMap<String, TypeAlias>,
    pub effects: HashMap<String, EffectDeclaration>,
    pub foreign_libs: HashMap<String, Vec<ForeignFnDecl>>,
    pub ffi_struct_types: HashMap<String, FfiStructInfo>,
    pub parent: Option<Box<TypeEnv>>,
}

pub fn empty_env() -> TypeEnv {
    TypeEnv {
        bindings: HashMap::new(),
        data_types: HashMap::new(),
        constructors: HashMap::new(),
        classes: HashMap::new(),
        instances: HashMap::new(),
        aliases: HashMap::new(),
        effects: HashMap::new(),
        foreign_libs: HashMap::new(),
        ffi_struct_types: HashMap::new(),
        parent: None,
    }
}

pub fn child_env(parent: &TypeEnv) -> TypeEnv {
    TypeEnv {
        bindings: HashMap::new(),
        data_types: HashMap::new(),
        constructors: HashMap::new(),
        classes: HashMap::new(),
        instances: HashMap::new(),
        aliases: HashMap::new(),
        effects: HashMap::new(),
        foreign_libs: HashMap::new(),
        ffi_struct_types: HashMap::new(),
        parent: Some(Box::new(parent.clone())),
    }
}

pub fn extend_env(env: &TypeEnv, name: &str, scheme: TypeScheme) -> TypeEnv {
    let mut new_env = env.clone();
    new_env.bindings.insert(name.to_string(), scheme);
    new_env
}

pub fn extend_env_many(env: &TypeEnv, bindings: &[(&str, TypeScheme)]) -> TypeEnv {
    let mut new_env = env.clone();
    for (name, scheme) in bindings {
        new_env.bindings.insert(name.to_string(), scheme.clone());
    }
    new_env
}

// ===== Lookups (recurse through parent) =====

pub fn lookup_var(env: &TypeEnv, name: &str) -> Option<TypeScheme> {
    if let Some(scheme) = env.bindings.get(name) {
        return Some(scheme.clone());
    }
    if let Some(parent) = &env.parent {
        return lookup_var(parent, name);
    }
    None
}

pub fn lookup_data_type(env: &TypeEnv, name: &str) -> Option<DataType> {
    if let Some(dt) = env.data_types.get(name) {
        return Some(dt.clone());
    }
    if let Some(parent) = &env.parent {
        return lookup_data_type(parent, name);
    }
    None
}

pub fn lookup_constructor(env: &TypeEnv, name: &str) -> Option<(DataType, Constructor)> {
    if let Some(entry) = env.constructors.get(name) {
        return Some(entry.clone());
    }
    if let Some(parent) = &env.parent {
        return lookup_constructor(parent, name);
    }
    None
}

pub fn lookup_class(env: &TypeEnv, name: &str) -> Option<TypeClass> {
    if let Some(cls) = env.classes.get(name) {
        return Some(cls.clone());
    }
    if let Some(parent) = &env.parent {
        return lookup_class(parent, name);
    }
    None
}

pub fn lookup_effect(env: &TypeEnv, name: &str) -> Option<EffectDeclaration> {
    if let Some(eff) = env.effects.get(name) {
        return Some(eff.clone());
    }
    if let Some(parent) = &env.parent {
        return lookup_effect(parent, name);
    }
    None
}

pub fn lookup_effect_op(env: &TypeEnv, op_name: &str) -> Option<(EffectDeclaration, EffectOp)> {
    let mut current: Option<&TypeEnv> = Some(env);
    while let Some(e) = current {
        // Search in reverse insertion order
        let effects: Vec<_> = e.effects.values().collect();
        for eff in effects.iter().rev() {
            for op in eff.operations.iter().rev() {
                if op.name == op_name {
                    return Some(((*eff).clone(), op.clone()));
                }
            }
        }
        current = e.parent.as_deref();
    }
    None
}

// ===== Define operations =====

pub fn define_data_type(env: &TypeEnv, dt: DataType) -> TypeEnv {
    let mut new_env = env.clone();
    for ctor in &dt.constructors {
        new_env
            .constructors
            .insert(ctor.name.clone(), (dt.clone(), ctor.clone()));
    }
    new_env.data_types.insert(dt.name.clone(), dt);
    new_env
}

pub fn define_class(env: &TypeEnv, cls: TypeClass) -> TypeEnv {
    let mut new_env = env.clone();
    new_env.classes.insert(cls.name.clone(), cls);
    new_env
}

pub fn define_instance(env: &TypeEnv, instance: ClassInstance) -> TypeEnv {
    let mut new_env = env.clone();
    let existing = new_env
        .instances
        .entry(instance.class_name.clone())
        .or_default();
    existing.push(instance);
    new_env
}

pub fn define_effect(env: &TypeEnv, effect: EffectDeclaration) -> TypeEnv {
    let mut new_env = env.clone();
    new_env.effects.insert(effect.name.clone(), effect);
    new_env
}

pub fn define_foreign(env: &TypeEnv, lib_name: &str, functions: Vec<ForeignFnDecl>) -> TypeEnv {
    let mut new_env = env.clone();
    new_env
        .foreign_libs
        .insert(lib_name.to_string(), functions);
    new_env
}

pub fn lookup_foreign_fn<'a>(
    env: &'a TypeEnv,
    lib_name: &str,
    fn_name: &str,
) -> Option<&'a ForeignFnDecl> {
    if let Some(fns) = env.foreign_libs.get(lib_name) {
        for f in fns {
            if f.name == fn_name {
                return Some(f);
            }
        }
    }
    if let Some(parent) = &env.parent {
        return lookup_foreign_fn(parent, lib_name, fn_name);
    }
    None
}

pub fn has_foreign_lib(env: &TypeEnv, lib_name: &str) -> bool {
    if env.foreign_libs.contains_key(lib_name) {
        return true;
    }
    if let Some(parent) = &env.parent {
        return has_foreign_lib(parent, lib_name);
    }
    false
}

pub fn register_ffi_struct_type(env: &mut TypeEnv, name: &str, info: FfiStructInfo) {
    env.ffi_struct_types.insert(name.to_string(), info);
}

pub fn is_ffi_struct_type(env: &TypeEnv, name: &str) -> bool {
    if env.ffi_struct_types.contains_key(name) {
        return true;
    }
    if let Some(parent) = &env.parent {
        return is_ffi_struct_type(parent, name);
    }
    false
}

pub fn lookup_ffi_struct_info<'a>(env: &'a TypeEnv, name: &str) -> Option<&'a FfiStructInfo> {
    if let Some(info) = env.ffi_struct_types.get(name) {
        return Some(info);
    }
    if let Some(parent) = &env.parent {
        return lookup_ffi_struct_info(parent, name);
    }
    None
}

// ===== Import Form =====

pub enum ImportForm {
    All,
    Selective(Vec<String>),
    Qualified(String),
}

// ===== Type Rewriting (for qualified imports) =====

/// Rewrite Type::Const names based on a name mapping.
fn rewrite_type(t: &Type, name_map: &HashMap<String, String>) -> Type {
    match t {
        Type::Var(_) => t.clone(),
        Type::Const(name) => match name_map.get(name) {
            Some(new_name) => Type::Const(new_name.clone()),
            None => t.clone(),
        },
        Type::App { constructor, args } => Type::App {
            constructor: Box::new(rewrite_type(constructor, name_map)),
            args: args.iter().map(|a| rewrite_type(a, name_map)).collect(),
        },
        Type::Arrow { params, effects, result } => Type::Arrow {
            params: params.iter().map(|p| rewrite_type(p, name_map)).collect(),
            effects: effects.as_ref().map(|row| rewrite_effect_row(row, name_map)),
            result: Box::new(rewrite_type(result, name_map)),
        },
        Type::Rec { var, body } => Type::Rec {
            var: var.clone(),
            body: Box::new(rewrite_type(body, name_map)),
        },
    }
}

fn rewrite_effect_row(row: &EffectRow, name_map: &HashMap<String, String>) -> EffectRow {
    match row {
        EffectRow::Closed(effects) => EffectRow::Closed(
            effects.iter().map(|e| name_map.get(e).cloned().unwrap_or_else(|| e.clone())).collect(),
        ),
        EffectRow::Open { effects, row_var } => EffectRow::Open {
            effects: effects.iter().map(|e| name_map.get(e).cloned().unwrap_or_else(|| e.clone())).collect(),
            row_var: row_var.clone(),
        },
    }
}

fn rewrite_constraint(c: &Constraint, name_map: &HashMap<String, String>) -> Constraint {
    Constraint {
        class: c.class.clone(),
        type_arg: rewrite_type(&c.type_arg, name_map),
    }
}

fn rewrite_type_scheme(scheme: &TypeScheme, name_map: &HashMap<String, String>) -> TypeScheme {
    TypeScheme {
        quantified_vars: scheme.quantified_vars.clone(),
        qual_type: QualType {
            constraints: scheme.qual_type.constraints.iter().map(|c| rewrite_constraint(c, name_map)).collect(),
            ty: rewrite_type(&scheme.qual_type.ty, name_map),
        },
    }
}

fn rewrite_constructor(ctor: &Constructor, name_map: &HashMap<String, String>) -> Constructor {
    Constructor {
        name: ctor.name.clone(),
        params: ctor.params.iter().map(|p| rewrite_type(p, name_map)).collect(),
        result_type: rewrite_type(&ctor.result_type, name_map),
    }
}

fn rewrite_data_type(dt: &DataType, name_map: &HashMap<String, String>, new_name: &str) -> DataType {
    DataType {
        name: new_name.to_string(),
        type_params: dt.type_params.clone(),
        constructors: dt.constructors.iter().map(|c| rewrite_constructor(c, name_map)).collect(),
    }
}

fn rewrite_effect_decl(effect: &EffectDeclaration, name_map: &HashMap<String, String>, new_name: &str) -> EffectDeclaration {
    EffectDeclaration {
        name: new_name.to_string(),
        type_params: effect.type_params.clone(),
        operations: effect.operations.iter().map(|op| EffectOp {
            name: op.name.clone(),
            type_params: op.type_params.clone(),
            constraints: op.constraints.iter().map(|c| rewrite_constraint(c, name_map)).collect(),
            params: op.params.iter().map(|p| rewrite_type(p, name_map)).collect(),
            result: rewrite_type(&op.result, name_map),
        }).collect(),
    }
}

// ===== Module Environment Merging =====

/// Merge a module's type environment into the target environment according to the import form.
/// For qualified imports, type names that are NEW in the source (not already in target)
/// get prefixed with the qualifier, and all type references are rewritten accordingly.
pub fn merge_module_env(target: &TypeEnv, source: &TypeEnv, form: &ImportForm) -> TypeEnv {
    let mut result = target.clone();

    let should_import = |name: &str| -> bool {
        match form {
            ImportForm::All => true,
            ImportForm::Selective(names) => names.iter().any(|n| n == name),
            ImportForm::Qualified(_) => true,
        }
    };

    let transform_name = |name: &str| -> String {
        match form {
            ImportForm::Qualified(q) => format!("{}/{}", q, name),
            _ => name.to_string(),
        }
    };

    let is_qualified = matches!(form, ImportForm::Qualified(_));

    // Build type name map for qualified imports
    let mut type_name_map: HashMap<String, String> = HashMap::new();
    if is_qualified {
        for name in source.data_types.keys() {
            if !target.data_types.contains_key(name) {
                type_name_map.insert(name.clone(), transform_name(name));
            }
        }
        for name in source.effects.keys() {
            if !target.effects.contains_key(name) {
                type_name_map.insert(name.clone(), transform_name(name));
            }
        }
        for name in source.aliases.keys() {
            if !target.aliases.contains_key(name) {
                type_name_map.insert(name.clone(), transform_name(name));
            }
        }
    }

    // Merge bindings
    for (name, scheme) in &source.bindings {
        if should_import(name) {
            let new_scheme = if is_qualified {
                rewrite_type_scheme(scheme, &type_name_map)
            } else {
                scheme.clone()
            };
            result.bindings.insert(transform_name(name), new_scheme);
        }
    }

    // Merge data types
    for (name, dt) in &source.data_types {
        if should_import(name) {
            let new_name = transform_name(name);
            let new_dt = if is_qualified {
                rewrite_data_type(dt, &type_name_map, &new_name)
            } else {
                dt.clone()
            };
            result.data_types.insert(new_name, new_dt);
        }
    }

    // Merge constructors
    for (name, (dt, ctor)) in &source.constructors {
        if should_import(name) {
            let new_name = transform_name(name);
            if is_qualified {
                let rewritten_ctor = rewrite_constructor(ctor, &type_name_map);
                let dt_new_name = transform_name(&dt.name);
                let rewritten_dt = rewrite_data_type(dt, &type_name_map, &dt_new_name);
                result.constructors.insert(new_name, (rewritten_dt, rewritten_ctor));
            } else {
                result.constructors.insert(new_name, (dt.clone(), ctor.clone()));
            }
        }
    }

    // Merge effects
    for (name, effect) in &source.effects {
        if should_import(name) {
            let new_name = transform_name(name);
            let new_effect = if is_qualified {
                rewrite_effect_decl(effect, &type_name_map, &new_name)
            } else {
                effect.clone()
            };
            result.effects.insert(new_name, new_effect);
        }
    }

    // Merge aliases
    for (name, alias) in &source.aliases {
        if should_import(name) {
            let new_name = transform_name(name);
            if is_qualified {
                let rewritten_alias = TypeAlias {
                    name: new_name.clone(),
                    type_params: alias.type_params.clone(),
                    body: rewrite_type(&alias.body, &type_name_map),
                };
                result.aliases.insert(new_name, rewritten_alias);
            } else {
                result.aliases.insert(new_name, alias.clone());
            }
        }
    }

    result
}

// ===== Generalization and Instantiation =====

pub fn free_type_vars_env(env: &TypeEnv) -> HashSet<String> {
    let mut result = HashSet::new();
    for scheme in env.bindings.values() {
        result.extend(free_type_vars_scheme(scheme));
    }
    if let Some(parent) = &env.parent {
        result.extend(free_type_vars_env(parent));
    }
    result
}

pub fn generalize(env: &TypeEnv, constraints: &[Constraint], t: &Type) -> TypeScheme {
    let env_free = free_type_vars_env(env);
    let mut type_free = free_type_vars(t);

    // Also include free vars from constraints
    for c in constraints {
        type_free.extend(free_type_vars(&c.type_arg));
    }

    let mut quantified: Vec<String> = type_free
        .iter()
        .filter(|v| !env_free.contains(*v))
        .cloned()
        .collect();
    quantified.sort(); // Deterministic ordering

    let quantified_set: HashSet<_> = quantified.iter().cloned().collect();
    let relevant_constraints: Vec<Constraint> = constraints
        .iter()
        .filter(|c| {
            let c_free = free_type_vars(&c.type_arg);
            c_free.iter().any(|v| quantified_set.contains(v))
        })
        .cloned()
        .collect();

    TypeScheme {
        quantified_vars: quantified,
        qual_type: QualType {
            constraints: relevant_constraints,
            ty: t.clone(),
        },
    }
}

pub fn instantiate(scheme: &TypeScheme) -> (Type, Vec<Constraint>) {
    let mut subst = HashMap::new();
    for v in &scheme.quantified_vars {
        subst.insert(v.clone(), fresh_type_var());
    }
    let ty = apply_subst(&subst, &scheme.qual_type.ty);
    let constraints = scheme
        .qual_type
        .constraints
        .iter()
        .map(|c| apply_subst_constraint(&subst, c))
        .collect();
    (ty, constraints)
}
