use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use marlang_core::ast::{self, Expr, LispValue};
use marlang_core::intern::{Interner, Symbol};
use marlang_core::lexer::lex;
use marlang_core::parser::{parse, parse_with_interner};
use marlang_core::span::Span;

use crate::builtins::register_builtins;
use crate::compiler::compile_program;
use crate::object::VMObject;
use crate::value::Value;
use crate::vm::{VM, call_value, interpret};

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
    as_sym: Symbol,
    next_mark: u64,
    expansion_depth: u32,
    file_path: Option<PathBuf>,
    stdlib_dir: Option<PathBuf>,
    module_macro_cache: HashMap<PathBuf, MacroExports>,
    expanding_paths: HashSet<PathBuf>,
    virtual_modules: HashMap<PathBuf, String>,
}

/// Cached macro exports for a module
#[derive(Clone)]
struct MacroExports {
    /// Macros defined by this module itself
    own: HashMap<Symbol, Value>,
    /// All macros visible in this module (own + imported)
    all: HashMap<Symbol, Value>,
}

impl<'a> MacroExpander<'a> {
    pub fn new(interner: &'a Interner) -> Self {
        let mut vm = VM::new();
        register_builtins(&mut vm);

        let immune = build_immune_set(interner);

        let expander = MacroExpander {
            vm,
            macros: HashMap::new(),
            own_macros: HashSet::new(),
            interner,
            immune,
            defmacro_sym: interner.intern("defmacro"),
            quote_sym: interner.intern("quote"),
            quasiquote_sym: interner.intern("quasiquote"),
            import_sym: interner.intern("import"),
            as_sym: interner.intern("as"),
            next_mark: 1,
            expansion_depth: 0,
            file_path: None,
            stdlib_dir: None,
            module_macro_cache: HashMap::new(),
            expanding_paths: HashSet::new(),
            virtual_modules: HashMap::new(),
        };
        expander
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
        match self.expand_all(exprs) {
            Ok(remaining) => remaining,
            Err(_) => vec![],
        }
    }

    /// Run prelude expressions in the expansion VM so it has access to
    /// data constructors (Nil, Cons, etc.) and utility functions for quasiquote.
    pub fn run_prelude_in_vm(&mut self, exprs: &[Expr]) {
        for expr in exprs {
            if let Ok(chunk) = compile_program(std::slice::from_ref(expr), self.interner) {
                let _ = interpret(&mut self.vm, &chunk);
            }
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
    pub fn expand_all(&mut self, exprs: Vec<Expr>) -> Result<Vec<Expr>, MacroError> {
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
                result.push(expanded);
            }
        }

        Ok(result)
    }

    /// Get the exported macros (for cross-module imports).
    pub fn exported_macros(&self) -> HashMap<Symbol, Value> {
        self.macros.clone()
    }

    fn is_defmacro(&self, expr: &Expr) -> bool {
        if let Expr::List { .. } = expr {
            let elements = ast::list_to_array(expr);
            if let Some(first) = elements.first() {
                if let Expr::Atom(LispValue::Symbol(sym), _) = first {
                    return *sym == self.defmacro_sym;
                }
            }
        }
        false
    }

    fn is_import(&self, expr: &Expr) -> bool {
        if let Expr::List { .. } = expr {
            let elements = ast::list_to_array(expr);
            if let Some(first) = elements.first() {
                if let Expr::Atom(LispValue::Symbol(sym), _) = first {
                    return *sym == self.import_sym;
                }
            }
        }
        false
    }

    fn is_macro_call(&self, expr: &Expr) -> Option<Symbol> {
        if let Expr::List { .. } = expr {
            let elements = ast::list_to_array(expr);
            if let Some(first) = elements.first() {
                if let Expr::Atom(LispValue::Symbol(sym), _) = first {
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

        // Compile and run in expansion VM
        let tokens = lex(&lambda_source, self.interner)
            .map_err(|e| MacroError::new(format!("defmacro compile error: {}", e)))?;
        let exprs =
            parse(&tokens).map_err(|e| MacroError::new(format!("defmacro parse error: {}", e)))?;
        let chunk = compile_program(&exprs, self.interner)
            .map_err(|e| MacroError::new(format!("defmacro compile error: {}", e)))?;
        interpret(&mut self.vm, &chunk)
            .map_err(|e| MacroError::new(format!("defmacro execution error: {}", e)))?;

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
        match expr {
            Expr::Atom(LispValue::Symbol(sym), _) => self.interner.resolve(*sym).to_string(),
            Expr::Atom(LispValue::Number(n, is_float), _) => {
                if *is_float {
                    format!("{}", n)
                } else {
                    let i = *n as i64;
                    if (i as f64 - n).abs() < 1e-10 {
                        format!("{}", i)
                    } else {
                        format!("{}", n)
                    }
                }
            }
            Expr::Atom(LispValue::String(s), _) => format!("\"{}\"", s),
            Expr::Nil(_) => "()".to_string(),
            Expr::List { .. } => {
                let elems = ast::list_to_array(expr);
                let parts: Vec<String> = elems.iter().map(|e| self.expr_to_source(e)).collect();
                format!("({})", parts.join(" "))
            }
        }
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

        // Check arity for closures
        if let Some(VMObject::Closure(closure)) = transformer.as_object() {
            let expected = closure.proto.arity as usize;
            if arg_values.len() != expected {
                let name = self.interner.resolve(macro_sym);
                return Err(MacroError::new(format!(
                    "Macro '{}' expects {} arguments, got {}",
                    name,
                    expected,
                    arg_values.len()
                )));
            }
        }

        let result = call_value(&mut self.vm, &transformer, &arg_values)
            .map_err(|e| MacroError::new(format!("Macro expansion runtime error: {}", e)))?;

        // 4. Convert back to AST
        let result_expr = self.value_to_expr(&result, Span::DUMMY)?;

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
                if let Expr::Atom(LispValue::Symbol(sym), _) = head.as_ref() {
                    if *sym == self.quote_sym {
                        return Ok(Expr::List { head, tail, span });
                    }
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
        if let Some(cached) = self.module_macro_cache.get(&resolved) {
            let cached = cached.clone();
            self.register_imported_macros(&cached.own, &cached.all, qualifier.as_deref());
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

        // Expand the module (we don't need the expanded expressions, just the macros)
        let expand_result = self.expand_all(exprs);
        let module_all_macros = self.macros.clone();
        let module_own_macros: HashMap<Symbol, Value> = self
            .own_macros
            .iter()
            .filter_map(|sym| self.macros.get(sym).map(|v| (*sym, v.clone())))
            .collect();

        // Restore state
        self.macros = saved_macros;
        self.own_macros = saved_own;
        self.expansion_depth = saved_depth;
        self.file_path = saved_file;
        self.expanding_paths.remove(&resolved);

        // If expansion failed, skip (let runtime handle)
        if expand_result.is_err() {
            return Ok(());
        }

        // Cache module exports
        let exports = MacroExports {
            own: module_own_macros.clone(),
            all: module_all_macros.clone(),
        };
        self.module_macro_cache.insert(resolved, exports);

        // Register with qualifier
        self.register_imported_macros(&module_own_macros, &module_all_macros, qualifier.as_deref());

        Ok(())
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
        if import_path.starts_with("std:") {
            let module_name = &import_path[4..];
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
            Expr::Atom(LispValue::Number(n, _), _) => Value::number(*n),
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
        if let Some(n) = value.as_number() {
            let is_float = n != n.trunc() || n.is_infinite() || n.is_nan();
            return Ok(Expr::Atom(LispValue::Number(n, is_float), span));
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
    match name.find('~') {
        Some(pos) => {
            let rest = &name[pos + 1..];
            // Verify the rest is digits possibly interspersed with ~digits
            if !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit() || b == b'~') {
                &name[..pos]
            } else {
                name
            }
        }
        None => name,
    }
}

// --- Immune set ---

fn build_immune_set(interner: &Interner) -> HashSet<Symbol> {
    let names = [
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
    ];
    names.iter().map(|n| interner.intern(n)).collect()
}
