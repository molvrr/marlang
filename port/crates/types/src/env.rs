use crate::ast::*;
use std::collections::HashMap;
use std::collections::HashSet;
use std::rc::Rc;

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

/// Maximum number of type nodes apply_subst will produce before bailing out.
/// This prevents exponential blowup from cyclic or deeply-branching substitutions
/// (e.g. a → μa. App(a, a)) while allowing arbitrarily deep but narrow types.
const APPLY_SUBST_MAX_NODES: usize = 2_000;

pub fn apply_subst(subst: &Substitution, t: &Type) -> Type {
    let mut budget = APPLY_SUBST_MAX_NODES;
    apply_subst_counted(subst, t, &mut budget)
}

fn apply_subst_counted(subst: &Substitution, t: &Type, budget: &mut usize) -> Type {
    if *budget == 0 {
        return t.clone();
    }
    *budget -= 1;
    match t {
        Type::Var(_) => {
            // Chase Var → Var chains to find the terminal binding, then
            // structurally walk the resolved type.
            let resolved = chase_subst(subst, t);
            if std::ptr::eq(resolved, t) {
                t.clone()
            } else if let Type::Var(_) = resolved {
                resolved.clone()
            } else {
                apply_subst_counted(subst, resolved, budget)
            }
        }
        Type::Const(_) => t.clone(),
        Type::App { constructor, args } => Type::App {
            constructor: Box::new(apply_subst_counted(subst, constructor, budget)),
            args: args
                .iter()
                .map(|a| apply_subst_counted(subst, a, budget))
                .collect(),
        },
        Type::Arrow {
            params,
            rest,
            effects,
            result,
        } => Type::Arrow {
            params: params
                .iter()
                .map(|p| apply_subst_counted(subst, p, budget))
                .collect(),
            rest: rest
                .as_ref()
                .map(|r| Box::new(apply_subst_counted(subst, r, budget))),
            effects: effects
                .as_ref()
                .map(|row| apply_effect_row_subst_via_type_subst(subst, row)),
            result: Box::new(apply_subst_counted(subst, result, budget)),
        },
        Type::Rec { var, body } => {
            let mut filtered = subst.clone();
            filtered.remove(var);
            Type::Rec {
                var: var.clone(),
                body: Box::new(apply_subst_counted(&filtered, body, budget)),
            }
        }
        Type::EffectRowRef(row) => {
            Type::EffectRowRef(apply_effect_row_subst_via_type_subst(subst, row))
        }
    }
}

/// Apply a type substitution to an effect row.
/// Effect row variables (ε0, ε1, ...) are looked up in the type substitution.
/// If the substitution maps a row var to Type::EffectRowRef(row), that row is
/// spliced in (combining any existing effects). If it maps to Type::Var(new_var),
/// the row var is renamed.
pub fn apply_effect_row_subst_via_type_subst(subst: &Substitution, row: &EffectRow) -> EffectRow {
    match row {
        EffectRow::Closed(_) => row.clone(),
        EffectRow::Open { effects, row_var } => match subst.get(row_var) {
            None => row.clone(),
            Some(replacement) => {
                // Chase substitution chains
                let resolved = chase_subst(subst, replacement);
                match resolved {
                    Type::Var(new_var) => EffectRow::Open {
                        effects: effects.clone(),
                        row_var: new_var.clone(),
                    },
                    Type::EffectRowRef(inner_row) => {
                        // Combine: prepend our effects to the inner row
                        let mut combined = effects.clone();
                        match &inner_row {
                            EffectRow::Closed(r_effects) => {
                                for e in r_effects {
                                    if !combined.contains(e) {
                                        combined.push(e.clone());
                                    }
                                }
                                EffectRow::Closed(combined)
                            }
                            EffectRow::Open {
                                effects: r_effects,
                                row_var: r_var,
                            } => {
                                for e in r_effects {
                                    if !combined.contains(e) {
                                        combined.push(e.clone());
                                    }
                                }
                                EffectRow::Open {
                                    effects: combined,
                                    row_var: r_var.clone(),
                                }
                            }
                        }
                    }
                    _ => row.clone(), // Shouldn't happen, leave unchanged
                }
            }
        },
    }
}

/// Chase a substitution chain: if subst[x] = Var(y) and subst[y] = ..., follow.
/// Uses a step limit to prevent infinite loops on cyclic substitutions
/// (e.g. a → Var(b), b → Var(a)).
fn chase_subst<'a>(subst: &'a Substitution, t: &'a Type) -> &'a Type {
    chase_subst_step(subst, t, 64)
}

fn chase_subst_step<'a>(subst: &'a Substitution, t: &'a Type, remaining: usize) -> &'a Type {
    if remaining == 0 {
        return t;
    }
    if let Type::Var(name) = t
        && let Some(next) = subst.get(name)
    {
        return chase_subst_step(subst, next, remaining - 1);
    }
    t
}

pub fn apply_subst_scheme(subst: &Substitution, scheme: &TypeScheme) -> TypeScheme {
    let mut filtered = subst.clone();
    for v in &scheme.quantified_vars {
        filtered.remove(v);
    }
    TypeScheme {
        quantified_vars: scheme.quantified_vars.clone(),
        ty: apply_subst(&filtered, &scheme.ty),
    }
}

pub fn apply_subst_env(subst: &Substitution, env: &TypeEnv) -> TypeEnv {
    if subst.is_empty() {
        return env.clone();
    }
    // O(|subst|): compose with any existing pending substitution.
    // The actual application to bindings is deferred until lookup.
    let composed = match &env.pending_subst {
        None => Rc::new(subst.clone()),
        Some(existing) => Rc::new(compose_subst(subst, existing)),
    };
    TypeEnv {
        inner: Rc::clone(&env.inner),
        parent: env.parent.clone(),
        pending_subst: Some(composed),
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

/// The inner data of a type environment, shared via Rc for cheap cloning.
#[derive(Debug, Clone)]
struct TypeEnvInner {
    bindings: HashMap<String, TypeScheme>,
    data_types: HashMap<String, DataType>,
    constructors: HashMap<String, (DataType, Constructor)>,
    aliases: HashMap<String, TypeAlias>,
    effects: HashMap<String, EffectDeclaration>,
    foreign_libs: HashMap<String, Vec<ForeignFnDecl>>,
    ffi_struct_types: HashMap<String, FfiStructInfo>,
}

/// A type environment that supports lazy substitution.
///
/// The environment stores bindings and type metadata. A pending substitution
/// can be attached, which is applied lazily on lookup rather than eagerly
/// to every binding. This turns `apply_subst_env` from O(n) to O(|subst|).
///
/// The parent chain uses Rc for structural sharing — cloning a TypeEnv is O(1)
/// (just incrementing reference counts), and creating child environments does
/// not require materializing pending substitutions.
#[derive(Debug, Clone)]
pub struct TypeEnv {
    inner: Rc<TypeEnvInner>,
    /// Parent environment, shared via Rc for cheap cloning.
    parent: Option<Rc<TypeEnv>>,
    /// Pending substitution to be applied lazily on lookup.
    /// Applies to bindings in `inner` and the entire `parent` chain.
    pending_subst: Option<Rc<Substitution>>,
}

impl TypeEnv {
    /// Access data_types.
    pub fn data_types(&self) -> &HashMap<String, DataType> {
        &self.inner.data_types
    }

    /// Access constructors.
    pub fn constructors(&self) -> &HashMap<String, (DataType, Constructor)> {
        &self.inner.constructors
    }

    /// Access aliases.
    pub fn aliases(&self) -> &HashMap<String, TypeAlias> {
        &self.inner.aliases
    }

    /// Access effects.
    pub fn effects(&self) -> &HashMap<String, EffectDeclaration> {
        &self.inner.effects
    }

    /// Collect all effects from this env and all parent envs.
    pub fn all_effects(&self) -> HashMap<String, EffectDeclaration> {
        let mut result = HashMap::new();
        // Parent effects first (lower priority)
        if let Some(parent) = &self.parent {
            result.extend(parent.all_effects());
        }
        // Current level overrides
        result.extend(self.inner.effects.clone());
        result
    }

    /// Access foreign_libs.
    pub fn foreign_libs(&self) -> &HashMap<String, Vec<ForeignFnDecl>> {
        &self.inner.foreign_libs
    }

    /// Access ffi_struct_types.
    pub fn ffi_struct_types(&self) -> &HashMap<String, FfiStructInfo> {
        &self.inner.ffi_struct_types
    }

    /// Access bindings (raw, without applying pending substitution).
    /// Use `lookup_var` for substitution-aware access.
    pub fn bindings(&self) -> &HashMap<String, TypeScheme> {
        &self.inner.bindings
    }

    /// Get mutable access to the bindings (copy-on-write).
    /// New entries inserted here will NOT have the pending substitution applied
    /// to them — they are assumed to already be in the "post-substitution" state.
    pub fn bindings_mut(&mut self) -> &mut HashMap<String, TypeScheme> {
        &mut Rc::make_mut(&mut self.inner).bindings
    }

    /// Get mutable access to data_types (copy-on-write).
    pub fn data_types_mut(&mut self) -> &mut HashMap<String, DataType> {
        &mut Rc::make_mut(&mut self.inner).data_types
    }

    /// Get mutable access to constructors (copy-on-write).
    pub fn constructors_mut(&mut self) -> &mut HashMap<String, (DataType, Constructor)> {
        &mut Rc::make_mut(&mut self.inner).constructors
    }

    /// Get mutable access to aliases (copy-on-write).
    pub fn aliases_mut(&mut self) -> &mut HashMap<String, TypeAlias> {
        &mut Rc::make_mut(&mut self.inner).aliases
    }

    /// Get mutable access to effects (copy-on-write).
    pub fn effects_mut(&mut self) -> &mut HashMap<String, EffectDeclaration> {
        &mut Rc::make_mut(&mut self.inner).effects
    }

    /// Get mutable access to foreign_libs (copy-on-write).
    pub fn foreign_libs_mut(&mut self) -> &mut HashMap<String, Vec<ForeignFnDecl>> {
        &mut Rc::make_mut(&mut self.inner).foreign_libs
    }

    /// Get mutable access to ffi_struct_types (copy-on-write).
    pub fn ffi_struct_types_mut(&mut self) -> &mut HashMap<String, FfiStructInfo> {
        &mut Rc::make_mut(&mut self.inner).ffi_struct_types
    }

    /// Get a mutable reference to the inner data, cloning if shared.
    fn inner_mut(&mut self) -> &mut TypeEnvInner {
        Rc::make_mut(&mut self.inner)
    }
}

pub fn empty_env() -> TypeEnv {
    TypeEnv {
        inner: Rc::new(TypeEnvInner {
            bindings: HashMap::new(),
            data_types: HashMap::new(),
            constructors: HashMap::new(),
            aliases: HashMap::new(),
            effects: HashMap::new(),
            foreign_libs: HashMap::new(),
            ffi_struct_types: HashMap::new(),
        }),
        parent: None,
        pending_subst: None,
    }
}

pub fn child_env(parent: &TypeEnv) -> TypeEnv {
    // O(1): just Rc-share the parent. No materialization needed.
    TypeEnv {
        inner: Rc::new(TypeEnvInner {
            bindings: HashMap::new(),
            data_types: HashMap::new(),
            constructors: HashMap::new(),
            aliases: HashMap::new(),
            effects: HashMap::new(),
            foreign_libs: HashMap::new(),
            ffi_struct_types: HashMap::new(),
        }),
        parent: Some(Rc::new(parent.clone())),
        pending_subst: None,
    }
}

pub fn extend_env(env: &TypeEnv, name: &str, scheme: TypeScheme) -> TypeEnv {
    // Clone-on-write: get a mutable copy of the inner data and insert.
    let mut new_env = env.clone();
    new_env
        .inner_mut()
        .bindings
        .insert(name.to_string(), scheme);
    new_env
}

pub fn extend_env_many(env: &TypeEnv, bindings: &[(&str, TypeScheme)]) -> TypeEnv {
    let mut new_env = env.clone();
    let inner = new_env.inner_mut();
    for (name, scheme) in bindings {
        inner.bindings.insert(name.to_string(), scheme.clone());
    }
    new_env
}

// ===== Lookups (recurse through parent) =====

pub fn lookup_var(env: &TypeEnv, name: &str) -> Option<TypeScheme> {
    // Search in current bindings
    if let Some(scheme) = env.inner.bindings.get(name) {
        return Some(match &env.pending_subst {
            None => scheme.clone(),
            Some(subst) => apply_subst_scheme(subst, scheme),
        });
    }
    // Recurse into parent chain. The parent carries its own pending_subst.
    // We also need to apply our pending_subst to the result.
    let parent = env.parent.as_ref()?;
    let result = lookup_var(parent, name)?;
    Some(match &env.pending_subst {
        None => result,
        Some(subst) => apply_subst_scheme(subst, &result),
    })
}

pub fn lookup_data_type(env: &TypeEnv, name: &str) -> Option<DataType> {
    if let Some(dt) = env.inner.data_types.get(name) {
        return Some(dt.clone());
    }
    env.parent.as_ref().and_then(|p| lookup_data_type(p, name))
}

pub fn lookup_constructor(env: &TypeEnv, name: &str) -> Option<(DataType, Constructor)> {
    if let Some(entry) = env.inner.constructors.get(name) {
        return Some(entry.clone());
    }
    env.parent
        .as_ref()
        .and_then(|p| lookup_constructor(p, name))
}

pub fn lookup_effect(env: &TypeEnv, name: &str) -> Option<EffectDeclaration> {
    if let Some(eff) = env.inner.effects.get(name) {
        return Some(eff.clone());
    }
    env.parent.as_ref().and_then(|p| lookup_effect(p, name))
}

pub fn lookup_effect_op(env: &TypeEnv, op_name: &str) -> Option<(EffectDeclaration, EffectOp)> {
    for eff in env.inner.effects.values() {
        for op in eff.operations.iter().rev() {
            if op.name == op_name {
                return Some((eff.clone(), op.clone()));
            }
        }
    }
    env.parent
        .as_ref()
        .and_then(|p| lookup_effect_op(p, op_name))
}

pub fn lookup_effect_op_in_effect(
    env: &TypeEnv,
    effect_name: &str,
    op_name: &str,
) -> Option<EffectOp> {
    let effect = lookup_effect(env, effect_name)?;
    effect
        .operations
        .iter()
        .find(|op| op.name == op_name)
        .cloned()
}

pub fn find_effects_with_op(env: &TypeEnv, op_name: &str) -> Vec<String> {
    let mut effect_names = Vec::new();
    find_effects_with_op_inner(env, op_name, &mut effect_names);
    effect_names.sort();
    effect_names
}

fn find_effects_with_op_inner(env: &TypeEnv, op_name: &str, result: &mut Vec<String>) {
    for (name, effect) in &env.inner.effects {
        if effect.operations.iter().any(|op| op.name == op_name) && !result.contains(name) {
            result.push(name.clone());
        }
    }
    if let Some(parent) = &env.parent {
        find_effects_with_op_inner(parent, op_name, result);
    }
}

// ===== Define operations =====

pub fn define_data_type(env: &TypeEnv, dt: DataType) -> TypeEnv {
    let mut new_env = env.clone();
    let inner = new_env.inner_mut();
    for ctor in &dt.constructors {
        inner
            .constructors
            .insert(ctor.name.clone(), (dt.clone(), ctor.clone()));
    }
    inner.data_types.insert(dt.name.clone(), dt);
    new_env
}

pub fn define_effect(env: &TypeEnv, effect: EffectDeclaration) -> TypeEnv {
    let mut new_env = env.clone();
    new_env
        .inner_mut()
        .effects
        .insert(effect.name.clone(), effect);
    new_env
}

pub fn define_foreign(env: &TypeEnv, lib_name: &str, functions: Vec<ForeignFnDecl>) -> TypeEnv {
    let mut new_env = env.clone();
    new_env
        .inner_mut()
        .foreign_libs
        .insert(lib_name.to_string(), functions);
    new_env
}

pub fn lookup_foreign_fn<'a>(
    env: &'a TypeEnv,
    lib_name: &str,
    fn_name: &str,
) -> Option<&'a ForeignFnDecl> {
    if let Some(fns) = env.inner.foreign_libs.get(lib_name) {
        for f in fns {
            if f.name == fn_name {
                return Some(f);
            }
        }
    }
    env.parent
        .as_ref()
        .and_then(|p| lookup_foreign_fn(p, lib_name, fn_name))
}

pub fn has_foreign_lib(env: &TypeEnv, lib_name: &str) -> bool {
    if env.inner.foreign_libs.contains_key(lib_name) {
        return true;
    }
    env.parent
        .as_ref()
        .is_some_and(|p| has_foreign_lib(p, lib_name))
}

pub fn register_ffi_struct_type(env: &mut TypeEnv, name: &str, info: FfiStructInfo) {
    env.inner_mut()
        .ffi_struct_types
        .insert(name.to_string(), info);
}

pub fn is_ffi_struct_type(env: &TypeEnv, name: &str) -> bool {
    if env.inner.ffi_struct_types.contains_key(name) {
        return true;
    }
    env.parent
        .as_ref()
        .is_some_and(|p| is_ffi_struct_type(p, name))
}

pub fn lookup_ffi_struct_info<'a>(env: &'a TypeEnv, name: &str) -> Option<&'a FfiStructInfo> {
    if let Some(info) = env.inner.ffi_struct_types.get(name) {
        return Some(info);
    }
    env.parent
        .as_ref()
        .and_then(|p| lookup_ffi_struct_info(p, name))
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
        Type::Arrow {
            params,
            rest,
            effects,
            result,
        } => Type::Arrow {
            params: params.iter().map(|p| rewrite_type(p, name_map)).collect(),
            rest: rest.as_ref().map(|r| Box::new(rewrite_type(r, name_map))),
            effects: effects
                .as_ref()
                .map(|row| rewrite_effect_row(row, name_map)),
            result: Box::new(rewrite_type(result, name_map)),
        },
        Type::Rec { var, body } => Type::Rec {
            var: var.clone(),
            body: Box::new(rewrite_type(body, name_map)),
        },
        Type::EffectRowRef(row) => Type::EffectRowRef(rewrite_effect_row(row, name_map)),
    }
}

fn rewrite_effect_row(row: &EffectRow, name_map: &HashMap<String, String>) -> EffectRow {
    match row {
        EffectRow::Closed(effects) => EffectRow::Closed(
            effects
                .iter()
                .map(|e| name_map.get(e).cloned().unwrap_or_else(|| e.clone()))
                .collect(),
        ),
        EffectRow::Open { effects, row_var } => EffectRow::Open {
            effects: effects
                .iter()
                .map(|e| name_map.get(e).cloned().unwrap_or_else(|| e.clone()))
                .collect(),
            row_var: row_var.clone(),
        },
    }
}

fn rewrite_type_scheme(scheme: &TypeScheme, name_map: &HashMap<String, String>) -> TypeScheme {
    TypeScheme {
        quantified_vars: scheme.quantified_vars.clone(),
        ty: rewrite_type(&scheme.ty, name_map),
    }
}

fn rewrite_constructor(ctor: &Constructor, name_map: &HashMap<String, String>) -> Constructor {
    Constructor {
        name: ctor.name.clone(),
        params: ctor
            .params
            .iter()
            .map(|p| rewrite_type(p, name_map))
            .collect(),
        result_type: rewrite_type(&ctor.result_type, name_map),
    }
}

fn rewrite_data_type(
    dt: &DataType,
    name_map: &HashMap<String, String>,
    new_name: &str,
) -> DataType {
    DataType {
        name: new_name.to_string(),
        type_params: dt.type_params.clone(),
        constructors: dt
            .constructors
            .iter()
            .map(|c| rewrite_constructor(c, name_map))
            .collect(),
    }
}

fn rewrite_effect_decl(
    effect: &EffectDeclaration,
    name_map: &HashMap<String, String>,
    new_name: &str,
) -> EffectDeclaration {
    EffectDeclaration {
        name: new_name.to_string(),
        type_params: effect.type_params.clone(),
        operations: effect
            .operations
            .iter()
            .map(|op| EffectOp {
                name: op.name.clone(),
                type_params: op.type_params.clone(),
                params: op
                    .params
                    .iter()
                    .map(|p| rewrite_type(p, name_map))
                    .collect(),
                result: rewrite_type(&op.result, name_map),
            })
            .collect(),
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
        for name in source.inner.data_types.keys() {
            if !target.inner.data_types.contains_key(name) {
                type_name_map.insert(name.clone(), transform_name(name));
            }
        }
        for name in source.inner.effects.keys() {
            if !target.inner.effects.contains_key(name) {
                type_name_map.insert(name.clone(), transform_name(name));
            }
        }
        for name in source.inner.aliases.keys() {
            if !target.inner.aliases.contains_key(name) {
                type_name_map.insert(name.clone(), transform_name(name));
            }
        }
    }

    let inner = result.inner_mut();

    // Merge bindings
    for (name, scheme) in &source.inner.bindings {
        if should_import(name) {
            let new_scheme = if is_qualified {
                rewrite_type_scheme(scheme, &type_name_map)
            } else {
                scheme.clone()
            };
            inner.bindings.insert(transform_name(name), new_scheme);
        }
    }

    // Merge data types
    for (name, dt) in &source.inner.data_types {
        if should_import(name) {
            let new_name = transform_name(name);
            let new_dt = if is_qualified {
                rewrite_data_type(dt, &type_name_map, &new_name)
            } else {
                dt.clone()
            };
            inner.data_types.insert(new_name.clone(), new_dt.clone());
            // Also register data types under unqualified names so pattern matching
            // and exhaustiveness checking work without the module prefix.
            if is_qualified && !inner.data_types.contains_key(name) {
                inner.data_types.insert(name.clone(), new_dt);
            }
        }
    }

    // Merge constructors
    for (name, (dt, ctor)) in &source.inner.constructors {
        if should_import(name) {
            let new_name = transform_name(name);
            if is_qualified {
                let rewritten_ctor = rewrite_constructor(ctor, &type_name_map);
                let dt_new_name = transform_name(&dt.name);
                let rewritten_dt = rewrite_data_type(dt, &type_name_map, &dt_new_name);
                inner
                    .constructors
                    .insert(new_name, (rewritten_dt.clone(), rewritten_ctor.clone()));
                // Also register constructors under unqualified names so they work
                // in pattern matching without the module prefix. ADT constructors
                // are PascalCase and typically unique, so collisions are rare.
                // Only insert if not already present (don't shadow prelude constructors).
                if !inner.constructors.contains_key(name) {
                    inner
                        .constructors
                        .insert(name.clone(), (rewritten_dt, rewritten_ctor));
                }
            } else {
                inner
                    .constructors
                    .insert(new_name, (dt.clone(), ctor.clone()));
            }
        }
    }

    // Merge effects
    for (name, effect) in &source.inner.effects {
        if should_import(name) {
            let new_name = transform_name(name);
            let new_effect = if is_qualified {
                rewrite_effect_decl(effect, &type_name_map, &new_name)
            } else {
                effect.clone()
            };
            inner.effects.insert(new_name, new_effect);
        }
    }

    // Merge aliases
    for (name, alias) in &source.inner.aliases {
        if should_import(name) {
            let new_name = transform_name(name);
            if is_qualified {
                let rewritten_alias = TypeAlias {
                    name: new_name.clone(),
                    type_params: alias.type_params.clone(),
                    body: rewrite_type(&alias.body, &type_name_map),
                };
                inner.aliases.insert(new_name, rewritten_alias);
            } else {
                inner.aliases.insert(new_name, alias.clone());
            }
        }
    }

    result
}

// ===== Generalization and Instantiation =====

pub fn free_type_vars_env(env: &TypeEnv) -> HashSet<String> {
    let mut result = HashSet::new();
    free_type_vars_env_inner(env, &mut result);
    result
}

fn free_type_vars_env_inner(env: &TypeEnv, result: &mut HashSet<String>) {
    // Apply pending substitution to each scheme when computing free vars
    match &env.pending_subst {
        None => {
            for scheme in env.inner.bindings.values() {
                result.extend(free_type_vars_scheme(scheme));
            }
        }
        Some(subst) => {
            for scheme in env.inner.bindings.values() {
                let applied = apply_subst_scheme(subst, scheme);
                result.extend(free_type_vars_scheme(&applied));
            }
        }
    }
    // Recurse into parent (which carries its own pending_subst)
    if let Some(parent) = &env.parent {
        free_type_vars_env_inner(parent, result);
    }
}

/// Level-based generalization: quantify all type variables in `t` whose level
/// is strictly greater than the current binding level. This is O(|free_vars(t)|)
/// instead of O(|env|), eliminating the need to walk the entire environment.
pub fn generalize(_env: &TypeEnv, t: &Type) -> TypeScheme {
    let type_free = free_type_vars(t);
    let cur_level = current_level();

    let mut quantified: Vec<String> = type_free
        .iter()
        .filter(|v| get_var_level(v) > cur_level)
        .cloned()
        .collect();
    quantified.sort(); // Deterministic ordering

    TypeScheme {
        quantified_vars: quantified,
        ty: t.clone(),
    }
}

pub fn instantiate(scheme: &TypeScheme) -> Type {
    let mut subst = HashMap::new();
    for v in &scheme.quantified_vars {
        if v.starts_with('ε') {
            // Effect row variable: replace with a fresh open effect row
            let fresh_rv = fresh_effect_row_var();
            subst.insert(
                v.clone(),
                Type::EffectRowRef(EffectRow::Open {
                    effects: vec![],
                    row_var: fresh_rv,
                }),
            );
        } else {
            subst.insert(v.clone(), fresh_type_var());
        }
    }
    apply_subst(&subst, &scheme.ty)
}
