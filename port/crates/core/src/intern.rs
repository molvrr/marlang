use std::collections::HashMap;
use std::sync::RwLock;

/// Interned string handle — 4 bytes, Copy, Eq, Hash.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Symbol(pub u32);

/// Thread-safe global interner.
/// Strings are leaked to 'static — safe because interning is append-only
/// and the interner lives for the process lifetime.
pub struct Interner {
    map: RwLock<HashMap<&'static str, Symbol>>,
    strings: RwLock<Vec<&'static str>>,
}

impl Interner {
    pub fn new() -> Self {
        Interner {
            map: RwLock::new(HashMap::new()),
            strings: RwLock::new(Vec::new()),
        }
    }

    /// Intern a string. Takes write lock only if the string is new.
    pub fn intern(&self, s: &str) -> Symbol {
        // Fast path: read lock
        if let Some(&sym) = self.map.read().unwrap().get(s) {
            return sym;
        }
        // Slow path: write lock, double-check
        let mut map = self.map.write().unwrap();
        if let Some(&sym) = map.get(s) {
            return sym;
        }
        let mut strings = self.strings.write().unwrap();
        let leaked: &'static str = Box::leak(s.to_string().into_boxed_str());
        let sym = Symbol(strings.len() as u32);
        strings.push(leaked);
        map.insert(leaked, sym);
        sym
    }

    /// Resolve a symbol to its string.
    pub fn resolve(&self, sym: Symbol) -> &str {
        self.strings.read().unwrap()[sym.0 as usize]
    }
}

impl Default for Interner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intern_and_resolve() {
        let interner = Interner::new();
        let s1 = interner.intern("hello");
        let s2 = interner.intern("hello");
        assert_eq!(s1, s2);
        assert_eq!(interner.resolve(s1), "hello");
    }

    #[test]
    fn different_strings_different_symbols() {
        let interner = Interner::new();
        let a = interner.intern("a");
        let b = interner.intern("b");
        assert_ne!(a, b);
    }
}
