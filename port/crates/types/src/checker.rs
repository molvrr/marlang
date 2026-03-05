use crate::ast::{Type, reset_effect_row_var_counter, reset_type_var_counter};
use crate::env::{TypeEnv, empty_env};
use crate::errors::TypeError;
use crate::infer::{infer_program, infer_program_with_context};
use marlang_core::ast::Expr;
use marlang_core::intern::Interner;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ===== Module Loading Trait =====

/// Trait for resolving and loading modules during type checking.
/// Implemented by the CLI crate which has access to the VM for macro expansion.
pub trait TypeModuleLoader {
    /// Resolve an import path, load source, lex/parse/expand macros,
    /// and return the module expressions (without prelude) and the resolved path.
    fn resolve_and_load(
        &self,
        import_path: &str,
        from_file: Option<&Path>,
        interner: &Interner,
    ) -> Result<(Vec<Expr>, PathBuf), TypeError>;
}

// ===== Module Cache =====

enum ModuleCacheEntry {
    Loading,
    Loaded(Box<TypeEnv>),
}

/// Cache for type-checked modules, preventing duplicate work and detecting cycles.
pub struct TypeModuleCache {
    entries: RefCell<HashMap<PathBuf, ModuleCacheEntry>>,
}

impl Default for TypeModuleCache {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeModuleCache {
    pub fn new() -> Self {
        TypeModuleCache {
            entries: RefCell::new(HashMap::new()),
        }
    }

    pub fn get_state(&self, path: &Path) -> Option<ModuleCacheState> {
        let entries = self.entries.borrow();
        match entries.get(path) {
            None => None,
            Some(ModuleCacheEntry::Loading) => Some(ModuleCacheState::Loading),
            Some(ModuleCacheEntry::Loaded(env)) => Some(ModuleCacheState::Loaded(env.clone())),
        }
    }

    pub fn mark_loading(&self, path: PathBuf) {
        self.entries
            .borrow_mut()
            .insert(path, ModuleCacheEntry::Loading);
    }

    pub fn mark_loaded(&self, path: PathBuf, env: TypeEnv) {
        self.entries
            .borrow_mut()
            .insert(path, ModuleCacheEntry::Loaded(Box::new(env)));
    }
}

pub enum ModuleCacheState {
    Loading,
    Loaded(Box<TypeEnv>),
}

// ===== Type Check Entry Points =====

pub struct TypeCheckOutput {
    pub env: TypeEnv,
    pub types: Vec<Type>,
}

/// Type check without module support (backward compatible).
///
/// `exprs` should include prelude expressions prepended before user code.
/// All type declarations (data types, builtins, effects) are defined in
/// `stdlib/prelude.mar`; the initial env is empty.
pub fn type_check(exprs: &[Expr], interner: &Interner) -> Result<TypeCheckOutput, TypeError> {
    reset_type_var_counter();
    reset_effect_row_var_counter();

    let env = empty_env();
    let (env, types) = infer_program(&env, exprs, interner)?;
    Ok(TypeCheckOutput { env, types })
}

/// Type check with module/import support.
///
/// `exprs` should include prelude expressions prepended before user code.
/// All type declarations (data types, builtins, effects) are defined in
/// `stdlib/prelude.mar`; the initial env is empty.
pub fn type_check_with_modules(
    exprs: &[Expr],
    interner: &Interner,
    file_path: Option<&Path>,
    loader: &dyn TypeModuleLoader,
) -> Result<TypeCheckOutput, TypeError> {
    reset_type_var_counter();
    reset_effect_row_var_counter();

    let env = empty_env();
    let cache = TypeModuleCache::new();
    let (env, types) =
        infer_program_with_context(&env, exprs, interner, file_path, loader, &cache)?;
    Ok(TypeCheckOutput { env, types })
}
