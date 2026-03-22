use crate::builtins::register_builtins;
use crate::bytecode::ROOT_MODULE;
use crate::compiler::compile_expr_with_context;
use crate::modules::is_constructor_value;
use crate::object::VMObject;
use crate::value::Value;
use crate::vm::{VM, call_value, interpret};
use marlang_core::ast::{self, Expr, LispValue};
use marlang_core::intern::{Interner, Symbol};
use marlang_core::lexer::lex;
use marlang_core::parser::{parse, parse_with_interner};
use marlang_core::span::Span;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

// --- Macro expansion error ---

#[derive(Debug)]
pub struct MacroError {
    pub message: String,
}

impl MacroError {
    fn new(msg: impl Into<String>) -> Self {
        MacroError {
            message: msg.into(),
        }
    }
}

impl std::fmt::Display for MacroError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Macro expansion error: {}", self.message)
    }
}

impl std::error::Error for MacroError {}

// --- MacroExpander ---

pub struct MacroExpander<'a> {
    vm: VM,
    macros: HashMap<Symbol, Value>,
    /// Macros defined in this module (not imported)
    own_macros: HashSet<Symbol>,
    interner: &'a Interner,
    immune: HashSet<Symbol>,
    defmacro_sym: Symbol,
    quote_sym: Symbol,
    #[allow(dead_code)]
    quasiquote_sym: Symbol,
    import_sym: Symbol,
    begin_sym: Symbol,
    as_sym: Symbol,
    next_mark: u64,
    expansion_depth: u32,
    file_path: Option<PathBuf>,
    stdlib_dir: Option<PathBuf>,
    module_export_cache: HashMap<PathBuf, ModuleExports>,
    expanding_paths: HashSet<PathBuf>,
    virtual_modules: HashMap<PathBuf, String>,
    /// Accumulated constructor knowledge across compilations in the expansion VM.
    known_constructors: HashSet<Symbol>,
    /// Accumulated effect-op knowledge across compilations in the expansion VM.
    known_effect_ops: HashMap<Symbol, Symbol>,
    /// When true, run each expression in the expansion VM during expand_all.
    /// Set by expand_import (to capture module globals) or when expand_all detects
    /// defmacros that may reference preceding definitions.
    force_vm_execution: bool,
}

/// Cached exports for a module (macros + globals for compile-time use)
#[derive(Clone)]
struct ModuleExports {
    /// Macros defined by this module itself
    own_macros: HashMap<Symbol, Value>,
    /// All macros visible in this module (own + imported)
    all_macros: HashMap<Symbol, Value>,
    /// Global names exported by this module (for qualified import aliasing)
    global_names: Vec<String>,
}

impl<'a> MacroExpander<'a> {
    pub fn new(interner: &'a Interner) -> Self {
        let mut vm = VM::new();
        register_builtins(&mut vm);

        let immune = build_immune_set(interner);

        MacroExpander {
            vm,
            macros: HashMap::new(),
            own_macros: HashSet::new(),
            interner,
            immune,
            defmacro_sym: interner.intern("defmacro"),
            quote_sym: interner.intern("quote"),
            quasiquote_sym: interner.intern("quasiquote"),
            import_sym: interner.intern("import"),
            begin_sym: interner.intern("begin"),
            as_sym: interner.intern("as"),
            next_mark: 1,
            expansion_depth: 0,
            file_path: None,
            stdlib_dir: None,
            module_export_cache: HashMap::new(),
            expanding_paths: HashSet::new(),
            virtual_modules: HashMap::new(),
            known_constructors: HashSet::new(),
            known_effect_ops: HashMap::new(),
            force_vm_execution: false,
        }
    }

    /// Load the prelude file, register any macros, and return remaining expressions.
    /// Returns (prelude_exprs, prelude was loaded successfully).
    pub fn load_prelude(&mut self) -> Vec<Expr> {
        let stdlib_dir = match &self.stdlib_dir {
            Some(d) => d.clone(),
            None => return vec![],
        };
        let prelude_path = stdlib_dir.join("prelude.mar");
        let source = match std::fs::read_to_string(&prelude_path) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        let tokens = match lex(&source, self.interner) {
            Ok(t) => t,
            Err(_) => return vec![],
        };
        let exprs = match parse_with_interner(&tokens, self.interner) {
            Ok(e) => e,
            Err(_) => return vec![],
        };
        // expand_all consumes defmacro forms (registering them) and returns the rest
        self.expand_all(exprs).unwrap_or_default()
    }

    /// Run a single expression in the expansion VM.
    /// Errors are silently ignored — the expression is still passed through to
    /// the runtime. This allows pure definitions to be available at compile time
    /// while effectful code gracefully degrades.
    fn run_expr_in_vm(&mut self, expr: &Expr) {
        if let Ok(result) = compile_expr_with_context(
            expr,
            self.interner,
            &self.known_constructors,
            &self.known_effect_ops,
            ROOT_MODULE,
        ) {
            self.known_constructors = result.known_constructors;
            self.known_effect_ops = result.known_effect_ops;
            let _ = interpret(&mut self.vm, &result.chunk);
        }
    }

    pub fn with_file_path(mut self, path: &Path) -> Self {
        self.file_path = Some(path.to_path_buf());
        self
    }

    pub fn with_stdlib_dir(mut self, dir: PathBuf) -> Self {
        self.stdlib_dir = Some(dir);
        self
    }

    pub fn with_virtual_modules(mut self, modules: HashMap<PathBuf, String>) -> Self {
        self.virtual_modules = modules;
        self
    }

    fn fresh_mark(&mut self) -> u64 {
        let m = self.next_mark;
        self.next_mark += 1;
        m
    }

    /// Expand all top-level expressions.
    /// Returns the expanded expressions and any macro definitions found (for cross-module use).
    ///
    /// Every expression is also compiled and executed in the expansion VM so that
    /// definitions (functions, data types, etc.) are available to subsequent macros.
    /// This enables macros to run arbitrary Marlang code at compile time.
    ///
    /// Per R5RS §5.1, top-level `(begin ...)` is a splicing form: its children
    /// are flattened into the enclosing sequence of top-level forms. This allows
    /// macros to expand into multiple top-level definitions.
    pub fn expand_all(&mut self, exprs: Vec<Expr>) -> Result<Vec<Expr>, MacroError> {
        // Run expressions in the expansion VM if:
        // 1. force_vm_execution is set (e.g., inside an import), OR
        // 2. This expression list contains defmacros that may reference
        //    preceding definitions
        let run_in_vm = self.force_vm_execution || exprs.iter().any(|e| self.is_defmacro(e));

        let mut result = Vec::new();

        for expr in exprs {
            if self.is_defmacro(&expr) {
                self.register_macro(&expr)?;
                // defmacro is consumed — not passed through
            } else if self.is_import(&expr) {
                self.expand_import(&expr)?;
                result.push(expr);
            } else {
                let expanded = self.expand_expr(expr)?;
                // Run in expansion VM so later macros can use this definition
                if run_in_vm {
                    self.run_expr_in_vm(&expanded);
                }
                self.splice_top_level(expanded, &mut result, run_in_vm)?;
            }
        }

        Ok(result)
    }

    /// If `expr` is a top-level `(begin ...)`, recursively splice its children
    /// into `out`. Otherwise push the expression as-is.
    /// When `run_in_vm` is true, each non-macro expression is also run in the expansion VM.
    fn splice_top_level(
        &mut self,
        expr: Expr,
        out: &mut Vec<Expr>,
        run_in_vm: bool,
    ) -> Result<(), MacroError> {
        if self.is_begin(&expr) {
            let children: Vec<Expr> = ast::list_to_array(&expr)
                .into_iter()
                .skip(1)
                .cloned()
                .collect();
            for child in children {
                if self.is_defmacro(&child) {
                    self.register_macro(&child)?;
                } else {
                    if run_in_vm {
                        self.run_expr_in_vm(&child);
                    }
                    self.splice_top_level(child, out, run_in_vm)?;
                }
            }
        } else {
            out.push(expr);
        }
        Ok(())
    }

    fn is_begin(&self, expr: &Expr) -> bool {
        if let Expr::List { .. } = expr {
            let elements = ast::list_to_array(expr);
            if let Some(first) = elements.first()
                && let Expr::Atom(LispValue::Symbol(sym), _) = first
            {
                return *sym == self.begin_sym;
            }
        }
        false
    }

    /// Get the exported macros (for cross-module imports).
    pub fn exported_macros(&self) -> HashMap<Symbol, Value> {
        self.macros.clone()
    }

    fn is_defmacro(&self, expr: &Expr) -> bool {
        if let Expr::List { .. } = expr {
            let elements = ast::list_to_array(expr);
            if let Some(first) = elements.first()
                && let Expr::Atom(LispValue::Symbol(sym), _) = first
            {
                return *sym == self.defmacro_sym;
            }
        }
        false
    }

    fn is_import(&self, expr: &Expr) -> bool {
        if let Expr::List { .. } = expr {
            let elements = ast::list_to_array(expr);
            if let Some(first) = elements.first()
                && let Expr::Atom(LispValue::Symbol(sym), _) = first
            {
                return *sym == self.import_sym;
            }
        }
        false
    }

    fn is_macro_call(&self, expr: &Expr) -> Option<Symbol> {
        if let Expr::List { .. } = expr {
            let elements = ast::list_to_array(expr);
            if let Some(first) = elements.first()
                && let Expr::Atom(LispValue::Symbol(sym), _) = first
            {
                if self.macros.contains_key(sym) {
                    return Some(*sym);
                }
                // Fallback: strip marks and check again
                let name = self.interner.resolve(*sym);
                let stripped = strip_all_marks(name);
                if stripped != name {
                    let stripped_sym = self.interner.intern(stripped);
                    if self.macros.contains_key(&stripped_sym) {
                        return Some(stripped_sym);
                    }
                }
            }
        }
        None
    }

    // --- Macro registration ---

    fn register_macro(&mut self, expr: &Expr) -> Result<(), MacroError> {
        let elements = ast::list_to_array(expr);
        // (defmacro name (params) body...)
        if elements.len() < 4 {
            return Err(MacroError::new(
                "defmacro expects at least: name, params, body",
            ));
        }

        let name_sym = match elements[1] {
            Expr::Atom(LispValue::Symbol(sym), _) => *sym,
            _ => return Err(MacroError::new("defmacro: name must be a symbol")),
        };

        // Build (define name (lambda (params) body...))
        let name_str = self.interner.resolve(name_sym);
        let lambda_source = self.build_macro_lambda_source(name_str, &elements[2..])?;

        // Compile and run in expansion VM, using accumulated constructor knowledge
        // so that match patterns in macro bodies correctly recognise ADT constructors.
        let tokens = lex(&lambda_source, self.interner)
            .map_err(|e| MacroError::new(format!("defmacro compile error: {}", e)))?;
        let exprs =
            parse(&tokens).map_err(|e| MacroError::new(format!("defmacro parse error: {}", e)))?;
        for expr in &exprs {
            let result = compile_expr_with_context(
                expr,
                self.interner,
                &self.known_constructors,
                &self.known_effect_ops,
                ROOT_MODULE,
            )
            .map_err(|e| MacroError::new(format!("defmacro compile error: {}", e)))?;
            self.known_constructors = result.known_constructors;
            self.known_effect_ops = result.known_effect_ops;
            interpret(&mut self.vm, &result.chunk)
                .map_err(|e| MacroError::new(format!("defmacro execution error: {}", e)))?;
        }

        // Retrieve the transformer closure from globals (root module)
        let transformer = self.vm.globals().get(name_str).cloned().ok_or_else(|| {
            MacroError::new(format!(
                "defmacro: transformer '{}' not found after definition",
                name_str
            ))
        })?;

        self.macros.insert(name_sym, transformer);
        self.own_macros.insert(name_sym);
        Ok(())
    }

    fn build_macro_lambda_source(&self, name: &str, rest: &[&Expr]) -> Result<String, MacroError> {
        // rest[0] = params, rest[1..] = body
        let params = self.expr_to_source(rest[0]);
        let body_parts: Vec<String> = rest[1..].iter().map(|e| self.expr_to_source(e)).collect();
        let body = body_parts.join(" ");
        Ok(format!("(define {} (lambda {} {}))", name, params, body))
    }

    fn expr_to_source(&self, expr: &Expr) -> String {
        ast::expr_to_source(expr, self.interner)
    }

    // --- Macro expansion ---

    fn expand_macro_call(&mut self, macro_sym: Symbol, expr: &Expr) -> Result<Expr, MacroError> {
        self.expansion_depth += 1;
        if self.expansion_depth > 1000 {
            return Err(MacroError::new(
                "Macro expansion depth limit exceeded (1000)",
            ));
        }

        let elements = ast::list_to_array(expr);
        let args = &elements[1..]; // skip the macro name

        let mark = self.fresh_mark();

        // 1. Mark all argument symbols
        let marked_args: Vec<Expr> = args.iter().map(|a| self.apply_mark(a, mark)).collect();

        // 2. Convert to VM values
        let arg_values: Vec<Value> = marked_args.iter().map(|a| self.expr_to_value(a)).collect();

        // 3. Call transformer (with arity check)
        let transformer = self
            .macros
            .get(&macro_sym)
            .cloned()
            .ok_or_else(|| MacroError::new("Macro transformer not found"))?;

        // Check arity for closures (support variadic macros with dot params)
        if let Some(VMObject::Closure(closure)) = transformer.as_object() {
            let expected = closure.proto.arity as usize;
            if closure.proto.is_variadic {
                if arg_values.len() < expected {
                    let name = self.interner.resolve(macro_sym);
                    return Err(MacroError::new(format!(
                        "Macro '{}' expects at least {} arguments, got {}",
                        name,
                        expected,
                        arg_values.len()
                    )));
                }
            } else if arg_values.len() != expected {
                let name = self.interner.resolve(macro_sym);
                return Err(MacroError::new(format!(
                    "Macro '{}' expects {} arguments, got {}",
                    name,
                    expected,
                    arg_values.len()
                )));
            }
        }

        // Pack variadic rest args into a Cons/Nil chain (matching VM's Op::Call
        // behavior, but using ADT values so car/cdr/nil? work in macro bodies)
        let call_args = if let Some(VMObject::Closure(closure)) = transformer.as_object() {
            if closure.proto.is_variadic {
                let required = closure.proto.arity as usize;
                let mut packed = arg_values[..required].to_vec();
                // Build a Cons/Nil list from the rest args
                let mut rest_list = Value::object(VMObject::AdtValue {
                    tag: "Nil".to_string(),
                    fields: vec![],
                });
                for val in arg_values[required..].iter().rev() {
                    rest_list = Value::object(VMObject::AdtValue {
                        tag: "Cons".to_string(),
                        fields: vec![val.clone(), rest_list],
                    });
                }
                packed.push(rest_list);
                packed
            } else {
                arg_values
            }
        } else {
            arg_values
        };

        let result = call_value(&mut self.vm, &transformer, &call_args)
            .map_err(|e| MacroError::new(format!("Macro expansion runtime error: {}", e)))?;

        // 4. Convert back to AST (use the macro call site span so type errors
        //    point to the (when ...) invocation rather than having no location)
        let call_site_span = expr.span();
        let result_expr = self.value_to_expr(&result, call_site_span)?;

        // 5. Mark result
        let marked_result = self.apply_mark(&result_expr, mark);

        // 6. Recursively expand
        let final_result = self.expand_expr(marked_result)?;

        self.expansion_depth -= 1;
        Ok(final_result)
    }

    fn expand_expr(&mut self, expr: Expr) -> Result<Expr, MacroError> {
        // Check if this is a macro call
        if let Some(macro_sym) = self.is_macro_call(&expr) {
            return self.expand_macro_call(macro_sym, &expr);
        }

        match expr {
            Expr::Atom(_, _) | Expr::Nil(_) => Ok(expr),
            Expr::List { head, tail, span } => {
                // Don't expand inside quote
                if let Expr::Atom(LispValue::Symbol(sym), _) = head.as_ref()
                    && *sym == self.quote_sym
                {
                    return Ok(Expr::List { head, tail, span });
                }

                let expanded_head = self.expand_expr(*head)?;
                let expanded_tail = self.expand_list_tail(*tail)?;
                Ok(Expr::List {
                    head: Box::new(expanded_head),
                    tail: Box::new(expanded_tail),
                    span,
                })
            }
        }
    }

    fn expand_list_tail(&mut self, expr: Expr) -> Result<Expr, MacroError> {
        match expr {
            Expr::Nil(_) => Ok(expr),
            Expr::List { head, tail, span } => {
                let expanded_head = self.expand_expr(*head)?;
                let expanded_tail = self.expand_list_tail(*tail)?;
                Ok(Expr::List {
                    head: Box::new(expanded_head),
                    tail: Box::new(expanded_tail),
                    span,
                })
            }
            _ => Ok(expr),
        }
    }

    // --- Cross-module macros ---

    fn expand_import(&mut self, expr: &Expr) -> Result<(), MacroError> {
        let elements = ast::list_to_array(expr);
        if elements.len() < 2 {
            return Ok(()); // let runtime handle the error
        }

        let path_str = match elements[1] {
            Expr::Atom(LispValue::String(s), _) => s.to_string(),
            _ => return Ok(()), // let runtime handle
        };

        // Check for qualified import: (import "path" as alias)
        let qualifier = if elements.len() == 4 {
            if let Expr::Atom(LispValue::Symbol(sym), _) = elements[2] {
                if *sym == self.as_sym {
                    if let Expr::Atom(LispValue::Symbol(alias), _) = elements[3] {
                        Some(self.interner.resolve(*alias).to_string())
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        // Resolve the path — if resolution fails, skip (let runtime handle)
        let resolved = match self.resolve_import_path(&path_str) {
            Ok(p) => p,
            Err(_) => return Ok(()),
        };

        // Check cache
        if let Some(cached) = self.module_export_cache.get(&resolved).cloned() {
            self.register_imported_macros(
                &cached.own_macros,
                &cached.all_macros,
                qualifier.as_deref(),
            );
            // Module already loaded in expansion VM — add qualified aliases if needed
            if let Some(ref q) = qualifier {
                self.add_qualified_vm_aliases(q, &cached.global_names);
            }
            return Ok(());
        }

        // Check for circular import
        if self.expanding_paths.contains(&resolved) {
            return Ok(()); // circular — skip macro collection, runtime will handle values
        }

        // Load source — if loading fails, skip (let runtime handle)
        let source = match self.load_source(&resolved) {
            Ok(s) => s,
            Err(_) => return Ok(()),
        };

        // Parse — if parse fails, skip (let runtime handle)
        let tokens = match lex(&source, self.interner) {
            Ok(t) => t,
            Err(_) => return Ok(()),
        };
        let exprs = match parse_with_interner(&tokens, self.interner) {
            Ok(e) => e,
            Err(_) => return Ok(()),
        };

        // Create child expander
        self.expanding_paths.insert(resolved.clone());

        let saved_macros = self.macros.clone();
        let saved_own = self.own_macros.clone();
        let saved_depth = self.expansion_depth;
        let saved_file = self.file_path.take();

        self.macros = HashMap::new();
        self.own_macros = HashSet::new();
        self.file_path = Some(resolved.clone());
        self.expansion_depth = 0;

        // Force VM execution for imported modules so their globals are available
        // for qualified aliases and compile-time access.
        let saved_force = self.force_vm_execution;
        self.force_vm_execution = true;

        // Enable dirty_globals tracking so we know exactly which names this
        // module wrote, even if they overwrite existing globals (e.g., both
        // list.mar and string.mar define `concat`).
        let saved_dirty = self.vm.dirty_globals.take();
        self.vm.dirty_globals = Some(HashSet::new());

        // Expand and execute the module in the expansion VM.
        // expand_all now runs each expression in the VM automatically.
        let expand_result = self.expand_all(exprs);
        let module_all_macros = self.macros.clone();
        let module_own_macros: HashMap<Symbol, Value> = self
            .own_macros
            .iter()
            .filter_map(|sym| self.macros.get(sym).map(|v| (*sym, v.clone())))
            .collect();

        // Collect the module's global exports from dirty_globals tracking
        let module_global_names: Vec<String> = self
            .vm
            .dirty_globals
            .take()
            .unwrap_or_default()
            .into_iter()
            .collect();
        self.vm.dirty_globals = saved_dirty;

        // Restore state
        self.macros = saved_macros;
        self.own_macros = saved_own;
        self.expansion_depth = saved_depth;
        self.file_path = saved_file;
        self.force_vm_execution = saved_force;
        self.expanding_paths.remove(&resolved);

        // If expansion failed, skip (let runtime handle)
        if expand_result.is_err() {
            return Ok(());
        }

        // For qualified imports, add qualifier/name aliases in the expansion VM
        if let Some(ref q) = qualifier {
            self.add_qualified_vm_aliases(q, &module_global_names);
        }

        // Cache module exports (macros + global names for re-aliasing on cache hit)
        let exports = ModuleExports {
            own_macros: module_own_macros.clone(),
            all_macros: module_all_macros.clone(),
            global_names: module_global_names,
        };
        self.module_export_cache.insert(resolved, exports);

        // Register macros with qualifier
        self.register_imported_macros(&module_own_macros, &module_all_macros, qualifier.as_deref());

        Ok(())
    }

    /// Add qualified aliases (e.g., `list/map`, `list/filter`) in the expansion VM
    /// for a module's exported globals.
    fn add_qualified_vm_aliases(&mut self, qualifier: &str, global_names: &[String]) {
        let aliases: Vec<(String, Value)> = global_names
            .iter()
            .filter_map(|name| {
                let qualified = format!("{}/{}", qualifier, name);
                self.vm.globals().get(name).cloned().map(|v| (qualified, v))
            })
            .collect();
        for (qualified_name, value) in aliases {
            // Also register constructor knowledge for qualified names
            if is_constructor_value(&value) {
                let sym = self.interner.intern(&qualified_name);
                self.known_constructors.insert(sym);
            }
            self.vm.globals_mut().insert(qualified_name, value);
        }
    }

    fn register_imported_macros(
        &mut self,
        own_macros: &HashMap<Symbol, Value>,
        all_macros: &HashMap<Symbol, Value>,
        qualifier: Option<&str>,
    ) {
        if let Some(q) = qualifier {
            // Qualify the module's own macros
            for (sym, val) in own_macros {
                let name = self.interner.resolve(*sym);
                let qualified = format!("{}/{}", q, name);
                let qsym = self.interner.intern(&qualified);
                self.macros.insert(qsym, val.clone());
            }
            // Also register transitive macros (already qualified from their origin)
            // so that expanded templates referencing them can resolve
            for (sym, val) in all_macros {
                if !own_macros.contains_key(sym) {
                    self.macros.insert(*sym, val.clone());
                }
            }
        } else {
            // Unqualified import: register all macros
            for (sym, val) in all_macros {
                self.macros.insert(*sym, val.clone());
            }
        }
    }

    fn resolve_import_path(&self, import_path: &str) -> Result<PathBuf, MacroError> {
        if let Some(module_name) = import_path.strip_prefix("std:") {
            let stdlib = self.stdlib_dir.as_ref().ok_or_else(|| {
                MacroError::new(format!(
                    "Cannot resolve stdlib module '{}': stdlib dir not set",
                    module_name
                ))
            })?;
            let mut path = stdlib.join(module_name);
            if path.extension().is_none() {
                path.set_extension("mar");
            }
            Ok(path)
        } else {
            let from_file = self.file_path.as_deref().unwrap_or(Path::new("."));
            let dir = from_file.parent().unwrap_or(Path::new("."));
            let mut path = dir.join(import_path);
            if path.extension().is_none() {
                path.set_extension("mar");
            }
            if let Ok(canonical) = path.canonicalize() {
                Ok(canonical)
            } else {
                Ok(path)
            }
        }
    }

    fn load_source(&self, path: &Path) -> Result<String, MacroError> {
        if let Some(source) = self.virtual_modules.get(path) {
            return Ok(source.clone());
        }
        std::fs::read_to_string(path)
            .map_err(|_| MacroError::new(format!("Cannot read module: {}", path.display())))
    }

    // --- Mark-based hygiene ---

    fn apply_mark(&self, expr: &Expr, mark: u64) -> Expr {
        match expr {
            Expr::Atom(LispValue::Symbol(sym), span) => {
                if self.immune.contains(sym) {
                    return expr.clone();
                }
                let name = self.interner.resolve(*sym);
                let new_sym = toggle_mark(name, mark, self.interner);
                Expr::Atom(LispValue::Symbol(new_sym), *span)
            }
            Expr::Atom(_, _) | Expr::Nil(_) => expr.clone(),
            Expr::List { head, tail, span } => Expr::List {
                head: Box::new(self.apply_mark(head, mark)),
                tail: Box::new(self.apply_mark(tail, mark)),
                span: *span,
            },
        }
    }

    // --- AST ↔ Value conversion ---

    fn expr_to_value(&self, expr: &Expr) -> Value {
        match expr {
            Expr::Atom(LispValue::Symbol(sym), _) => {
                let name = self.interner.resolve(*sym);
                Value::symbol(name)
            }
            Expr::Atom(LispValue::Number(n, is_float), _) => {
                if *is_float {
                    Value::float(*n)
                } else {
                    Value::int(*n as i64)
                }
            }
            Expr::Atom(LispValue::String(s), _) => Value::string(s.as_ref()),
            Expr::Nil(_) => Value::object(VMObject::AdtValue {
                tag: "Nil".to_string(),
                fields: vec![],
            }),
            Expr::List { head, tail, .. } => {
                let h = self.expr_to_value(head);
                let t = self.expr_to_value(tail);
                Value::object(VMObject::AdtValue {
                    tag: "Cons".to_string(),
                    fields: vec![h, t],
                })
            }
        }
    }

    fn value_to_expr(&self, value: &Value, span: Span) -> Result<Expr, MacroError> {
        if let Some(n) = value.as_int() {
            return Ok(Expr::Atom(LispValue::Number(n as f64, false), span));
        }
        if let Some(n) = value.as_float() {
            return Ok(Expr::Atom(LispValue::Number(n, true), span));
        }
        if value.is_nil() {
            return Ok(Expr::Nil(span));
        }
        if value.is_true() {
            return Ok(Expr::Atom(
                LispValue::Symbol(self.interner.intern("#t")),
                span,
            ));
        }
        if value.is_false() {
            return Ok(Expr::Atom(
                LispValue::Symbol(self.interner.intern("#f")),
                span,
            ));
        }

        match value.as_object() {
            Some(VMObject::Symbol(name)) => {
                let sym = self.interner.intern(name);
                Ok(Expr::Atom(LispValue::Symbol(sym), span))
            }
            Some(VMObject::String(s)) => {
                Ok(Expr::Atom(LispValue::String(Arc::from(s.as_ref())), span))
            }
            Some(VMObject::AdtValue { tag, fields }) if tag == "Nil" && fields.is_empty() => {
                Ok(Expr::Nil(span))
            }
            Some(VMObject::AdtValue { tag, fields }) if tag == "Cons" && fields.len() == 2 => {
                let head = self.value_to_expr(&fields[0], span)?;
                let tail = self.value_to_expr(&fields[1], span)?;
                Ok(Expr::List {
                    head: Box::new(head),
                    tail: Box::new(tail),
                    span,
                })
            }
            _ => Err(MacroError::new(format!(
                "Cannot convert VM value to AST: {}",
                crate::value::format_value(value)
            ))),
        }
    }
}

// --- Toggle mark ---

fn toggle_mark(name: &str, mark: u64, interner: &Interner) -> Symbol {
    let suffix = format!("~{}", mark);
    if let Some(stripped) = name.strip_suffix(&suffix) {
        // Cancel the mark
        interner.intern(stripped)
    } else {
        // Add the mark
        interner.intern(&format!("{}{}", name, suffix))
    }
}

/// Strip all hygiene marks from a name (for fallback lookups).
pub fn strip_all_marks(name: &str) -> &str {
    ast::strip_marks(name)
}

// --- Immune set ---

fn build_immune_set(interner: &Interner) -> HashSet<Symbol> {
    let names = [
        // Special forms
        "define",
        "lambda",
        "if",
        "let",
        "begin",
        "cond",
        "match",
        "data",
        "effect",
        "perform",
        "handle",
        "resume",
        "import",
        "type",
        "declare",
        "foreign",
        "foreign-lib",
        "else",
        "->",
        "return",
        ":",
        "_",
        "#t",
        "#f",
        "nil",
        "quote",
        "quasiquote",
        "unquote",
        "unquote-splicing",
        "defmacro",
        "unshadow",
        // Primitive types — lets macros synthesize type annotations
        "Int",
        "Float",
        "Bool",
        "String",
        "Char",
        "Unit",
        "Bytes",
        // Common type constructors
        "List",
        "Option",
        "Result",
        "Pair",
        // ADT constructors used in type patterns
        "Some",
        "None",
        "Ok",
        "Err",
        "Nil",
        "Cons",
        "MkPair",
        // Effect row syntax (used by macros to synthesize type annotations)
        "__effect_row",
        "__row_var",
    ];
    names.iter().map(|n| interner.intern(n)).collect()
}
