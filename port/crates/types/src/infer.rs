use crate::ast::*;
use crate::checker::{ModuleCacheState, TypeModuleCache, TypeModuleLoader};
use crate::env::*;
use crate::errors::TypeError;
use crate::unify::*;
use marlang_core::ast::{self, Expr, LispValue};
use marlang_core::intern::Interner;
use marlang_core::span::Span;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

// ===== Runtime-Handled Effects =====

/// Effects that the VM/scheduler handles implicitly at the top level.
/// Top-level expressions may use these without explicit `handle` blocks.
const RUNTIME_HANDLED_EFFECTS: &[&str] = &[
    "IO",
    "Exception",
    "Fiber",
    "Channel",
    "Actor",
    "Tcp",
    "HttpClient",
    "ParseState", // VM-managed fast-path for codec parsing
];

// ===== Inference Session =====

/// The kind of symbol being defined or referenced.
/// Used to distinguish between different namespaces so that, e.g.,
/// a variable `println` and an effect operation `println` don't collide
/// in go-to-definition lookups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    /// A variable binding: top-level define, let binding, lambda parameter.
    Variable,
    /// An effect operation: declared inside an `(effect ...)` form.
    EffectOp,
    /// An effect name: the name in `(effect Name ...)`.
    Effect,
}

/// Bundles the interner with an optional context collector for LSP features.
///
/// This replaces the bare `&Interner` parameter throughout the inference engine.
/// When `context` is `None` (the default for CLI usage), there is zero overhead.
/// When `Some`, the inference engine records type information, definitions, and
/// references for hover and go-to-definition support.
pub struct InferSession<'a> {
    pub interner: &'a Interner,
    pub context: Option<&'a RefCell<InferContext>>,
}

impl<'a> InferSession<'a> {
    /// Create a session without context collection (CLI usage).
    pub fn new(interner: &'a Interner) -> Self {
        Self {
            interner,
            context: None,
        }
    }

    /// Create a session with context collection (LSP usage).
    pub fn with_context(interner: &'a Interner, context: &'a RefCell<InferContext>) -> Self {
        Self {
            interner,
            context: Some(context),
        }
    }

    /// Record a type for an expression span, if context collection is active.
    fn record_type(&self, span: Span, ty: &Type) {
        if let Some(ctx) = &self.context {
            ctx.borrow_mut().type_map.push((span, ty.clone()));
        }
    }

    /// Record a definition site, if context collection is active.
    fn record_definition(&self, name: &str, span: Span, kind: SymbolKind) {
        if let Some(ctx) = &self.context {
            ctx.borrow_mut()
                .definitions
                .push((name.to_string(), span, kind));
        }
    }

    /// Record a reference to a name, if context collection is active.
    fn record_reference(&self, name: &str, span: Span, kind: SymbolKind) {
        if let Some(ctx) = &self.context {
            ctx.borrow_mut()
                .references
                .push((span, name.to_string(), kind));
        }
    }
}

/// Collected type information from a type-checking pass.
///
/// Used by the LSP for hover (type_map) and go-to-definition (definitions/references).
pub struct InferContext {
    /// Every inferred expression: span -> type.
    pub type_map: Vec<(Span, Type)>,
    /// Definition sites: (name, span, kind). Recorded in program order.
    /// For scoped definitions (lambda params, let bindings), later entries shadow earlier ones.
    pub definitions: Vec<(String, Span, SymbolKind)>,
    /// Reference sites: (use_span, name, kind). Recorded when a variable/constructor/effect-op is resolved.
    pub references: Vec<(Span, String, SymbolKind)>,
    /// Cross-file definitions: maps an imported name (possibly qualified like "L/map")
    /// to (original_name, file_path, span_in_that_file, kind).
    /// Populated during import processing for go-to-definition across files.
    pub cross_file_definitions: Vec<(String, String, PathBuf, Span, SymbolKind)>,
    /// Per-module definition cache: maps a resolved module path to the definitions
    /// recorded during its type-checking. Used to serve cross-file definitions
    /// when a module is loaded from the TypeModuleCache on subsequent imports.
    pub module_definitions: HashMap<PathBuf, Vec<(String, Span, SymbolKind)>>,
    /// Number of definitions that came from the prelude. These definitions have
    /// spans that are byte offsets within `prelude.mar`, not the user's file.
    /// Set by the LSP after type-checking the prelude portion of the expression list.
    pub prelude_definition_count: usize,
    /// Path to the prelude file on disk. Used by `find_definitions` to route
    /// prelude definitions to the correct file for cross-file navigation.
    pub prelude_path: Option<PathBuf>,
}

impl InferContext {
    pub fn new() -> Self {
        Self {
            type_map: Vec::new(),
            definitions: Vec::new(),
            references: Vec::new(),
            cross_file_definitions: Vec::new(),
            module_definitions: HashMap::new(),
            prelude_definition_count: 0,
            prelude_path: None,
        }
    }

    /// Apply a substitution to all recorded types, resolving type variables.
    ///
    /// Called at the end of type inference to replace unresolved type variables
    /// (e.g. `t0`, `t1`) with their final concrete types (e.g. `Int`, `String`).
    pub fn apply_substitution(&mut self, subst: &Substitution) {
        for entry in &mut self.type_map {
            entry.1 = apply_subst(subst, &entry.1);
        }
    }
}

impl Default for InferContext {
    fn default() -> Self {
        Self::new()
    }
}

// ===== Hygiene mark stripping =====

/// Strip all hygiene marks (~N suffixes) from a name for fallback lookups.
fn strip_marks(name: &str) -> &str {
    match name.find('~') {
        Some(pos) => {
            let rest = &name[pos + 1..];
            if !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit() || b == b'~') {
                &name[..pos]
            } else {
                name
            }
        }
        None => name,
    }
}

// ===== Inference Result =====

pub struct InferResult {
    pub ty: Type,
    pub subst: Substitution,
    pub effects: EffectRow,
}

fn pure_result(ty: Type) -> InferResult {
    InferResult {
        ty,
        subst: empty_subst(),
        effects: effect_row_empty(),
    }
}

// ===== Branch Unification =====

/// Unify a new branch type with the accumulated result type.
///
/// On the first branch (`result_type` is `None`), simply stores the type.
/// On subsequent branches, unifies with the existing type and composes the
/// resulting substitution into `subst`.
fn unify_branch(
    result_type: &mut Option<Type>,
    subst: &mut Substitution,
    branch_type: Type,
    context: &str,
    span: Option<Span>,
) -> Result<(), TypeError> {
    match result_type {
        None => *result_type = Some(branch_type),
        Some(rt) => {
            let u = unify_with_context(rt, &branch_type, context).map_err(|e| {
                if let Some(s) = span {
                    e.with_span_if_missing(s)
                } else {
                    e
                }
            })?;
            *subst = compose_subst(&u, subst);
            *result_type = Some(apply_subst(subst, rt));
        }
    }
    Ok(())
}

// ===== AST Helpers =====

fn is_symbol(expr: &Expr, interner: &Interner, name: &str) -> bool {
    ast::get_symbol_name(expr, interner) == Some(name)
}

/// Get the symbol name of the first element of a list expression.
fn get_first_symbol<'a>(expr: &'a Expr, interner: &'a Interner) -> Option<&'a str> {
    if let Expr::List { .. } = expr {
        let elements = ast::list_to_array(expr);
        if !elements.is_empty() {
            return ast::get_symbol_name(elements[0], interner);
        }
    }
    None
}

/// Check if an expression is a `(define name value)` form and extract the name.
fn is_define_form<'a>(expr: &'a Expr, interner: &'a Interner) -> Option<&'a str> {
    if let Expr::List { .. } = expr {
        let elements = ast::list_to_array(expr);
        if elements.len() == 3 && is_symbol(elements[0], interner, "define") {
            // Simple define: (define name value)
            if let Some(name) = ast::get_symbol_name(elements[1], interner) {
                return Some(name);
            }
            // Annotated define: (define (name : Type) value)
            if let Expr::List { .. } = elements[1] {
                let name_elements = ast::list_to_array(elements[1]);
                if name_elements.len() == 3 && is_symbol(name_elements[1], interner, ":") {
                    return ast::get_symbol_name(name_elements[0], interner);
                }
            }
        }
    }
    None
}

/// Extract the annotation type from a `(define (name : Type) value)` form, if present.
fn get_define_annotation(expr: &Expr, env: &TypeEnv, interner: &Interner) -> Option<Type> {
    if let Expr::List { .. } = expr {
        let elements = ast::list_to_array(expr);
        if elements.len() == 3
            && is_symbol(elements[0], interner, "define")
            && let Expr::List { .. } = elements[1]
        {
            let name_elements = ast::list_to_array(elements[1]);
            if name_elements.len() == 3 && is_symbol(name_elements[1], interner, ":") {
                return parse_type_annotation(name_elements[2], env, interner).ok();
            }
        }
    }
    None
}

/// Infer a body sequence with letrec semantics for leading defines.
/// Leading define forms get placeholder type variables inserted into the environment
/// before their bodies are inferred, allowing mutual recursion.
/// Collect all symbol names referenced in an expression.
/// This is used for dependency analysis of local defines.
fn collect_free_symbols(expr: &Expr, interner: &Interner, out: &mut HashSet<String>) {
    match expr {
        Expr::Atom(LispValue::Symbol(sym), _) => {
            out.insert(interner.resolve(*sym).to_string());
        }
        Expr::List { head, tail, .. } => {
            collect_free_symbols(head, interner, out);
            collect_free_symbols(tail, interner, out);
        }
        _ => {}
    }
}

/// Compute strongly connected components using Tarjan's algorithm.
/// Returns SCCs in reverse topological order (dependencies come first when reversed).
fn tarjan_scc(num_nodes: usize, adj: &[Vec<usize>]) -> Vec<Vec<usize>> {
    struct State {
        index_counter: usize,
        stack: Vec<usize>,
        on_stack: Vec<bool>,
        index: Vec<Option<usize>>,
        lowlink: Vec<usize>,
        result: Vec<Vec<usize>>,
    }

    fn strongconnect(v: usize, adj: &[Vec<usize>], state: &mut State) {
        state.index[v] = Some(state.index_counter);
        state.lowlink[v] = state.index_counter;
        state.index_counter += 1;
        state.stack.push(v);
        state.on_stack[v] = true;

        for &w in &adj[v] {
            if state.index[w].is_none() {
                strongconnect(w, adj, state);
                state.lowlink[v] = state.lowlink[v].min(state.lowlink[w]);
            } else if state.on_stack[w] {
                state.lowlink[v] = state.lowlink[v].min(state.index[w].unwrap());
            }
        }

        if state.lowlink[v] == state.index[v].unwrap() {
            let mut component = Vec::new();
            loop {
                let w = state.stack.pop().unwrap();
                state.on_stack[w] = false;
                component.push(w);
                if w == v {
                    break;
                }
            }
            state.result.push(component);
        }
    }

    let mut state = State {
        index_counter: 0,
        stack: Vec::new(),
        on_stack: vec![false; num_nodes],
        index: vec![None; num_nodes],
        lowlink: vec![0; num_nodes],
        result: Vec::new(),
    };

    for v in 0..num_nodes {
        if state.index[v].is_none() {
            strongconnect(v, adj, &mut state);
        }
    }

    // Tarjan's produces SCCs in reverse topological order of the condensation
    // DAG, which means sinks (nodes with no outgoing edges / no dependencies)
    // come first. This is exactly the order we need: process dependencies before
    // their dependents.
    state.result
}

fn infer_body_with_defines(
    env: &TypeEnv,
    body_exprs: &[&Expr],
    session: &InferSession,
) -> Result<(Substitution, Type, EffectRow), TypeError> {
    if body_exprs.is_empty() {
        return Ok((empty_subst(), t_unit(), effect_row_empty()));
    }

    // Count leading defines
    let mut num_defines = 0;
    for expr in body_exprs {
        if is_define_form(expr, session.interner).is_some() {
            num_defines += 1;
        } else {
            break;
        }
    }

    if num_defines == 0 {
        // No leading defines — fall through to normal sequential inference
        return infer_body_sequence(env, body_exprs, session);
    }

    let define_exprs = &body_exprs[..num_defines];
    let rest_exprs = &body_exprs[num_defines..];

    // Collect define names and their value expressions
    let mut define_names: Vec<String> = Vec::new();
    let mut define_value_exprs: Vec<&Expr> = Vec::new();
    let mut define_full_exprs: Vec<&Expr> = Vec::new();

    for &def_expr in define_exprs {
        let name = is_define_form(def_expr, session.interner)
            .unwrap()
            .to_string();
        let elements = ast::list_to_array(def_expr);
        define_names.push(name);
        define_value_exprs.push(elements[2]);
        define_full_exprs.push(def_expr);
    }

    // Build name-to-index map
    let name_to_idx: HashMap<&str, usize> = define_names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();

    // Build dependency graph: edge from i to j means define i references define j
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); num_defines];
    for (i, &val_expr) in define_value_exprs.iter().enumerate() {
        let mut refs = HashSet::new();
        collect_free_symbols(val_expr, session.interner, &mut refs);
        for ref_name in &refs {
            if let Some(&j) = name_to_idx.get(ref_name.as_str())
                && !adj[i].contains(&j)
            {
                adj[i].push(j);
            }
        }
    }

    // Compute SCCs in topological order (dependencies first)
    let sccs = tarjan_scc(num_defines, &adj);

    // Process each SCC: create placeholders, infer, generalize
    let mut current_env = env.clone();
    let mut subst = empty_subst();
    let mut all_effects = effect_row_empty();

    for scc in &sccs {
        // Enter level for this SCC group
        enter_level();

        // Create placeholders for all names in this SCC
        let mut scc_placeholders: Vec<(String, Type)> = Vec::new();
        let mut scc_env = current_env.clone();

        for &idx in scc {
            let name = &define_names[idx];
            let ann_type =
                get_define_annotation(define_full_exprs[idx], &scc_env, session.interner);
            let placeholder = ann_type.unwrap_or_else(fresh_type_var);
            scc_env
                .bindings_mut()
                .insert(name.clone(), monotype(placeholder.clone()));
            scc_placeholders.push((name.clone(), placeholder));
        }

        // Infer each define's value in the extended environment
        for (scc_i, &idx) in scc.iter().enumerate() {
            let applied_env = apply_subst_env(&subst, &scc_env);
            let result = infer_expr(&applied_env, define_value_exprs[idx], session)?;
            subst = compose_subst(&result.subst, &subst);
            all_effects = combine_effect_rows(&all_effects, &result.effects);

            // Unify inferred type with placeholder
            let (ref name, ref placeholder) = scc_placeholders[scc_i];
            let placeholder_applied = apply_subst(&subst, placeholder);
            let value_type_applied = apply_subst(&subst, &result.ty);
            let unify_result = unify_with_context(
                &placeholder_applied,
                &value_type_applied,
                &format!("local definition of {name}"),
            )?;
            subst = compose_subst(&unify_result, &subst);
        }

        // Exit level before generalizing
        exit_level();

        // Generalize and add to current_env for subsequent SCCs
        current_env = apply_subst_env(&subst, &current_env);
        for (name, placeholder) in &scc_placeholders {
            let ty = apply_subst(&subst, placeholder);
            let scheme = generalize(&current_env, &ty);
            current_env.bindings_mut().insert(name.clone(), scheme);
        }
    }

    // Infer remaining (non-define) body expressions
    if rest_exprs.is_empty() {
        return Ok((subst, t_unit(), all_effects));
    }

    let (subst, all_effects, types) =
        infer_exprs(&current_env, rest_exprs, session, subst, all_effects)?;
    let body_type = types.into_iter().last().unwrap_or_else(t_unit);
    Ok((subst, body_type, all_effects))
}

/// Infer a body sequence without special define handling (the original logic).
fn infer_body_sequence(
    env: &TypeEnv,
    exprs: &[&Expr],
    session: &InferSession,
) -> Result<(Substitution, Type, EffectRow), TypeError> {
    let (subst, effects, types) =
        infer_exprs(env, exprs, session, empty_subst(), effect_row_empty())?;
    let last_type = types.into_iter().last().unwrap_or_else(t_unit);
    Ok((subst, last_type, effects))
}

// ===== Shared Inference Helpers =====

/// Infer a sequence of expressions, threading substitutions and collecting effects.
///
/// This is the core loop that appears throughout inference: for each expression,
/// apply the current substitution to the environment, infer, compose substitutions,
/// and combine effect rows.
///
/// Returns `(final_subst, combined_effects, inferred_types)`.
fn infer_exprs(
    env: &TypeEnv,
    exprs: &[&Expr],
    session: &InferSession,
    mut subst: Substitution,
    mut effects: EffectRow,
) -> Result<(Substitution, EffectRow, Vec<Type>), TypeError> {
    let mut types = Vec::with_capacity(exprs.len());
    for expr in exprs {
        let env_applied = apply_subst_env(&subst, env);
        let result = infer_expr(&env_applied, expr, session)?;
        subst = compose_subst(&result.subst, &subst);
        effects = combine_effect_rows(&effects, &result.effects);
        types.push(result.ty);
    }
    Ok((subst, effects, types))
}

// ===== Variadic Arithmetic =====

/// Returns the minimum argument count for a variadic arithmetic operator,
/// or `None` if the name is not a variadic arithmetic op.
fn variadic_arith_min_args(op: &str) -> Option<usize> {
    match op {
        "+" | "*" => Some(0),
        "-" => Some(1),
        "/" => Some(2),
        _ => None,
    }
}

const FFI_VARIADIC_FUNCTIONS: &[&str] = &["ffi-call", "struct-new", "struct-get", "struct-set"];

// ===== Effect Row Parsing =====

/// Check if an expression is an `__effect_row` form (produced by the parser for `{...}`).
fn is_effect_row_expr(expr: &Expr, interner: &Interner) -> bool {
    if let Expr::List { .. } = expr {
        let elements = ast::list_to_array(expr);
        if !elements.is_empty()
            && let Some("__effect_row") = ast::get_symbol_name(elements[0], interner)
        {
            return true;
        }
    }
    false
}

/// Parse an `__effect_row` expression into an EffectRow.
/// `(__effect_row)` → Closed([])
/// `(__effect_row IO Exception)` → Closed(["IO", "Exception"])
/// `(__effect_row IO __row_var:e)` → Open { effects: ["IO"], row_var: "e" }
/// `(__effect_row __row_var:e)` → Open { effects: [], row_var: "e" }
fn parse_effect_row_expr(expr: &Expr, interner: &Interner) -> Result<EffectRow, TypeError> {
    let elements = ast::list_to_array(expr);
    // elements[0] is "__effect_row"
    let mut effects = Vec::new();
    let mut row_var: Option<String> = None;

    for e in &elements[1..] {
        if let Some(name) = ast::get_symbol_name(e, interner) {
            if let Some(stripped) = name.strip_prefix("__row_var:") {
                row_var = Some(stripped.to_string());
            } else {
                effects.push(name.to_string());
            }
        } else {
            return Err(TypeError::InvalidTypeAnnotation {
                message: "Effect row elements must be symbols".into(),
                span: Some(e.span()),
            });
        }
    }

    match row_var {
        Some(rv) => Ok(effect_row_open(effects, rv)),
        None => Ok(effect_row_closed(effects)),
    }
}

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
                "Char" => Ok(t_char()),
                "Unit" => Ok(t_unit()),
                "Bytes" => Ok(t_bytes()),
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
            span: Some(expr.span()),
        }),
        Expr::List { .. } => {
            let elements = ast::list_to_array(expr);
            if elements.is_empty() {
                return Ok(t_unit());
            }

            let first_sym = ast::get_symbol_name(elements[0], interner);

            // Arrow type: (-> a b c) or (-> a (... b) c) or (-> a {IO} b) for variadic/effectful
            if first_sym == Some("->") {
                if elements.len() < 2 {
                    return Err(TypeError::InvalidTypeAnnotation {
                        message: "Arrow type needs at least a result type".into(),
                        span: Some(expr.span()),
                    });
                }
                let mut types = Vec::new();
                let mut rest_type: Option<Type> = None;
                let mut effect_row: Option<EffectRow> = None;
                for e in &elements[1..] {
                    // Check for {effects} effect row syntax
                    if is_effect_row_expr(e, interner) {
                        if effect_row.is_some() {
                            return Err(TypeError::InvalidTypeAnnotation {
                                message: "Arrow type can have at most one effect row annotation"
                                    .into(),
                                span: Some(expr.span()),
                            });
                        }
                        effect_row = Some(parse_effect_row_expr(e, interner)?);
                        continue;
                    }
                    // Check for (... T) rest type syntax
                    if let Expr::List { .. } = e {
                        let sub_elems = ast::list_to_array(e);
                        if sub_elems.len() == 2
                            && let Some("...") = ast::get_symbol_name(sub_elems[0], interner)
                        {
                            if rest_type.is_some() {
                                return Err(TypeError::InvalidTypeAnnotation {
                                    message: "Arrow type can have at most one (... T) rest type"
                                        .into(),
                                    span: Some(e.span()),
                                });
                            }
                            rest_type = Some(parse_type_expr(sub_elems[1], env, interner)?);
                            continue;
                        }
                    }
                    types.push(parse_type_expr(e, env, interner)?);
                }
                if types.is_empty() && rest_type.is_some() {
                    return Err(TypeError::InvalidTypeAnnotation {
                        message: "Arrow type needs at least a result type".into(),
                        span: Some(expr.span()),
                    });
                }
                if types.len() < 2 && rest_type.is_none() {
                    return Err(TypeError::InvalidTypeAnnotation {
                        message: "Arrow type needs at least a parameter type and a result type, e.g. (-> Unit Unit)".into(),
                        span: Some(expr.span()),
                    });
                }
                let result = types.pop().unwrap();
                if let Some(rest_ty) = rest_type {
                    return Ok(type_arrow_variadic_with_effects(
                        types, rest_ty, effect_row, result,
                    ));
                }
                return Ok(type_arrow_with_effects(types, effect_row, result));
            }

            // Constrained type: (=> constraints type)
            if first_sym == Some("=>") {
                if elements.len() < 2 {
                    return Err(TypeError::InvalidTypeAnnotation {
                        message: "Constrained type needs a body type".into(),
                        span: Some(expr.span()),
                    });
                }
                return parse_type_expr(elements[elements.len() - 1], env, interner);
            }

            // Recursive type: (mu a T)
            if first_sym == Some("mu") {
                if elements.len() != 3 {
                    return Err(TypeError::InvalidTypeAnnotation {
                        message: "mu type needs: (mu var body)".into(),
                        span: Some(expr.span()),
                    });
                }
                let var_name = ast::get_symbol_name(elements[1], interner).ok_or(
                    TypeError::InvalidTypeAnnotation {
                        message: "mu variable must be a symbol".into(),
                        span: Some(elements[1].span()),
                    },
                )?;
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

fn parse_type_annotation(
    expr: &Expr,
    env: &TypeEnv,
    interner: &Interner,
) -> Result<Type, TypeError> {
    if let Expr::List { .. } = expr {
        let elements = ast::list_to_array(expr);
        if !elements.is_empty()
            && let Some("=>") = ast::get_symbol_name(elements[0], interner)
        {
            return Err(TypeError::InvalidTypeAnnotation {
                message: "'=> (constrained types)' declarations are not supported".into(),
                span: Some(expr.span()),
            });
        }
    }
    parse_type_expr(expr, env, interner)
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
            if let Some((_, ctor)) = lookup_constructor(env, name)
                && ctor.params.is_empty()
            {
                return Ok(Pattern::Constructor {
                    name: name.to_string(),
                    args: vec![],
                });
            }
            Ok(Pattern::Var(name.to_string()))
        }
        Expr::Atom(LispValue::Number(v, _), _) => Ok(Pattern::Literal(LiteralPattern::Number(*v))),
        Expr::Atom(LispValue::String(s), _) => {
            Ok(Pattern::Literal(LiteralPattern::String(s.to_string())))
        }
        Expr::List { .. } => {
            let elements = ast::list_to_array(expr);
            if elements.is_empty() {
                return Err(TypeError::InvalidTypeAnnotation {
                    message: "Empty pattern".into(),
                    span: Some(expr.span()),
                });
            }
            let ctor_name = ast::get_symbol_name(elements[0], interner).ok_or(
                TypeError::InvalidTypeAnnotation {
                    message: "Pattern constructor must be a symbol".into(),
                    span: Some(elements[0].span()),
                },
            )?;
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
    session: &InferSession,
) -> Result<InferResult, TypeError> {
    let span = expr.span();
    let result = infer_expr_inner(env, expr, session)?;
    session.record_type(span, &result.ty);
    Ok(result)
}

fn infer_expr_inner(
    env: &TypeEnv,
    expr: &Expr,
    session: &InferSession,
) -> Result<InferResult, TypeError> {
    match expr {
        Expr::Nil(_) => Ok(pure_result(t_unit())),
        Expr::Atom(value, _) => infer_atom(env, value, expr.span(), session),
        Expr::List { .. } => {
            let elements = ast::list_to_array(expr);
            if elements.is_empty() {
                return Ok(pure_result(t_unit()));
            }

            let first_sym = ast::get_symbol_name(elements[0], session.interner);

            if let Some(sym) = first_sym {
                match sym {
                    "define" => return infer_define(env, &elements, session),
                    "lambda" => return infer_lambda(env, &elements, session),
                    "if" => return infer_if(env, &elements, session),
                    "let" => return infer_let(env, &elements, session),
                    "begin" => return infer_begin(env, &elements, session),
                    "cond" => return infer_cond(env, &elements, session),
                    "match" => return infer_match(env, &elements, session),
                    "data" => return infer_data_def(env, &elements, session.interner),
                    "class" | "instance" => {
                        return Err(TypeError::InvalidTypeAnnotation {
                            message: format!("'{}' declarations are not supported", sym),
                            span: Some(elements[0].span()),
                        });
                    }
                    "type" | "foreign" | "foreign-lib" | "defmacro" | "declare" => {
                        return Ok(pure_result(t_unit()));
                    }
                    "quote" | "quasiquote" => {
                        let tv = fresh_type_var();
                        return Ok(pure_result(tv));
                    }
                    "effect" => return infer_effect_def(env, &elements, session.interner),
                    "perform" => return infer_perform(env, &elements, session),
                    "handle" => return infer_handle(env, &elements, session),
                    "resume" => return infer_resume(env, &elements, session),
                    "import" => return Ok(pure_result(t_unit())),
                    _ => {}
                }
            }

            infer_application(env, &elements, session)
        }
    }
}

fn infer_atom(
    env: &TypeEnv,
    value: &LispValue,
    span: Span,
    session: &InferSession,
) -> Result<InferResult, TypeError> {
    match value {
        LispValue::Number(_, is_float) => {
            let ty = if *is_float { t_float() } else { t_int() };
            Ok(pure_result(ty))
        }
        LispValue::String(_) => Ok(pure_result(t_string())),
        LispValue::Symbol(sym) => {
            let name = session.interner.resolve(*sym);
            // Boolean literals
            if name == "#t" || name == "#f" {
                return Ok(pure_result(t_bool()));
            }
            let scheme = lookup_var(env, name)
                .or_else(|| {
                    // Fallback: strip hygiene marks from macro expansion
                    let stripped = strip_marks(name);
                    if stripped != name {
                        lookup_var(env, stripped)
                    } else {
                        None
                    }
                })
                .ok_or(TypeError::UndefinedVariable {
                    name: strip_marks(name).to_string(),
                    span: Some(span),
                })?;
            let ty = instantiate(&scheme);
            session.record_reference(strip_marks(name), span, SymbolKind::Variable);
            Ok(pure_result(ty))
        }
    }
}

// ===== Define =====

fn infer_define(
    env: &TypeEnv,
    elements: &[&Expr],
    session: &InferSession,
) -> Result<InferResult, TypeError> {
    if elements.len() < 3 {
        return Err(TypeError::ArityMismatch {
            expected: 2,
            got: elements.len() - 1,
            context: "define".into(),
            span: Some(elements[0].span()),
        });
    }

    let name_expr = elements[1];
    let value_expr = elements[2];

    // Check for type annotation: (define (name : Type) value)
    if let Expr::List { .. } = name_expr {
        let name_elements = ast::list_to_array(name_expr);
        if name_elements.len() == 3 && is_symbol(name_elements[1], session.interner, ":") {
            let name = ast::get_symbol_name(name_elements[0], session.interner).ok_or(
                TypeError::InvalidTypeAnnotation {
                    message: "Expected symbol before :".into(),
                    span: Some(name_elements[0].span()),
                },
            )?;

            let annotated_type = parse_type_annotation(name_elements[2], env, session.interner)?;
            let value_result = infer_expr(env, value_expr, session)?;
            let unify_subst = unify_with_context(
                &annotated_type,
                &value_result.ty,
                &format!("definition of {name}"),
            )?;
            let final_subst = compose_subst(&unify_subst, &value_result.subst);

            return Ok(InferResult {
                ty: t_unit(),
                subst: final_subst,
                effects: value_result.effects,
            });
        }
    }

    // Regular define — validate that the name is a symbol
    let _name = ast::get_symbol_name(name_expr, session.interner).ok_or(
        TypeError::InvalidTypeAnnotation {
            message: "define expects a symbol name".into(),
            span: Some(elements[1].span()),
        },
    )?;

    let result = infer_expr(env, value_expr, session)?;
    Ok(InferResult {
        ty: t_unit(),
        subst: result.subst,
        effects: result.effects,
    })
}

// ===== Lambda =====

fn infer_lambda(
    env: &TypeEnv,
    elements: &[&Expr],
    session: &InferSession,
) -> Result<InferResult, TypeError> {
    if elements.len() < 3 {
        return Err(TypeError::ArityMismatch {
            expected: 2,
            got: elements.len() - 1,
            context: "lambda".into(),
            span: Some(elements[0].span()),
        });
    }

    let params_expr = elements[1];
    let body_exprs = &elements[2..];

    let param_list = ast::list_to_array(params_expr);
    let mut params: Vec<(String, Type)> = Vec::new();
    let mut rest_param: Option<(String, Type)> = None; // (name, element_type)
    let mut param_env = child_env(env);

    // Detect dot notation: (a b . rest) => param_list = [a, b, ., rest]
    let dot_pos = param_list
        .iter()
        .position(|p| ast::get_symbol_name(p, session.interner) == Some("."));

    let (required_params, rest_param_expr) = if let Some(pos) = dot_pos {
        if pos + 1 >= param_list.len() {
            return Err(TypeError::InvalidTypeAnnotation {
                message: "rest parameter name expected after '.'".into(),
                span: Some(elements[1].span()),
            });
        }
        if pos + 2 != param_list.len() {
            return Err(TypeError::InvalidTypeAnnotation {
                message: "only one rest parameter name allowed after '.'".into(),
                span: Some(elements[1].span()),
            });
        }
        (&param_list[..pos], Some(param_list[pos + 1]))
    } else {
        (&param_list[..], None)
    };

    for param in required_params {
        // Check for annotated param: (x : Int)
        if let Expr::List { .. } = param {
            let param_elements = ast::list_to_array(param);
            if param_elements.len() == 3 && is_symbol(param_elements[1], session.interner, ":") {
                let name = ast::get_symbol_name(param_elements[0], session.interner).ok_or(
                    TypeError::InvalidTypeAnnotation {
                        message: "Parameter name must be a symbol".into(),
                        span: Some(param_elements[0].span()),
                    },
                )?;
                let ty = parse_type_expr(param_elements[2], env, session.interner)?;
                params.push((name.to_string(), ty.clone()));
                param_env
                    .bindings_mut()
                    .insert(name.to_string(), monotype(ty));
                session.record_definition(name, param_elements[0].span(), SymbolKind::Variable);
                continue;
            }
        }

        let name = ast::get_symbol_name(param, session.interner).ok_or(
            TypeError::InvalidTypeAnnotation {
                message: "Parameter must be a symbol".into(),
                span: Some(param.span()),
            },
        )?;
        let param_type = fresh_type_var();
        params.push((name.to_string(), param_type.clone()));
        param_env
            .bindings_mut()
            .insert(name.to_string(), monotype(param_type));
        session.record_definition(name, param.span(), SymbolKind::Variable);
    }

    // Handle rest parameter
    if let Some(rest_expr) = rest_param_expr {
        // Check for annotated rest param: (rest : Type) — Type is the element type
        let (rest_name, elem_type) = if let Expr::List { .. } = rest_expr {
            let param_elements = ast::list_to_array(rest_expr);
            if param_elements.len() == 3 && is_symbol(param_elements[1], session.interner, ":") {
                let name = ast::get_symbol_name(param_elements[0], session.interner).ok_or(
                    TypeError::InvalidTypeAnnotation {
                        message: "Rest parameter name must be a symbol".into(),
                        span: Some(param_elements[0].span()),
                    },
                )?;
                let ty = parse_type_expr(param_elements[2], env, session.interner)?;
                (name, ty)
            } else {
                return Err(TypeError::InvalidTypeAnnotation {
                    message: "Rest parameter must be a symbol or (name : Type)".into(),
                    span: Some(rest_expr.span()),
                });
            }
        } else {
            let name = ast::get_symbol_name(rest_expr, session.interner).ok_or(
                TypeError::InvalidTypeAnnotation {
                    message: "Rest parameter must be a symbol".into(),
                    span: Some(rest_expr.span()),
                },
            )?;
            (name, fresh_type_var())
        };

        // The rest parameter has type (List elem_type) in the function body
        let list_type = t_list(elem_type.clone());
        param_env
            .bindings_mut()
            .insert(rest_name.to_string(), monotype(list_type));
        rest_param = Some((rest_name.to_string(), elem_type));
    }

    // Infer body with letrec semantics for leading defines.
    // Determine which body expressions to process (unwrap a single begin if present).
    let effective_body: Vec<&Expr> = if body_exprs.len() == 1 {
        if let Some("begin") = get_first_symbol(body_exprs[0], session.interner) {
            let begin_elements = ast::list_to_array(body_exprs[0]);
            begin_elements[1..].to_vec()
        } else {
            body_exprs.to_vec()
        }
    } else {
        body_exprs.to_vec()
    };

    let (body_subst, body_type, body_effects) =
        infer_body_with_defines(&param_env, &effective_body, session)?;

    let mut param_types: Vec<Type> = params
        .iter()
        .map(|(_, t)| apply_subst(&body_subst, t))
        .collect();
    let rest_type = rest_param.map(|(_, t)| apply_subst(&body_subst, &t));
    let result_type = apply_subst(&body_subst, &body_type);
    let fn_effects = Some(body_effects);
    // Zero-param lambdas get an implicit Unit param: (lambda () body) => (-> Unit body_type)
    if param_types.is_empty() && rest_type.is_none() {
        param_types.push(t_unit());
    }
    let fn_type = if let Some(rest_ty) = rest_type {
        type_arrow_variadic_with_effects(param_types, rest_ty, fn_effects, result_type)
    } else {
        type_arrow_with_effects(param_types, fn_effects, result_type)
    };

    Ok(InferResult {
        ty: fn_type,
        subst: body_subst,
        effects: effect_row_empty(),
    })
}

// ===== If =====

fn infer_if(
    env: &TypeEnv,
    elements: &[&Expr],
    session: &InferSession,
) -> Result<InferResult, TypeError> {
    if elements.len() < 3 || elements.len() > 4 {
        return Err(TypeError::ArityMismatch {
            expected: 3,
            got: elements.len() - 1,
            context: "if".into(),
            span: Some(elements[0].span()),
        });
    }

    let if_span = elements[0].span();
    let cond_result = infer_expr(env, elements[1], session)?;
    let cond_unify = unify_with_context(&cond_result.ty, &t_bool(), "if condition")
        .map_err(|e| e.with_span_if_missing(if_span))?;
    let s1 = compose_subst(&cond_unify, &cond_result.subst);
    let env1 = apply_subst_env(&s1, env);

    let then_result = infer_expr(&env1, elements[2], session)?;
    let s2 = compose_subst(&then_result.subst, &s1);

    if elements.len() == 4 {
        let env2 = apply_subst_env(&s2, env);
        let else_result = infer_expr(&env2, elements[3], session)?;
        let s3 = compose_subst(&else_result.subst, &s2);

        let then_type = apply_subst(&s3, &then_result.ty);
        let else_type = apply_subst(&s3, &else_result.ty);

        let branch_unify = unify_with_context(&then_type, &else_type, "if branches")
            .map_err(|e| e.with_span_if_missing(if_span))?;
        let final_subst = compose_subst(&branch_unify, &s3);
        let all_effects = combine_effect_rows(
            &combine_effect_rows(&cond_result.effects, &then_result.effects),
            &else_result.effects,
        );

        Ok(InferResult {
            ty: apply_subst(&final_subst, &then_type),
            subst: final_subst,
            effects: all_effects,
        })
    } else {
        let all_effects = combine_effect_rows(&cond_result.effects, &then_result.effects);
        Ok(InferResult {
            ty: t_unit(),
            subst: s2,
            effects: all_effects,
        })
    }
}

// ===== Let =====

fn infer_let(
    env: &TypeEnv,
    elements: &[&Expr],
    session: &InferSession,
) -> Result<InferResult, TypeError> {
    if elements.len() < 3 {
        return Err(TypeError::ArityMismatch {
            expected: 2,
            got: elements.len() - 1,
            context: "let".into(),
            span: Some(elements[0].span()),
        });
    }

    let bindings_expr = elements[1];
    let body_exprs = &elements[2..];

    let bindings = ast::list_to_array(bindings_expr);
    let mut let_env = child_env(env);
    let mut subst = empty_subst();
    let mut all_effects = effect_row_empty();

    for binding in &bindings {
        let pair = ast::list_to_array(binding);
        if pair.len() != 2 {
            return Err(TypeError::InvalidTypeAnnotation {
                message: "let binding must be (name value)".into(),
                span: Some(binding.span()),
            });
        }

        let name_expr = pair[0];
        let value_expr = pair[1];

        let name: String;
        let mut annotated_type: Option<Type> = None;

        if let Expr::List { .. } = name_expr {
            let name_elements = ast::list_to_array(name_expr);
            if name_elements.len() == 3 && is_symbol(name_elements[1], session.interner, ":") {
                let n = ast::get_symbol_name(name_elements[0], session.interner).ok_or(
                    TypeError::InvalidTypeAnnotation {
                        message: "Expected symbol before :".into(),
                        span: Some(name_elements[0].span()),
                    },
                )?;
                name = n.to_string();
                annotated_type = Some(parse_type_expr(name_elements[2], env, session.interner)?);
                session.record_definition(&name, name_elements[0].span(), SymbolKind::Variable);
            } else {
                return Err(TypeError::InvalidTypeAnnotation {
                    message: "Invalid let binding name".into(),
                    span: Some(name_expr.span()),
                });
            }
        } else {
            let n = ast::get_symbol_name(name_expr, session.interner).ok_or(
                TypeError::InvalidTypeAnnotation {
                    message: "let binding name must be a symbol".into(),
                    span: Some(name_expr.span()),
                },
            )?;
            name = n.to_string();
            session.record_definition(&name, name_expr.span(), SymbolKind::Variable);
        }

        // Enter level for this binding's inference
        enter_level();

        let env_for_value = apply_subst_env(&subst, &let_env);
        let value_result = infer_expr(&env_for_value, value_expr, session)?;

        subst = compose_subst(&value_result.subst, &subst);
        all_effects = combine_effect_rows(&all_effects, &value_result.effects);

        let mut binding_type = value_result.ty;

        if let Some(ann_type) = annotated_type {
            let binding_span = name_expr.span();
            let unify_result =
                unify_with_context(&ann_type, &binding_type, &format!("let binding {name}"))
                    .map_err(|e| e.with_span_if_missing(binding_span))?;
            subst = compose_subst(&unify_result, &subst);
            binding_type = apply_subst(&subst, &ann_type);
        }

        // Exit level before generalizing
        exit_level();

        let env_for_gen = apply_subst_env(&subst, &let_env);
        let scheme = generalize(&env_for_gen, &apply_subst(&subst, &binding_type));
        let_env.bindings_mut().insert(name.to_string(), scheme);
    }

    // Infer body
    let (subst, all_effects, types) =
        infer_exprs(&let_env, body_exprs, session, subst, all_effects)?;
    let body_type = types.into_iter().last().unwrap_or_else(t_unit);

    Ok(InferResult {
        ty: apply_subst(&subst, &body_type),
        subst,
        effects: all_effects,
    })
}

// ===== Begin =====

fn infer_begin(
    env: &TypeEnv,
    elements: &[&Expr],
    session: &InferSession,
) -> Result<InferResult, TypeError> {
    let body_exprs: Vec<&Expr> = elements[1..].to_vec();

    if body_exprs.is_empty() {
        return Ok(pure_result(t_unit()));
    }

    let (subst, last_type, all_effects) = infer_body_with_defines(env, &body_exprs, session)?;

    Ok(InferResult {
        ty: apply_subst(&subst, &last_type),
        subst,
        effects: all_effects,
    })
}

// ===== Cond =====

fn infer_cond(
    env: &TypeEnv,
    elements: &[&Expr],
    session: &InferSession,
) -> Result<InferResult, TypeError> {
    let clauses = &elements[1..];
    if clauses.is_empty() {
        return Ok(pure_result(t_unit()));
    }

    let mut subst = empty_subst();
    let mut result_type: Option<Type> = None;
    let mut all_effects = effect_row_empty();

    for clause_expr in clauses {
        let clause = ast::list_to_array(clause_expr);
        if clause.len() != 2 {
            return Err(TypeError::InvalidTypeAnnotation {
                message: "cond clause must be (test result)".into(),
                span: Some(clause_expr.span()),
            });
        }

        let test_expr = clause[0];
        let result_expr = clause[1];

        if !is_symbol(test_expr, session.interner, "else") {
            let test_env = apply_subst_env(&subst, env);
            let test_result = infer_expr(&test_env, test_expr, session)?;
            let test_unify = unify_with_context(&test_result.ty, &t_bool(), "cond test")
                .map_err(|e| e.with_span_if_missing(test_expr.span()))?;
            subst = compose_subst(&test_unify, &compose_subst(&test_result.subst, &subst));
            all_effects = combine_effect_rows(&all_effects, &test_result.effects);
        }

        let result_env = apply_subst_env(&subst, env);
        let expr_result = infer_expr(&result_env, result_expr, session)?;
        subst = compose_subst(&expr_result.subst, &subst);
        all_effects = combine_effect_rows(&all_effects, &expr_result.effects);

        let clause_type = apply_subst(&subst, &expr_result.ty);
        unify_branch(
            &mut result_type,
            &mut subst,
            clause_type,
            "cond branches",
            Some(clause_expr.span()),
        )?;
    }

    Ok(InferResult {
        ty: result_type.unwrap_or_else(t_unit),
        subst,
        effects: all_effects,
    })
}

// ===== Match =====

fn check_exhaustiveness(
    env: &TypeEnv,
    scrutinee_type: &Type,
    patterns: &[Pattern],
    span: Span,
) -> Result<(), TypeError> {
    match crate::exhaustiveness::check_exhaustiveness(env, scrutinee_type, patterns) {
        Ok(()) => Ok(()),
        Err(witnesses) => Err(TypeError::NonExhaustivePatterns {
            missing: witnesses,
            span: Some(span),
        }),
    }
}

fn infer_match(
    env: &TypeEnv,
    elements: &[&Expr],
    session: &InferSession,
) -> Result<InferResult, TypeError> {
    if elements.len() < 3 {
        return Err(TypeError::ArityMismatch {
            expected: 2,
            got: elements.len() - 1,
            context: "match".into(),
            span: Some(elements[0].span()),
        });
    }

    let scrutinee_expr = elements[1];
    let branches = &elements[2..];

    let scrutinee_result = infer_expr(env, scrutinee_expr, session)?;
    let mut subst = scrutinee_result.subst;
    let mut scrutinee_type = scrutinee_result.ty;
    let mut result_type: Option<Type> = None;
    let mut all_effects = scrutinee_result.effects;
    let mut parsed_patterns: Vec<Pattern> = Vec::new();

    for branch_expr in branches {
        let branch_elements = ast::list_to_array(branch_expr);
        if branch_elements.len() != 2 {
            return Err(TypeError::InvalidTypeAnnotation {
                message: "match branch must be (pattern body)".into(),
                span: Some(branch_expr.span()),
            });
        }

        let pattern_expr = branch_elements[0];
        let body_expr = branch_elements[1];

        let pattern = parse_pattern(pattern_expr, env, session.interner)?;
        parsed_patterns.push(pattern.clone());

        let pattern_check = infer_pattern(
            env,
            &pattern,
            &apply_subst(&subst, &scrutinee_type),
            pattern_expr.span(),
        )?;

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
            branch_env
                .bindings_mut()
                .insert(name.to_string(), monotype(apply_subst(&subst, ty)));
        }

        let body_result = infer_expr(&branch_env, body_expr, session)?;
        subst = compose_subst(&body_result.subst, &subst);
        all_effects = combine_effect_rows(&all_effects, &body_result.effects);

        let branch_type = apply_subst(&subst, &body_result.ty);
        unify_branch(
            &mut result_type,
            &mut subst,
            branch_type,
            "match branches",
            Some(body_expr.span()),
        )?;
    }

    // Check exhaustiveness
    check_exhaustiveness(
        env,
        &apply_subst(&subst, &scrutinee_type),
        &parsed_patterns,
        elements[0].span(),
    )?;

    Ok(InferResult {
        ty: result_type.unwrap_or_else(t_unit),
        subst,
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
    span: Span,
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
            let (data_type, constructor) = lookup_constructor(env, name)
                .or_else(|| {
                    let stripped = strip_marks(name);
                    if stripped != name {
                        lookup_constructor(env, stripped)
                    } else {
                        None
                    }
                })
                .ok_or(TypeError::UndefinedConstructor {
                    name: strip_marks(name).to_string(),
                    span: Some(span),
                })?;

            if args.len() != constructor.params.len() {
                return Err(TypeError::ArityMismatch {
                    expected: constructor.params.len(),
                    got: args.len(),
                    context: format!("constructor {name}"),
                    span: Some(span),
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
                    data_type
                        .type_params
                        .iter()
                        .map(|tv| type_var_map[tv].clone())
                        .collect(),
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

                let arg_result = infer_pattern(env, arg_pattern, &param_type, span)?;
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
    op_span: Span,
    session: &InferSession,
) -> Result<InferResult, TypeError> {
    let min_args =
        variadic_arith_min_args(op_name).expect("infer_variadic_arith called with non-arith op");

    if arg_exprs.len() < min_args {
        return Err(TypeError::ArityMismatch {
            expected: min_args,
            got: arg_exprs.len(),
            context: op_name.to_string(),
            span: Some(op_span),
        });
    }

    if arg_exprs.is_empty() {
        return Ok(pure_result(t_int()));
    }

    let (mut subst, all_effects, arg_types) =
        infer_exprs(env, arg_exprs, session, empty_subst(), effect_row_empty())?;

    let mut unified_type = apply_subst(&subst, &arg_types[0]);
    for (i, at) in arg_types.iter().enumerate().skip(1) {
        let arg_type = apply_subst(&subst, at);
        let unify_result = unify_with_context(
            &unified_type,
            &arg_type,
            &format!("{op_name} argument {}", i + 1),
        )?;
        subst = compose_subst(&unify_result, &subst);
        unified_type = apply_subst(&subst, &unified_type);
    }

    Ok(InferResult {
        ty: unified_type,
        subst,
        effects: all_effects,
    })
}

fn get_string_literal(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Atom(LispValue::String(s), _) => Some(s),
        _ => None,
    }
}

fn infer_ffi_application(
    env: &TypeEnv,
    fn_name: &str,
    arg_exprs: &[&Expr],
    op_span: Span,
    session: &InferSession,
) -> Result<InferResult, TypeError> {
    // ffi-call requires a foreign declaration for type checking
    if fn_name == "ffi-call" {
        if arg_exprs.len() < 2 {
            return Err(TypeError::ArityMismatch {
                expected: 2,
                got: arg_exprs.len(),
                context: "ffi-call requires at least a library and function name".into(),
                span: Some(op_span),
            });
        }

        let lib_name = ast::get_symbol_name(arg_exprs[0], session.interner).ok_or_else(|| {
            TypeError::InvalidTypeAnnotation {
                message: "ffi-call first argument must be a library variable".into(),
                span: Some(arg_exprs[0].span()),
            }
        })?;

        let foreign_fn_name =
            get_string_literal(arg_exprs[1]).ok_or_else(|| TypeError::InvalidTypeAnnotation {
                message: "ffi-call second argument must be a string literal function name".into(),
                span: Some(arg_exprs[1].span()),
            })?;

        if !has_foreign_lib(env, lib_name) {
            return Err(TypeError::InvalidTypeAnnotation {
                message: format!(
                    "ffi-call on '{lib_name}' requires a (foreign {lib_name} ...) declaration"
                ),
                span: Some(op_span),
            });
        }

        let decl = lookup_foreign_fn(env, lib_name, foreign_fn_name).ok_or_else(|| {
            TypeError::InvalidTypeAnnotation {
                message: format!(
                    "foreign function '{foreign_fn_name}' not declared in (foreign {lib_name} ...)"
                ),
                span: Some(arg_exprs[1].span()),
            }
        })?;

        let decl_params = decl.params.clone();
        let decl_result = decl.result.clone();
        let call_args = &arg_exprs[2..]; // skip lib and fn-name

        // Allow zero-arg calls to thunk-typed FFI fns: params=[Unit] with no call args
        let is_implicit_unit_thunk =
            call_args.is_empty() && decl_params.len() == 1 && decl_params[0] == t_unit();
        if call_args.len() != decl_params.len() && !is_implicit_unit_thunk {
            return Err(TypeError::ArityMismatch {
                expected: decl_params.len(),
                got: call_args.len(),
                context: format!("foreign function {foreign_fn_name}"),
                span: Some(op_span),
            });
        }

        // Infer all args (lib, fn-name, then call args)
        let (mut subst, all_effects, all_arg_types) =
            infer_exprs(env, arg_exprs, session, empty_subst(), effect_row_empty())?;

        // Unify each call arg type with its declared param type
        let call_arg_types = &all_arg_types[2..]; // skip lib and fn-name
        for (arg_type, param_type) in call_arg_types.iter().zip(&decl_params) {
            let arg_type = apply_subst(&subst, arg_type);
            let unify_result = unify_with_context(
                &arg_type,
                param_type,
                &format!("argument to foreign function {foreign_fn_name}"),
            )?;
            subst = compose_subst(&unify_result, &subst);
        }

        return Ok(InferResult {
            ty: decl_result,
            subst,
            effects: all_effects,
        });
    }

    // Non-ffi-call variadic functions: struct-new, struct-get, struct-set, list
    let (mut subst, all_effects, arg_types) =
        infer_exprs(env, arg_exprs, session, empty_subst(), effect_row_empty())?;

    let result_type = match fn_name {
        "struct-new" => {
            // (struct-new StructDef val1 val2 ...)
            // First arg is the struct def, remaining are field values
            if !arg_exprs.is_empty() {
                if let Some(sym_name) = ast::get_symbol_name(arg_exprs[0], session.interner) {
                    if let Some(info) = lookup_ffi_struct_info(env, sym_name) {
                        let value_args = &arg_types[1..]; // skip struct def arg
                        if value_args.len() != info.fields.len() {
                            return Err(TypeError::ArityMismatch {
                                expected: info.fields.len(),
                                got: value_args.len(),
                                context: format!("struct-new {}", sym_name),
                                span: Some(arg_exprs[0].span()),
                            });
                        }
                        // Unify each value arg with its field's Marlang type
                        for (i, field) in info.fields.iter().enumerate() {
                            let val_type = apply_subst(&subst, &value_args[i]);
                            let unify_result = unify_with_context(
                                &val_type,
                                &field.marlang_type,
                                &format!("field '{}' of struct-new {}", field.name, sym_name),
                            )?;
                            subst = compose_subst(&unify_result, &subst);
                        }
                        type_const(sym_name)
                    } else if is_ffi_struct_type(env, sym_name) {
                        type_const(sym_name)
                    } else {
                        t_ffi_struct()
                    }
                } else {
                    t_ffi_struct()
                }
            } else {
                t_ffi_struct()
            }
        }
        "struct-get" => {
            // (struct-get struct-instance "field-name")
            if arg_exprs.len() >= 2 {
                if let Some(field_name) = get_string_literal(arg_exprs[1]) {
                    // Try to determine the struct type from the first argument
                    let struct_type = apply_subst(&subst, &arg_types[0]);
                    if let Type::Const(ref type_name) = struct_type {
                        if let Some(info) = lookup_ffi_struct_info(env, type_name) {
                            let field = info.fields.iter().find(|f| f.name == field_name);
                            match field {
                                Some(f) => f.marlang_type.clone(),
                                None => {
                                    return Err(TypeError::InvalidTypeAnnotation {
                                        message: format!(
                                            "struct '{}' has no field '{}'",
                                            type_name, field_name
                                        ),
                                        span: Some(arg_exprs[1].span()),
                                    });
                                }
                            }
                        } else {
                            fresh_type_var() // unknown struct def → polymorphic fallback
                        }
                    } else {
                        fresh_type_var() // can't determine struct type → polymorphic fallback
                    }
                } else {
                    fresh_type_var() // field name not a string literal
                }
            } else {
                fresh_type_var()
            }
        }
        "struct-set" => {
            // (struct-set struct-instance "field-name" value)
            if arg_exprs.len() >= 3
                && let Some(field_name) = get_string_literal(arg_exprs[1])
            {
                let struct_type = apply_subst(&subst, &arg_types[0]);
                if let Type::Const(ref type_name) = struct_type
                    && let Some(info) = lookup_ffi_struct_info(env, type_name)
                {
                    let field = info.fields.iter().find(|f| f.name == field_name);
                    match field {
                        Some(f) => {
                            let val_type = apply_subst(&subst, &arg_types[2]);
                            let unify_result = unify_with_context(
                                &val_type,
                                &f.marlang_type,
                                &format!("field '{}' of struct-set {}", field_name, type_name),
                            )?;
                            subst = compose_subst(&unify_result, &subst);
                        }
                        None => {
                            return Err(TypeError::InvalidTypeAnnotation {
                                message: format!(
                                    "struct '{}' has no field '{}'",
                                    type_name, field_name
                                ),
                                span: Some(arg_exprs[1].span()),
                            });
                        }
                    }
                }
            }
            t_unit()
        }
        "list" => type_app(type_const("List"), vec![fresh_type_var()]),
        _ => fresh_type_var(),
    };

    Ok(InferResult {
        ty: result_type,
        subst,
        effects: all_effects,
    })
}

fn infer_application(
    env: &TypeEnv,
    elements: &[&Expr],
    session: &InferSession,
) -> Result<InferResult, TypeError> {
    let fn_expr = elements[0];
    let arg_exprs = &elements[1..];

    // Check for variadic arithmetic
    if let Some(fn_name) = ast::get_symbol_name(fn_expr, session.interner) {
        if variadic_arith_min_args(fn_name).is_some() {
            return infer_variadic_arith(env, fn_name, arg_exprs, fn_expr.span(), session);
        }
        if FFI_VARIADIC_FUNCTIONS.contains(&fn_name) {
            return infer_ffi_application(env, fn_name, arg_exprs, fn_expr.span(), session);
        }
    }

    let fn_result = infer_expr(env, fn_expr, session)?;

    let (subst, mut all_effects, arg_types) =
        infer_exprs(env, arg_exprs, session, fn_result.subst, fn_result.effects)?;

    let result_type = fresh_type_var();
    let fresh_eff_var = fresh_effect_row_var();
    // Zero-arg calls get an implicit Unit param to match thunk types (-> Unit X),
    // but not for variadic functions (which can legitimately take zero args).
    let fn_type = apply_subst(&subst, &fn_result.ty);
    let is_variadic = matches!(&fn_type, Type::Arrow { rest: Some(_), .. });
    let expected_params: Vec<Type> = if arg_types.is_empty() && !is_variadic {
        vec![t_unit()]
    } else {
        arg_types.iter().map(|t| apply_subst(&subst, t)).collect()
    };
    let expected_fn_type = type_arrow_with_effects(
        expected_params,
        Some(effect_row_open(vec![], fresh_eff_var)),
        result_type.clone(),
    );

    let app_span = fn_expr.span();
    let app_subst = unify_with_context(&fn_type, &expected_fn_type, "function application")
        .map_err(|e| e.with_span_if_missing(app_span))?;
    let final_subst = compose_subst(&app_subst, &subst);

    // Propagate function effects
    let applied_fn_type = apply_subst(&final_subst, &fn_result.ty);
    if let Type::Arrow {
        effects: Some(eff), ..
    } = &applied_fn_type
    {
        all_effects = combine_effect_rows(&all_effects, eff);
    }

    Ok(InferResult {
        ty: apply_subst(&final_subst, &result_type),
        subst: final_subst,
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

fn parse_effect_op(expr: &Expr, env: &TypeEnv, interner: &Interner) -> Result<EffectOp, TypeError> {
    let elements = ast::list_to_array(expr);
    if elements.len() != 2 {
        return Err(TypeError::InvalidTypeAnnotation {
            message: "Effect operation must be (name (-> ParamType ResultType))".into(),
            span: Some(expr.span()),
        });
    }

    let op_name =
        ast::get_symbol_name(elements[0], interner).ok_or(TypeError::InvalidTypeAnnotation {
            message: "Effect operation name must be a symbol".into(),
            span: Some(elements[0].span()),
        })?;

    let type_expr = elements[1];
    let mut arrow_expr = type_expr;

    // Check for constrained type: (=> constraints... arrow-type) — just skip to the arrow type
    if let Expr::List { .. } = type_expr {
        let type_elements = ast::list_to_array(type_expr);
        if !type_elements.is_empty() && is_symbol(type_elements[0], interner, "=>") {
            if type_elements.len() < 3 {
                return Err(TypeError::InvalidTypeAnnotation {
                    message: "Constrained type needs constraints and a body".into(),
                    span: Some(type_expr.span()),
                });
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
                span: Some(arrow_expr.span()),
            });
        }
    };

    // Collect type parameters
    let mut type_params = Vec::new();
    let mut seen = HashSet::new();
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
    session: &InferSession,
) -> Result<InferResult, TypeError> {
    if elements.len() < 2 {
        return Err(TypeError::ArityMismatch {
            expected: 1,
            got: elements.len() - 1,
            context: "perform".into(),
            span: Some(elements[0].span()),
        });
    }

    let op_name = ast::get_symbol_name(elements[1], session.interner).ok_or(
        TypeError::InvalidTypeAnnotation {
            message: "perform operation name must be a symbol".into(),
            span: Some(elements[1].span()),
        },
    )?;

    let op_name_str = op_name.to_string();
    let (effect, op) = if let Some((effect_name, op_name_only)) = op_name_str.rsplit_once('/') {
        let effect_decl = lookup_effect(env, effect_name).ok_or(TypeError::UndefinedVariable {
            name: format!("effect '{effect_name}' (not found)"),
            span: Some(elements[1].span()),
        })?;
        let op = lookup_effect_op_in_effect(env, effect_name, op_name_only).ok_or(
            TypeError::UndefinedVariable {
                name: format!("effect operation '{op_name}' in effect '{effect_name}'"),
                span: Some(elements[1].span()),
            },
        )?;
        // Record reference using the unqualified operation name (matches definition)
        session.record_reference(op_name_only, elements[1].span(), SymbolKind::EffectOp);
        (effect_decl, op)
    } else {
        let possible_effects = find_effects_with_op(env, op_name);
        if possible_effects.len() > 1 {
            return Err(TypeError::AmbiguousEffectOperation {
                op_name: op_name.to_string(),
                effects: possible_effects.clone(),
                suggestion: format!("({}/{})", possible_effects[0], op_name),
                span: Some(elements[1].span()),
            });
        }
        let result = lookup_effect_op(env, op_name).ok_or(TypeError::UndefinedVariable {
            name: format!("effect operation '{op_name}' (declare with (effect EffectName ({op_name} : ArgType -> ResultType)))"),
            span: Some(elements[1].span()),
        })?;
        // Record reference for the operation name
        session.record_reference(op_name, elements[1].span(), SymbolKind::EffectOp);
        result
    };

    let arg_exprs = &elements[2..];
    let (mut subst, mut all_effects, arg_types) =
        infer_exprs(env, arg_exprs, session, empty_subst(), effect_row_empty())?;

    // Allow zero-arg perform on thunk-typed ops: (perform op) when op has params=[Unit]
    let is_implicit_unit_thunk =
        arg_types.is_empty() && op.params.len() == 1 && op.params[0] == t_unit();
    if arg_types.len() != op.params.len() && !is_implicit_unit_thunk {
        return Err(TypeError::ArityMismatch {
            expected: op.params.len(),
            got: arg_types.len(),
            context: format!("effect operation {op_name}"),
            span: Some(elements[0].span()),
        });
    }

    // Instantiate type params
    let mut tp_subst = HashMap::new();
    for tv in &op.type_params {
        tp_subst.insert(tv.clone(), fresh_type_var());
    }

    let instantiated_params: Vec<Type> = op
        .params
        .iter()
        .map(|p| apply_subst(&tp_subst, p))
        .collect();
    let instantiated_result = apply_subst(&tp_subst, &op.result);

    for (i, arg_type) in arg_types.iter().enumerate() {
        let at = apply_subst(&subst, arg_type);
        let pt = &instantiated_params[i];
        let perform_span = elements[0].span();
        let unify_result = unify_with_context(&at, pt, &format!("argument {} of {op_name}", i + 1))
            .map_err(|e| e.with_span_if_missing(perform_span))?;
        subst = compose_subst(&unify_result, &subst);
    }

    let result_type = apply_subst(&subst, &instantiated_result);
    all_effects = combine_effect_rows(&all_effects, &effect_row_closed(vec![effect.name.clone()]));

    Ok(InferResult {
        ty: result_type,
        subst,
        effects: all_effects,
    })
}

/// Resolve an effect operation name to its (effect_name, op_name) pair.
///
/// Handles both qualified (`Effect/op`) and unqualified (`op`) names.
/// For unqualified names, looks up the operation across all effects and
/// errors if ambiguous.
fn resolve_effect_op(
    env: &TypeEnv,
    op_name: &str,
    span: Span,
) -> Result<(String, String), TypeError> {
    if let Some((e, o)) = op_name.rsplit_once('/') {
        return Ok((e.to_string(), o.to_string()));
    }
    let possible_effects = find_effects_with_op(env, op_name);
    if possible_effects.len() > 1 {
        return Err(TypeError::AmbiguousEffectOperation {
            op_name: op_name.to_string(),
            effects: possible_effects.clone(),
            suggestion: format!("({}/{})", possible_effects[0], op_name),
            span: Some(span),
        });
    }
    let (effect, _op) = lookup_effect_op(env, op_name).ok_or(TypeError::UndefinedVariable {
        name: format!("effect operation '{op_name}' in handler (declare with (effect EffectName ({op_name} : ArgType -> ResultType)))"),
        span: Some(span),
    })?;
    Ok((effect.name.clone(), op_name.to_string()))
}

/// Check that all operations of each handled effect are covered.
fn check_handler_exhaustiveness(
    env: &TypeEnv,
    handled_ops_by_effect: &HashMap<String, Vec<String>>,
    span: Span,
) -> Result<(), TypeError> {
    for (effect_name, handled_op_names) in handled_ops_by_effect {
        let Some(effect_decl) = lookup_effect(env, effect_name) else {
            continue;
        };
        let missing: Vec<String> = effect_decl
            .operations
            .iter()
            .filter(|op| !handled_op_names.iter().any(|h| h == &op.name))
            .map(|op| op.name.clone())
            .collect();
        if !missing.is_empty() {
            return Err(TypeError::NonExhaustiveHandlers {
                effect_name: effect_name.clone(),
                missing_ops: missing,
                span: Some(span),
            });
        }
    }
    Ok(())
}

/// Remove handled effects from the body's effect row.
fn remove_handled_effects(
    body_effects: &EffectRow,
    handled_ops_by_effect: &HashMap<String, Vec<String>>,
) -> EffectRow {
    if handled_ops_by_effect.is_empty() {
        return body_effects.clone();
    }
    let remaining: Vec<String> = body_effects
        .effects()
        .iter()
        .filter(|e| !handled_ops_by_effect.contains_key(*e))
        .cloned()
        .collect();
    match body_effects {
        EffectRow::Open { row_var, .. } => effect_row_open(remaining, row_var.clone()),
        EffectRow::Closed(_) => effect_row_closed(remaining),
    }
}

fn infer_handle(
    env: &TypeEnv,
    elements: &[&Expr],
    session: &InferSession,
) -> Result<InferResult, TypeError> {
    if elements.len() < 2 {
        return Err(TypeError::ArityMismatch {
            expected: 1,
            got: elements.len() - 1,
            context: "handle".into(),
            span: Some(elements[0].span()),
        });
    }

    let body_expr = elements[1];
    let body_result = infer_expr(env, body_expr, session)?;

    let mut subst = body_result.subst.clone();
    let mut result_type: Option<Type> = None;
    let mut handled_ops_by_effect: HashMap<String, Vec<String>> = HashMap::new();
    let mut has_return_handler = false;

    for clause in &elements[2..] {
        let clause_elements = ast::list_to_array(clause);
        if clause_elements.len() != 3 {
            return Err(TypeError::InvalidTypeAnnotation {
                message: "handle clause must be ((pattern) -> body)".into(),
                span: Some(clause.span()),
            });
        }
        if !is_symbol(clause_elements[1], session.interner, "->") {
            return Err(TypeError::InvalidTypeAnnotation {
                message: "handle clause must be ((pattern) -> body)".into(),
                span: Some(clause_elements[1].span()),
            });
        }

        let pattern = clause_elements[0];
        let body = clause_elements[2];
        let Expr::List { .. } = pattern else { continue };
        let pattern_elements = ast::list_to_array(pattern);
        if pattern_elements.is_empty() {
            return Err(TypeError::InvalidTypeAnnotation {
                message: "Handler pattern must have operation name".into(),
                span: Some(pattern.span()),
            });
        }

        let Some(op_name) = ast::get_symbol_name(pattern_elements[0], session.interner) else {
            continue;
        };

        if op_name == "return" {
            // Return handler — bind the return variable and infer the body
            has_return_handler = true;
            let mut handler_env = apply_subst_env(&subst, env);
            if pattern_elements.len() >= 2
                && let Some(return_var_name) =
                    ast::get_symbol_name(pattern_elements[1], session.interner)
            {
                handler_env.bindings_mut().insert(
                    return_var_name.to_string(),
                    monotype(apply_subst(&subst, &body_result.ty)),
                );
            }
            let handler_result = infer_expr(&handler_env, body, session)?;
            subst = compose_subst(&handler_result.subst, &subst);
            let handler_type = apply_subst(&subst, &handler_result.ty);
            unify_branch(
                &mut result_type,
                &mut subst,
                handler_type,
                "handler return types",
                None,
            )?;
            continue;
        }

        // Effect operation handler
        let (effect_name, op_name_only) =
            resolve_effect_op(env, op_name, pattern_elements[0].span())?;

        let op_info = lookup_effect_op_in_effect(env, &effect_name, &op_name_only).ok_or(
            TypeError::UndefinedVariable {
                name: format!("effect operation '{op_name_only}' in effect '{effect_name}'"),
                span: Some(pattern_elements[0].span()),
            },
        )?;
        // Validate that the effect itself exists
        let _effect_decl =
            lookup_effect(env, &effect_name).ok_or(TypeError::UndefinedVariable {
                name: format!("effect '{effect_name}' not found"),
                span: Some(pattern_elements[0].span()),
            })?;

        handled_ops_by_effect
            .entry(effect_name.clone())
            .or_default()
            .push(op_name_only);

        // Build handler environment: bind operation arguments and continuation
        let mut handler_env = apply_subst_env(&subst, env);
        let arg_patterns = &pattern_elements[1..pattern_elements.len().saturating_sub(1)];
        for (i, arg_p) in arg_patterns.iter().enumerate() {
            if let Some(arg_name) = ast::get_symbol_name(arg_p, session.interner)
                && i < op_info.params.len()
            {
                handler_env
                    .bindings_mut()
                    .insert(arg_name.to_string(), monotype(op_info.params[i].clone()));
            }
        }
        if let Some(cont_p) = pattern_elements.last()
            && let Some(k_name) = ast::get_symbol_name(cont_p, session.interner)
        {
            let k_type = type_arrow(
                vec![op_info.result.clone()],
                apply_subst(&subst, &body_result.ty),
            );
            handler_env
                .bindings_mut()
                .insert(k_name.to_string(), monotype(k_type));
        }

        let handler_result = infer_expr(&handler_env, body, session)?;
        subst = compose_subst(&handler_result.subst, &subst);
        let handler_type = apply_subst(&subst, &handler_result.ty);
        unify_branch(
            &mut result_type,
            &mut subst,
            handler_type,
            "handler return types",
            None,
        )?;
    }

    // Without a return handler, the body's value passes through unchanged when no
    // effect is performed, so the body type must unify with the handler arms' types.
    if !has_return_handler {
        let body_type = apply_subst(&subst, &body_result.ty);
        unify_branch(
            &mut result_type,
            &mut subst,
            body_type,
            "handle body and handler return types",
            Some(body_expr.span()),
        )?;
    }

    let final_type = result_type.unwrap_or_else(|| body_result.ty.clone());

    check_handler_exhaustiveness(env, &handled_ops_by_effect, elements[0].span())?;

    // Ensure the body's effect row explicitly includes the handled effects.
    // If the body has an open row variable (e.g. {ε12}), we expand it by
    // binding ε12 → {HandledEffect | ε_fresh}. This propagates through
    // substitution so e.g. a thunk parameter becomes (-> Unit {Foo | ε_fresh} t)
    // while the handler's remaining effects become {ε_fresh}.
    let body_effects = apply_effect_row_subst_via_type_subst(&subst, &body_result.effects);
    let handled_effect_names: Vec<String> = handled_ops_by_effect.keys().cloned().collect();
    let missing: Vec<String> = handled_effect_names
        .iter()
        .filter(|e| !body_effects.effects().contains(e))
        .cloned()
        .collect();
    let expanded_effects = if !missing.is_empty() {
        if let EffectRow::Open { row_var, effects } = &body_effects {
            // Create a fresh row variable and bind the old one to {missing... + effects... | fresh}
            let fresh_rv = fresh_effect_row_var();
            let mut expanded = effects.clone();
            for e in &missing {
                if !expanded.contains(e) {
                    expanded.push(e.clone());
                }
            }
            let expansion_subst = single_subst(
                row_var,
                Type::EffectRowRef(effect_row_open(expanded.clone(), fresh_rv.clone())),
            );
            subst = compose_subst(&expansion_subst, &subst);
            effect_row_open(expanded, fresh_rv)
        } else {
            // Closed row — just add the effects (they'll be removed)
            combine_effect_rows(&body_effects, &effect_row_closed(missing))
        }
    } else {
        body_effects
    };
    let remaining_effects = remove_handled_effects(&expanded_effects, &handled_ops_by_effect);

    Ok(InferResult {
        ty: apply_subst(&subst, &final_type),
        subst,
        effects: remaining_effects,
    })
}

fn infer_resume(
    env: &TypeEnv,
    elements: &[&Expr],
    session: &InferSession,
) -> Result<InferResult, TypeError> {
    if elements.len() != 3 {
        return Err(TypeError::ArityMismatch {
            expected: 2,
            got: elements.len() - 1,
            context: "resume".into(),
            span: Some(elements[0].span()),
        });
    }

    let cont_result = infer_expr(env, elements[1], session)?;
    let mut subst = cont_result.subst;
    let mut all_effects = cont_result.effects;

    let value_env = apply_subst_env(&subst, env);
    let value_result = infer_expr(&value_env, elements[2], session)?;
    subst = compose_subst(&value_result.subst, &subst);
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
        let type_elements = ast::list_to_array(type_expr);
        type_name = ast::get_symbol_name(type_elements[0], interner)
            .ok_or(TypeError::InvalidTypeAnnotation {
                message: "data type name must be a symbol".into(),
                span: Some(type_elements[0].span()),
            })?
            .to_string();
        for te in &type_elements[1..] {
            type_params.push(
                ast::get_symbol_name(te, interner)
                    .ok_or(TypeError::InvalidTypeAnnotation {
                        message: "type parameter must be a symbol".into(),
                        span: Some(te.span()),
                    })?
                    .to_string(),
            );
        }
    } else {
        type_name = ast::get_symbol_name(type_expr, interner)
            .ok_or(TypeError::InvalidTypeAnnotation {
                message: "data type name must be a symbol".into(),
                span: Some(type_expr.span()),
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
                let ctor_elements = ast::list_to_array(ctor_expr);
                let ctor_name = ast::get_symbol_name(ctor_elements[0], interner)
                    .ok_or(TypeError::InvalidTypeAnnotation {
                        message: "constructor name must be a symbol".into(),
                        span: Some(ctor_elements[0].span()),
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
            ty: ctor_type,
        };

        new_env.bindings_mut().insert(ctor.name.clone(), scheme);
    }

    Ok(new_env)
}

/// Process a top-level `(declare name Type)` form.
/// Registers a type signature for a name that already has a runtime implementation
/// (e.g. a Rust builtin). No body is required or inferred.
fn process_declare(
    env: &TypeEnv,
    elements: &[&Expr],
    session: &InferSession,
) -> Result<TypeEnv, TypeError> {
    if elements.len() != 3 {
        return Err(TypeError::ArityMismatch {
            expected: 2,
            got: elements.len() - 1,
            context: "declare".into(),
            span: Some(elements[0].span()),
        });
    }

    let name = ast::get_symbol_name(elements[1], session.interner).ok_or(
        TypeError::InvalidTypeAnnotation {
            message: "declare expects a symbol name".into(),
            span: Some(elements[1].span()),
        },
    )?;

    // Record the declared name as a definition for go-to-definition support.
    // This is important for builtins declared in the prelude — the LSP needs
    // a definition site to jump to.
    session.record_definition(name, elements[1].span(), SymbolKind::Variable);

    let ty = parse_type_expr(elements[2], env, session.interner)?;

    // Generalize: quantify over all free type variables
    let free_vars: Vec<String> = free_type_vars(&ty).into_iter().collect();
    let scheme = TypeScheme {
        quantified_vars: free_vars,
        ty,
    };

    Ok(extend_env(env, name, scheme))
}

fn process_define(
    env: &TypeEnv,
    elements: &[&Expr],
    current_subst: &Substitution,
    session: &InferSession,
) -> Result<(TypeEnv, Substitution), TypeError> {
    let name_expr = elements[1];
    let value_expr = elements[2];

    let name: String;
    let mut annotated_type: Option<Type> = None;
    let name_span: Span;

    if let Expr::List { .. } = name_expr {
        let name_elements = ast::list_to_array(name_expr);
        if name_elements.len() == 3 && is_symbol(name_elements[1], session.interner, ":") {
            name = ast::get_symbol_name(name_elements[0], session.interner)
                .ok_or(TypeError::InvalidTypeAnnotation {
                    message: "Expected symbol before :".into(),
                    span: Some(name_elements[0].span()),
                })?
                .to_string();
            name_span = name_elements[0].span();

            let ty = parse_type_annotation(name_elements[2], env, session.interner)?;
            annotated_type = Some(ty);
        } else {
            return Err(TypeError::InvalidTypeAnnotation {
                message: "Invalid define syntax".into(),
                span: Some(elements[1].span()),
            });
        }
    } else {
        name = ast::get_symbol_name(name_expr, session.interner)
            .ok_or(TypeError::InvalidTypeAnnotation {
                message: "define expects a symbol name".into(),
                span: Some(elements[1].span()),
            })?
            .to_string();
        name_span = name_expr.span();
    }

    session.record_definition(&name, name_span, SymbolKind::Variable);

    // Enter a new binding level for generalization
    enter_level();

    // Add placeholder for recursive functions
    let placeholder_type = annotated_type.clone().unwrap_or_else(fresh_type_var);
    let infer_env = extend_env(env, &name, monotype(placeholder_type));

    let applied_env = apply_subst_env(current_subst, &infer_env);
    let value_result = infer_expr(&applied_env, value_expr, session)?;

    let mut subst = compose_subst(&value_result.subst, current_subst);
    let mut value_type = value_result.ty;

    if let Some(ref ann_type) = annotated_type {
        let def_span = elements[1].span();
        let unify_result =
            unify_with_context(ann_type, &value_type, &format!("definition of {name}"))
                .map_err(|e| e.with_span_if_missing(def_span))?;
        subst = compose_subst(&unify_result, &subst);
        value_type = apply_subst(&subst, ann_type);
    }

    // Exit level before generalizing — vars created at the inner level
    // will have level > current_level and thus be quantified.
    exit_level();

    let gen_env = apply_subst_env(&subst, env);
    let scheme = generalize(&gen_env, &apply_subst(&subst, &value_type));

    let new_env = extend_env(env, &name, scheme);
    Ok((new_env, subst))
}

fn process_effect_def(
    env: &TypeEnv,
    elements: &[&Expr],
    session: &InferSession,
) -> Result<TypeEnv, TypeError> {
    let interner = session.interner;
    if elements.len() < 2 {
        return Err(TypeError::ArityMismatch {
            expected: 1,
            got: elements.len() - 1,
            context: "effect".into(),
            span: Some(elements[0].span()),
        });
    }

    let name_expr = elements[1];
    let effect_name = match name_expr {
        Expr::Atom(LispValue::Symbol(sym), _) => interner.resolve(*sym).to_string(),
        _ => {
            return Err(TypeError::InvalidTypeAnnotation {
                message: "effect name must be a symbol".into(),
                span: Some(elements[1].span()),
            });
        }
    };

    // Record the effect name as a definition
    session.record_definition(&effect_name, name_expr.span(), SymbolKind::Effect);

    let mut operations = Vec::new();
    for e in &elements[2..] {
        let op = parse_effect_op(e, env, interner)?;

        // Record each operation name as a definition.
        // The op name is the first element of the operation form (e.g., `print` in `(print (-> String Unit))`).
        let op_elements = ast::list_to_array(e);
        if !op_elements.is_empty() {
            session.record_definition(&op.name, op_elements[0].span(), SymbolKind::EffectOp);
        }

        operations.push(op);
    }

    let effect_decl = EffectDeclaration {
        name: effect_name,
        type_params: vec![],
        operations,
    };

    Ok(define_effect(env, effect_decl))
}

fn process_foreign_def(
    env: &TypeEnv,
    elements: &[&Expr],
    interner: &Interner,
) -> Result<TypeEnv, TypeError> {
    if elements.len() < 2 {
        return Err(TypeError::ArityMismatch {
            expected: 1,
            got: elements.len() - 1,
            context: "foreign".into(),
            span: Some(elements[0].span()),
        });
    }

    let lib_name =
        ast::get_symbol_name(elements[1], interner).ok_or(TypeError::InvalidTypeAnnotation {
            message: "foreign library name must be a symbol".into(),
            span: Some(elements[1].span()),
        })?;

    let mut functions = Vec::new();
    for e in &elements[2..] {
        let op_elements = ast::list_to_array(e);
        if op_elements.len() != 2 {
            return Err(TypeError::InvalidTypeAnnotation {
                message: "foreign function entry must be (FnName ArrowType)".into(),
                span: Some(e.span()),
            });
        }

        let fn_name = ast::get_symbol_name(op_elements[0], interner).ok_or(
            TypeError::InvalidTypeAnnotation {
                message: "foreign function name must be a symbol".into(),
                span: Some(op_elements[0].span()),
            },
        )?;

        let fn_type = parse_type_expr(op_elements[1], env, interner)?;
        let (params, result) = match &fn_type {
            Type::Arrow { params, result, .. } => (params.clone(), result.as_ref().clone()),
            _ => {
                return Err(TypeError::InvalidTypeAnnotation {
                    message: "foreign function type must be a function type (-> ...)".into(),
                    span: Some(op_elements[1].span()),
                });
            }
        };

        functions.push(ForeignFnDecl {
            name: fn_name.to_string(),
            params,
            result,
        });
    }

    Ok(define_foreign(env, lib_name, functions))
}

/// Convert FFI type names in arrow signatures to Marlang types for type checking.
/// E.g. I32 → Int, F32 → Float, CString → String, Void → Unit, etc.
/// Returns an error if an unknown struct type is referenced that wasn't defined via ffi-struct.
fn ffi_type_to_marlang(ty: &Type, env: &TypeEnv, span: Span) -> Result<Type, TypeError> {
    match ty {
        Type::Const(name) => match name.as_str() {
            "I8" | "U8" | "I16" | "U16" | "I32" | "U32" | "I64" | "U64" | "Int" => Ok(t_int()),
            "F32" | "F64" | "Float" => Ok(t_float()),
            "CString" => Ok(t_string()),
            "Bool" => Ok(t_bool()),
            "Void" => Ok(t_unit()),
            "Ptr" => Ok(t_ffi_pointer()),
            "FFIStruct" | "FFIPointer" | "FFILibrary" | "String" | "Unit" => Ok(ty.clone()),
            _ => {
                if is_ffi_struct_type(env, name) {
                    Ok(type_const(name)) // distinct struct type
                } else {
                    Err(TypeError::UndefinedType {
                        name: format!(
                            "struct type '{}' used in foreign-lib but not defined via ffi-struct",
                            name
                        ),
                        span: Some(span),
                    })
                }
            }
        },
        Type::Arrow {
            params,
            rest,
            effects,
            result,
        } => {
            let mut new_params = Vec::new();
            for p in params {
                new_params.push(ffi_type_to_marlang(p, env, span)?);
            }
            let new_rest = match rest {
                Some(r) => Some(Box::new(ffi_type_to_marlang(r, env, span)?)),
                None => None,
            };
            let new_result = ffi_type_to_marlang(result, env, span)?;
            Ok(Type::Arrow {
                params: new_params,
                rest: new_rest,
                effects: effects.clone(),
                result: Box::new(new_result),
            })
        }
        _ => Ok(ty.clone()),
    }
}

/// Map lowercase FFI type strings ("u8", "f64", etc.) to Type::Const values.
fn ffi_type_string_to_type_const(s: &str) -> Type {
    match s {
        "i8" | "u8" | "i16" | "u16" | "i32" | "u32" | "i64" | "u64" => t_int(),
        "f32" | "f64" => t_float(),
        "bool" => t_bool(),
        "string" | "cstring" => t_string(),
        "void" => t_unit(),
        "ptr" | "pointer" => t_ffi_pointer(),
        _ => type_const(s), // unknown → pass through as named type
    }
}

/// Check if an expression is `(ffi-struct "Name" (list (list "field" "type") ...))` and extract info.
fn try_extract_ffi_struct_info(expr: &Expr, interner: &Interner) -> Option<FfiStructInfo> {
    let Expr::List { .. } = expr else { return None };
    let elements = ast::list_to_array(expr);
    if elements.len() < 2 || !is_symbol(elements[0], interner, "ffi-struct") {
        return None;
    }
    let Expr::Atom(LispValue::String(name), _) = elements[1] else {
        return None;
    };

    let mut fields = Vec::new();

    // Parse field definitions from the third argument: (list (list "field" "type") ...)
    if elements.len() >= 3
        && let Expr::List { .. } = elements[2]
    {
        let list_elements = ast::list_to_array(elements[2]);
        // Skip the leading `list` symbol
        let field_entries =
            if !list_elements.is_empty() && is_symbol(list_elements[0], interner, "list") {
                &list_elements[1..]
            } else {
                &list_elements[..]
            };
        for entry in field_entries {
            let Expr::List { .. } = entry else { continue };
            let pair = ast::list_to_array(entry);
            // (list "field_name" "type_string")
            let inner = if pair.len() >= 3 && is_symbol(pair[0], interner, "list") {
                &pair[1..]
            } else {
                &pair[..]
            };
            if let [
                Expr::Atom(LispValue::String(fname), _),
                Expr::Atom(LispValue::String(ftype), _),
            ] = inner
            {
                fields.push(FfiStructFieldInfo {
                    name: fname.to_string(),
                    marlang_type: ffi_type_string_to_type_const(ftype),
                });
            }
        }
    }

    Some(FfiStructInfo {
        name: name.to_string(),
        fields,
    })
}

fn process_ffi_lib_def(
    env: &TypeEnv,
    elements: &[&Expr],
    interner: &Interner,
) -> Result<TypeEnv, TypeError> {
    // (foreign-lib <name> <path> (<FnName> (-> <Types>...)) ...)
    if elements.len() < 3 {
        return Err(TypeError::ArityMismatch {
            expected: 2,
            got: elements.len() - 1,
            context: "foreign-lib".into(),
            span: Some(elements[0].span()),
        });
    }

    let lib_name =
        ast::get_symbol_name(elements[1], interner).ok_or(TypeError::InvalidTypeAnnotation {
            message: "foreign-lib library name must be a symbol".into(),
            span: Some(elements[1].span()),
        })?;

    // elements[2] is the path string (we just check it's a string)
    match elements[2] {
        Expr::Atom(LispValue::String(_), _) => {}
        _ => {
            return Err(TypeError::InvalidTypeAnnotation {
                message: "foreign-lib second argument must be a path string".into(),
                span: Some(elements[2].span()),
            });
        }
    }

    let mut functions = Vec::new();
    let mut new_env = env.clone();

    for e in &elements[3..] {
        let op_elements = ast::list_to_array(e);
        if op_elements.len() != 2 {
            return Err(TypeError::InvalidTypeAnnotation {
                message: "foreign-lib function entry must be (FnName ArrowType)".into(),
                span: Some(e.span()),
            });
        }

        let fn_name = ast::get_symbol_name(op_elements[0], interner).ok_or(
            TypeError::InvalidTypeAnnotation {
                message: "foreign-lib function name must be a symbol".into(),
                span: Some(op_elements[0].span()),
            },
        )?;

        // Parse the type in FFI type namespace
        let raw_type = parse_type_expr(op_elements[1], env, interner)?;
        // Convert FFI types to Marlang types (errors on unknown struct types)
        let marlang_type = ffi_type_to_marlang(&raw_type, &new_env, op_elements[1].span())?;

        let (params, result) = match &marlang_type {
            Type::Arrow { params, result, .. } => (params.clone(), result.as_ref().clone()),
            _ => {
                return Err(TypeError::InvalidTypeAnnotation {
                    message: "foreign-lib function type must be a function type (-> ...)".into(),
                    span: Some(op_elements[1].span()),
                });
            }
        };

        // Register in foreign_libs (for ffi-call backward compat)
        functions.push(ForeignFnDecl {
            name: fn_name.to_string(),
            params: params.clone(),
            result: result.clone(),
        });

        // Register qualified binding: lib_name/FnName
        let qualified_name = format!("{}/{}", lib_name, fn_name);
        let fn_type = type_arrow(params, result);
        new_env
            .bindings_mut()
            .insert(qualified_name, monotype(fn_type));
    }

    // Register foreign lib for ffi-call backward compatibility
    new_env = define_foreign(&new_env, lib_name, functions);

    // Register lib_name with type FFILibrary
    new_env
        .bindings_mut()
        .insert(lib_name.to_string(), monotype(t_ffi_library()));

    Ok(new_env)
}

pub fn infer_program(
    env: &TypeEnv,
    exprs: &[Expr],
    interner: &Interner,
) -> Result<(TypeEnv, Vec<Type>), TypeError> {
    let session = InferSession::new(interner);
    infer_program_inner(env, exprs, &session, None, None, None, 0)
}

/// Type-check a program with module/import support.
pub fn infer_program_with_context(
    env: &TypeEnv,
    exprs: &[Expr],
    interner: &Interner,
    file_path: Option<&Path>,
    loader: &dyn TypeModuleLoader,
    cache: &TypeModuleCache,
) -> Result<(TypeEnv, Vec<Type>), TypeError> {
    let session = InferSession::new(interner);
    infer_program_inner(
        env,
        exprs,
        &session,
        file_path,
        Some(loader),
        Some(cache),
        0,
    )
}

/// Type-check a program with module/import support and an InferSession (for LSP context collection).
pub fn infer_program_with_session(
    env: &TypeEnv,
    exprs: &[Expr],
    session: &InferSession<'_>,
    file_path: Option<&Path>,
    loader: &dyn TypeModuleLoader,
    cache: &TypeModuleCache,
    prelude_count: usize,
) -> Result<(TypeEnv, Vec<Type>), TypeError> {
    infer_program_inner(
        env,
        exprs,
        session,
        file_path,
        Some(loader),
        Some(cache),
        prelude_count,
    )
}

fn infer_program_inner(
    env: &TypeEnv,
    exprs: &[Expr],
    session: &InferSession,
    file_path: Option<&Path>,
    loader: Option<&dyn TypeModuleLoader>,
    cache: Option<&TypeModuleCache>,
    prelude_count: usize,
) -> Result<(TypeEnv, Vec<Type>), TypeError> {
    let mut current_env = env.clone();
    let mut types = Vec::new();
    let mut global_subst = empty_subst();

    // Use a closure so `?` inside the loop captures the error without returning
    // from the outer function. This lets us apply the (possibly partial) substitution
    // to the context's type_map on both success and error paths.
    let loop_result: Result<(), TypeError> = (|| {
        for (expr_idx, expr) in exprs.iter().enumerate() {
            // After processing all prelude expressions, snapshot the definition count.
            // Definitions with index < prelude_definition_count have spans that are
            // byte offsets within prelude.mar, not the user's file.
            if expr_idx == prelude_count
                && let Some(ctx) = &session.context
            {
                let count = ctx.borrow().definitions.len();
                ctx.borrow_mut().prelude_definition_count = count;
            }
            if let Expr::List { .. } = expr {
                let elements = ast::list_to_array(expr);
                if !elements.is_empty() {
                    let first_sym = ast::get_symbol_name(elements[0], session.interner);

                    if first_sym == Some("data") {
                        current_env = process_data_def(&current_env, &elements, session.interner)?;
                        types.push(t_unit());
                        continue;
                    }

                    if first_sym == Some("define") && elements.len() >= 3 {
                        // Check if this define is binding an ffi-struct call
                        if let Some(info) =
                            try_extract_ffi_struct_info(elements[2], session.interner)
                        {
                            let name = info.name.clone();
                            register_ffi_struct_type(&mut current_env, &name, info);
                        }
                        let (new_env, new_subst) =
                            process_define(&current_env, &elements, &global_subst, session)?;
                        current_env = new_env;
                        global_subst = new_subst;
                        types.push(t_unit());
                        continue;
                    }

                    if first_sym == Some("declare") && elements.len() >= 3 {
                        current_env = process_declare(&current_env, &elements, session)?;
                        types.push(t_unit());
                        continue;
                    }

                    if first_sym == Some("effect") && elements.len() >= 2 {
                        current_env = process_effect_def(&current_env, &elements, session)?;
                        types.push(t_unit());
                        continue;
                    }

                    if first_sym == Some("foreign") && elements.len() >= 2 {
                        current_env =
                            process_foreign_def(&current_env, &elements, session.interner)?;
                        types.push(t_unit());
                        continue;
                    }

                    if first_sym == Some("foreign-lib") && elements.len() >= 3 {
                        current_env =
                            process_ffi_lib_def(&current_env, &elements, session.interner)?;
                        types.push(t_unit());
                        continue;
                    }

                    if first_sym == Some("import") {
                        if let (Some(loader), Some(cache)) = (loader, cache) {
                            current_env = process_import(
                                &current_env,
                                &elements,
                                session,
                                file_path,
                                loader,
                                cache,
                            )?;
                        }
                        // If no loader, silently skip (backward compat)
                        types.push(t_unit());
                        continue;
                    }
                }
            }

            // Regular expression
            let applied_env = apply_subst_env(&global_subst, &current_env);
            let result = infer_expr(&applied_env, expr, session)?;

            // Check for unhandled effects at the top level.
            // Effect names may be module-qualified (e.g. "io/IO"), so we
            // strip the prefix before checking against the runtime set.
            if expr_idx >= prelude_count {
                let unhandled: Vec<String> = result
                    .effects
                    .effects()
                    .iter()
                    .filter(|e| {
                        let base = e.rsplit_once('/').map_or(e.as_str(), |(_, name)| name);
                        !RUNTIME_HANDLED_EFFECTS.contains(&base)
                    })
                    .cloned()
                    .collect();
                if !unhandled.is_empty() {
                    return Err(TypeError::UnhandledEffect {
                        effects: unhandled,
                        span: Some(expr.span()),
                    });
                }
            }

            global_subst = compose_subst(&result.subst, &global_subst);
            types.push(apply_subst(&global_subst, &result.ty));
        }

        Ok(())
    })();

    // Apply the final (or partial, on error) substitution to the context's type_map
    // so hover shows resolved types like `Int` instead of unresolved vars like `t0`.
    if let Some(ctx) = &session.context {
        ctx.borrow_mut().apply_substitution(&global_subst);
    }

    loop_result?;
    Ok((current_env, types))
}

// ===== Import Processing =====

fn parse_import_form(
    elements: &[&Expr],
    interner: &Interner,
) -> Result<(String, ImportForm), TypeError> {
    // (import "path")                → All
    // (import "path" (name1 name2))  → Selective
    // (import "path" as alias)       → Qualified

    if elements.len() < 2 {
        return Err(TypeError::InvalidTypeAnnotation {
            message: "import expects a module path".into(),
            span: Some(elements[0].span()),
        });
    }

    let path = match elements[1] {
        Expr::Atom(LispValue::String(s), _) => s.to_string(),
        _ => {
            return Err(TypeError::InvalidTypeAnnotation {
                message: "import expects a string path".into(),
                span: Some(elements[1].span()),
            });
        }
    };

    if elements.len() == 2 {
        return Ok((path, ImportForm::All));
    }

    // Check for `as alias`
    if elements.len() == 4
        && is_symbol(elements[2], interner, "as")
        && let Some(alias) = ast::get_symbol_name(elements[3], interner)
    {
        return Ok((path, ImportForm::Qualified(alias.to_string())));
    }

    // Check for selective: (import "path" (name1 name2 ...))
    if elements.len() == 3
        && let Expr::List { .. } = elements[2]
    {
        let name_exprs = ast::list_to_array(elements[2]);
        let mut names = Vec::new();
        for ne in &name_exprs {
            if let Some(name) = ast::get_symbol_name(ne, interner) {
                names.push(name.to_string());
            } else {
                return Err(TypeError::InvalidTypeAnnotation {
                    message: "Selective import expects symbols".into(),
                    span: Some(ne.span()),
                });
            }
        }
        return Ok((path, ImportForm::Selective(names)));
    }

    Err(TypeError::InvalidTypeAnnotation {
        message: "Invalid import form".into(),
        span: Some(elements[0].span()),
    })
}

/// Record cross-file definition entries for an imported module.
///
/// Takes the module's definitions (name + span pairs), the import form, the module's
/// file path, and the module's exported binding names. For each exported binding that
/// has a matching definition, records a cross-file entry mapping the imported name
/// (possibly qualified) to the original name, file path, and definition span.
fn record_cross_file_definitions(
    session: &InferSession,
    module_defs: &[(String, Span, SymbolKind)],
    module_exports: &TypeEnv,
    form: &ImportForm,
    resolved_path: &Path,
) {
    let ctx = match &session.context {
        Some(c) => c,
        None => return,
    };

    // For each exported binding, find the last definition with that name in module_defs
    // (last = innermost/most recent in program order)
    for export_name in module_exports.bindings().keys() {
        let def = module_defs
            .iter()
            .rev()
            .find(|(name, _, _)| name == export_name);

        if let Some((_, span, kind)) = def {
            let imported_name = match form {
                ImportForm::Qualified(alias) => format!("{}/{}", alias, export_name),
                ImportForm::All => export_name.clone(),
                ImportForm::Selective(names) => {
                    if names.iter().any(|n| n == export_name) {
                        export_name.clone()
                    } else {
                        continue;
                    }
                }
            };
            ctx.borrow_mut().cross_file_definitions.push((
                imported_name,
                export_name.clone(),
                resolved_path.to_path_buf(),
                *span,
                *kind,
            ));
        }
    }

    // Also handle constructors (data type constructors like Some, None, Cons, etc.)
    for export_name in module_exports.constructors().keys() {
        let def = module_defs
            .iter()
            .rev()
            .find(|(name, _, _)| name == export_name);

        if let Some((_, span, kind)) = def {
            let imported_name = match form {
                ImportForm::Qualified(alias) => format!("{}/{}", alias, export_name),
                ImportForm::All => export_name.clone(),
                ImportForm::Selective(names) => {
                    if names.iter().any(|n| n == export_name) {
                        export_name.clone()
                    } else {
                        continue;
                    }
                }
            };
            ctx.borrow_mut().cross_file_definitions.push((
                imported_name,
                export_name.clone(),
                resolved_path.to_path_buf(),
                *span,
                *kind,
            ));
        }
    }

    // Also handle effect operations (e.g., print, println from IO effect)
    for effect_decl in module_exports.effects().values() {
        for op in &effect_decl.operations {
            let export_name = &op.name;
            let def = module_defs
                .iter()
                .rev()
                .find(|(name, _, kind)| name == export_name && *kind == SymbolKind::EffectOp);

            if let Some((_, span, kind)) = def {
                let imported_name = match form {
                    ImportForm::Qualified(alias) => format!("{}/{}", alias, export_name),
                    ImportForm::All => export_name.clone(),
                    ImportForm::Selective(names) => {
                        if names.iter().any(|n| n == export_name) {
                            export_name.clone()
                        } else {
                            continue;
                        }
                    }
                };
                ctx.borrow_mut().cross_file_definitions.push((
                    imported_name,
                    export_name.clone(),
                    resolved_path.to_path_buf(),
                    *span,
                    *kind,
                ));
            }
        }
    }
}

fn process_import(
    env: &TypeEnv,
    elements: &[&Expr],
    session: &InferSession,
    file_path: Option<&Path>,
    loader: &dyn TypeModuleLoader,
    cache: &TypeModuleCache,
) -> Result<TypeEnv, TypeError> {
    let (path_str, form) = parse_import_form(elements, session.interner)?;

    // Resolve and load module
    let (module_exprs, resolved_path) =
        loader.resolve_and_load(&path_str, file_path, session.interner)?;

    // Check cache
    match cache.get_state(&resolved_path) {
        Some(ModuleCacheState::Loading) => {
            return Err(TypeError::InternalError {
                message: format!("Circular dependency detected: {}", resolved_path.display()),
                span: Some(elements[0].span()),
            });
        }
        Some(ModuleCacheState::Loaded(module_env)) => {
            // Module was already type-checked — use cached module_definitions for cross-file defs
            if let Some(ctx) = &session.context {
                let module_defs = ctx.borrow().module_definitions.get(&resolved_path).cloned();
                if let Some(defs) = module_defs {
                    record_cross_file_definitions(
                        session,
                        &defs,
                        &module_env,
                        &form,
                        &resolved_path,
                    );
                }
            }
            return Ok(merge_module_env(env, &module_env, &form));
        }
        None => {}
    }

    // Mark as loading
    cache.mark_loading(resolved_path.clone());

    // Snapshot the definitions count so we can extract module-specific definitions after type-checking
    let defs_before = session
        .context
        .as_ref()
        .map(|c| c.borrow().definitions.len())
        .unwrap_or(0);

    // Use the current env (builtins + prelude + prior imports) as the baseline.
    // After type-checking the module, we extract everything it added on top
    // of this baseline as its exports.
    let baseline_binding_keys: HashSet<String> = env.bindings().keys().cloned().collect();
    let baseline_data_type_keys: HashSet<String> = env.data_types().keys().cloned().collect();
    let baseline_constructor_keys: HashSet<String> = env.constructors().keys().cloned().collect();
    let baseline_effect_keys: HashSet<String> = env.effects().keys().cloned().collect();
    let baseline_alias_keys: HashSet<String> = env.aliases().keys().cloned().collect();

    // Type-check the module using the current env as base (has builtins + prelude).
    // The prelude types (Option, Result, etc.) are available for the module code.
    // prelude_count is 0 because imported modules have their prelude stripped by the loader.
    let (module_env, _) = infer_program_inner(
        env,
        &module_exprs,
        session,
        Some(&resolved_path),
        Some(loader),
        Some(cache),
        0,
    )?;

    // Extract everything the module defines beyond the baseline env.
    let module_own_env = extract_module_exports(
        &module_env,
        &baseline_binding_keys,
        &baseline_data_type_keys,
        &baseline_constructor_keys,
        &baseline_effect_keys,
        &baseline_alias_keys,
    );

    // Extract module definitions recorded during type-checking and cache them
    if let Some(ctx) = &session.context {
        // Clone the module's definitions out of the borrow before mutating the context
        let module_defs: Vec<(String, Span, SymbolKind)> = {
            let defs = &ctx.borrow().definitions;
            defs[defs_before..].to_vec()
        };

        // Record cross-file definitions for this import
        record_cross_file_definitions(
            session,
            &module_defs,
            &module_own_env,
            &form,
            &resolved_path,
        );

        // Cache the module's definitions for future cached imports
        ctx.borrow_mut()
            .module_definitions
            .insert(resolved_path.clone(), module_defs);
    }

    cache.mark_loaded(resolved_path, module_own_env.clone());

    // Merge into current env
    Ok(merge_module_env(env, &module_own_env, &form))
}

/// Extract definitions that are new relative to the builtin key sets.
fn extract_module_exports(
    full: &TypeEnv,
    base_bindings: &HashSet<String>,
    base_data_types: &HashSet<String>,
    base_constructors: &HashSet<String>,
    base_effects: &HashSet<String>,
    base_aliases: &HashSet<String>,
) -> TypeEnv {
    let mut exports = empty_env();

    for (name, scheme) in full.bindings() {
        if !base_bindings.contains(name) {
            exports.bindings_mut().insert(name.clone(), scheme.clone());
        }
    }

    for (name, dt) in full.data_types() {
        if !base_data_types.contains(name) {
            exports.data_types_mut().insert(name.clone(), dt.clone());
        }
    }

    for (name, entry) in full.constructors() {
        if !base_constructors.contains(name) {
            exports
                .constructors_mut()
                .insert(name.clone(), entry.clone());
        }
    }

    for (name, effect) in full.effects() {
        if !base_effects.contains(name) {
            exports.effects_mut().insert(name.clone(), effect.clone());
        }
    }

    for (name, alias) in full.aliases() {
        if !base_aliases.contains(name) {
            exports.aliases_mut().insert(name.clone(), alias.clone());
        }
    }

    exports
}
