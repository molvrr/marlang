use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use marlang_core::ast::{self, Expr, LispValue};
use marlang_core::intern::{Interner, Symbol};
use marlang_core::lexer::lex;
use marlang_core::parser::parse;

use crate::builtins::register_builtins;
use crate::compiler::compile_expr_with_context;
use crate::errors::VMError;
use crate::value::Value;
use crate::vm::{interpret, VM};

// --- Module state ---

#[derive(Clone)]
enum ModuleState {
    Loading,
    Loaded { globals: HashMap<String, Value> },
}

// --- Import form ---

enum ImportForm {
    All,
    Selective(Vec<String>),
    Qualified(String),
}

// --- ModuleLoader ---

pub struct ModuleLoader {
    cache: HashMap<PathBuf, ModuleState>,
    virtual_modules: HashMap<PathBuf, String>,
    stdlib_dir: Option<PathBuf>,
}

impl ModuleLoader {
    pub fn new() -> Self {
        // Auto-detect stdlib dir: check MARLANG_HOME, then relative to crate root
        let stdlib_dir = std::env::var("MARLANG_HOME")
            .ok()
            .map(|h| PathBuf::from(h).join("stdlib"))
            .or_else(|| {
                // Development: relative to the workspace root
                let manifest = env!("CARGO_MANIFEST_DIR");
                let workspace = Path::new(manifest).parent()?.parent()?.parent()?;
                let stdlib = workspace.join("stdlib");
                if stdlib.is_dir() {
                    Some(stdlib)
                } else {
                    None
                }
            });

        ModuleLoader {
            cache: HashMap::new(),
            virtual_modules: HashMap::new(),
            stdlib_dir,
        }
    }

    /// Create a loader with a specific stdlib directory.
    pub fn with_stdlib_dir(stdlib_dir: PathBuf) -> Self {
        ModuleLoader {
            cache: HashMap::new(),
            virtual_modules: HashMap::new(),
            stdlib_dir: Some(stdlib_dir),
        }
    }

    /// Register a virtual module (for testing).
    pub fn register_virtual(&mut self, path: impl Into<PathBuf>, source: impl Into<String>) {
        self.virtual_modules.insert(path.into(), source.into());
    }

    /// Resolve an import path to an absolute path.
    fn resolve_path(&self, import_path: &str, from_file: &Path) -> Result<PathBuf, VMError> {
        if import_path.starts_with("std:") {
            // Standard library import
            let module_name = &import_path[4..];
            let stdlib = self.stdlib_dir.as_ref().ok_or_else(|| {
                VMError::runtime(format!(
                    "Cannot resolve stdlib module '{}': stdlib directory not found",
                    module_name
                ))
            })?;
            let mut path = stdlib.join(module_name);
            if path.extension().is_none() {
                path.set_extension("mar");
            }
            Ok(path)
        } else if import_path.starts_with("./") || import_path.starts_with("../") {
            // Relative import
            let dir = from_file.parent().unwrap_or(Path::new("."));
            let mut path = dir.join(import_path);
            if path.extension().is_none() {
                path.set_extension("mar");
            }
            // Canonicalize for deduplication (but don't fail on virtual modules)
            if let Ok(canonical) = path.canonicalize() {
                Ok(canonical)
            } else {
                Ok(path)
            }
        } else {
            // Bare path — treat as relative
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

    /// Load module source from filesystem or virtual modules.
    fn load_source(&self, path: &Path) -> Result<String, VMError> {
        if let Some(source) = self.virtual_modules.get(path) {
            return Ok(source.clone());
        }
        std::fs::read_to_string(path)
            .map_err(|_| VMError::runtime(format!("Module not found: {}", path.display())))
    }
}

// (no static builtin_names — we snapshot globals before/after module execution)

// --- run_program ---

/// Run a program expression-by-expression, handling imports inline.
/// This is the module-aware entry point for the VM.
pub fn run_program(
    vm: &mut VM,
    source: &str,
    file_path: Option<&Path>,
    loader: &mut ModuleLoader,
    interner: &Interner,
) -> Result<Value, VMError> {
    let tokens =
        lex(source, interner).map_err(|e| VMError::runtime(format!("Lex error: {}", e)))?;
    let exprs = parse(&tokens).map_err(|e| VMError::runtime(format!("Parse error: {}", e)))?;

    if exprs.is_empty() {
        return Ok(Value::nil());
    }

    let mut known_constructors: HashSet<Symbol> =
        ["None", "Some", "Nil", "Cons", "MkPair", "Ok", "Err"]
            .iter()
            .map(|s| interner.intern(s))
            .collect();

    let mut known_effect_ops: HashMap<Symbol, Symbol> = HashMap::new();
    let mut last_value = Value::nil();

    let import_sym = interner.intern("import");
    let as_sym = interner.intern("as");

    for expr in &exprs {
        // Check if this is an import expression
        if is_import(expr, import_sym) {
            let file = file_path
                .ok_or_else(|| VMError::runtime("Cannot use import without a file context"))?;
            handle_import(
                vm,
                expr,
                file,
                loader,
                interner,
                &mut known_constructors,
                &mut known_effect_ops,
                import_sym,
                as_sym,
            )?;
            last_value = Value::nil();
        } else {
            let result =
                compile_expr_with_context(expr, interner, &known_constructors, &known_effect_ops)
                    .map_err(|e| VMError::runtime(format!("Compile error: {}", e)))?;

            // Update knowledge from this expression
            known_constructors = result.known_constructors;
            known_effect_ops = result.known_effect_ops;

            // Execute
            last_value = interpret(vm, &result.chunk)?;
        }
    }

    Ok(last_value)
}

// --- Import handling ---

fn is_import(expr: &Expr, import_sym: Symbol) -> bool {
    if let Expr::List { .. } = expr {
        let elements = ast::list_to_array(expr);
        if let Some(first) = elements.first() {
            if let Expr::Atom(LispValue::Symbol(sym), _) = first {
                return *sym == import_sym;
            }
        }
    }
    false
}

fn parse_import_form(
    elements: &[&Expr],
    interner: &Interner,
    as_sym: Symbol,
) -> Result<(String, ImportForm), VMError> {
    // (import "path")                → All
    // (import "path" (name1 name2))  → Selective
    // (import "path" as alias)       → Qualified

    if elements.len() < 2 {
        return Err(VMError::runtime("import expects a module path"));
    }

    let path = match elements[1] {
        Expr::Atom(LispValue::String(s), _) => s.to_string(),
        _ => return Err(VMError::runtime("import expects a string path")),
    };

    if elements.len() == 2 {
        return Ok((path, ImportForm::All));
    }

    // Check for `as alias`
    if elements.len() == 4 {
        if let Expr::Atom(LispValue::Symbol(sym), _) = elements[2] {
            if *sym == as_sym {
                if let Expr::Atom(LispValue::Symbol(alias), _) = elements[3] {
                    let alias_name = interner.resolve(*alias).to_string();
                    return Ok((path, ImportForm::Qualified(alias_name)));
                }
            }
        }
    }

    // Check for selective: (import "path" (name1 name2 ...))
    if elements.len() == 3 {
        if let Expr::List { .. } = elements[2] {
            let names_exprs = ast::list_to_array(elements[2]);
            let mut names = Vec::new();
            for ne in &names_exprs {
                if let Expr::Atom(LispValue::Symbol(sym), _) = ne {
                    names.push(interner.resolve(*sym).to_string());
                } else {
                    return Err(VMError::runtime("Selective import expects symbols"));
                }
            }
            return Ok((path, ImportForm::Selective(names)));
        }
    }

    Err(VMError::runtime("Invalid import form"))
}

fn handle_import(
    vm: &mut VM,
    expr: &Expr,
    from_file: &Path,
    loader: &mut ModuleLoader,
    interner: &Interner,
    known_constructors: &mut HashSet<Symbol>,
    _known_effect_ops: &mut HashMap<Symbol, Symbol>,
    _import_sym: Symbol,
    as_sym: Symbol,
) -> Result<(), VMError> {
    let elements = ast::list_to_array(expr);
    let (path_str, form) = parse_import_form(&elements, interner, as_sym)?;

    // Resolve path
    let resolved = loader.resolve_path(&path_str, from_file)?;

    // Check cache
    if let Some(state) = loader.cache.get(&resolved) {
        match state {
            ModuleState::Loading => {
                return Err(VMError::runtime(format!(
                    "Circular dependency detected: {}",
                    resolved.display()
                )));
            }
            ModuleState::Loaded { globals } => {
                let globals = globals.clone();
                merge_exports(vm, &globals, &form, interner, known_constructors, &path_str)?;
                return Ok(());
            }
        }
    }

    // Mark as loading
    loader.cache.insert(resolved.clone(), ModuleState::Loading);

    // Load source
    let source = loader.load_source(&resolved)?;

    // Save parent globals and set up module globals (start with builtins)
    // Use std::mem::take to move globals instead of cloning (O(1) vs O(n))
    let parent_globals = std::mem::take(&mut vm.globals);
    let parent_handler_stack = std::mem::take(&mut vm.handler_stack);
    let parent_resumed = std::mem::take(&mut vm.resumed_continuations);
    let parent_dirty = vm.dirty_globals.take();

    // Globals is now empty, register builtins for the module
    register_builtins(vm);
    // Enable dirty-globals tracking for the module
    vm.dirty_globals = Some(HashSet::new());

    // Run the module
    let module_result = run_program(vm, &source, Some(&resolved), loader, interner);

    // Collect exports = globals that were explicitly written by module code
    let dirty = vm.dirty_globals.take().unwrap_or_default();
    let module_globals: HashMap<String, Value> = vm
        .globals
        .iter()
        .filter(|(name, _)| dirty.contains(name.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    // Restore parent state
    vm.globals = parent_globals;
    vm.handler_stack = parent_handler_stack;
    vm.resumed_continuations = parent_resumed;
    vm.dirty_globals = parent_dirty;

    // Check for module execution error
    module_result?;

    // Cache the module
    loader.cache.insert(
        resolved,
        ModuleState::Loaded {
            globals: module_globals.clone(),
        },
    );

    // Merge exports into current scope
    merge_exports(
        vm,
        &module_globals,
        &form,
        interner,
        known_constructors,
        &path_str,
    )?;

    Ok(())
}

fn merge_exports(
    vm: &mut VM,
    exports: &HashMap<String, Value>,
    form: &ImportForm,
    interner: &Interner,
    known_constructors: &mut HashSet<Symbol>,
    _path_str: &str,
) -> Result<(), VMError> {
    match form {
        ImportForm::All => {
            for (name, value) in exports {
                vm.globals.insert(name.clone(), value.clone());
                // Register constructors
                if is_constructor_value(value) {
                    known_constructors.insert(interner.intern(name));
                }
            }
        }
        ImportForm::Selective(names) => {
            for name in names {
                match exports.get(name) {
                    Some(value) => {
                        vm.globals.insert(name.clone(), value.clone());
                        if is_constructor_value(value) {
                            known_constructors.insert(interner.intern(name));
                        }
                    }
                    None => {
                        return Err(VMError::runtime(format!(
                            "Module does not export '{}'",
                            name
                        )));
                    }
                }
            }
        }
        ImportForm::Qualified(qualifier) => {
            for (name, value) in exports {
                let qualified_name = format!("{}/{}", qualifier, name);
                vm.globals.insert(qualified_name.clone(), value.clone());
                if is_constructor_value(value) {
                    known_constructors.insert(interner.intern(&qualified_name));
                }
            }
        }
    }
    Ok(())
}

fn is_constructor_value(value: &Value) -> bool {
    if let Some(obj) = value.as_object() {
        match obj {
            crate::object::VMObject::AdtConstructor { .. } => true,
            crate::object::VMObject::AdtValue { fields, .. } => fields.is_empty(),
            _ => false,
        }
    } else {
        false
    }
}
