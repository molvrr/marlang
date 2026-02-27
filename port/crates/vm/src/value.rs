use std::sync::Arc;

use crate::bytecode::ConstantValue;
use crate::object::VMObject;

// NaN-boxing constants
const QNAN: u64 = 0x7FFC_0000_0000_0000;
const SIGN_BIT: u64 = 0x8000_0000_0000_0000;
const TAG_NIL: u64 = 1;
const TAG_TRUE: u64 = 2;
const TAG_FALSE: u64 = 3;

/// A NaN-boxed value. Stores either:
/// - An f64 number (any bit pattern that is NOT a quiet NaN with our prefix)
/// - Nil, true, false (tagged singletons)
/// - A heap-allocated VMObject via Arc (pointer in low 48 bits + sign bit)
#[repr(transparent)]
pub struct Value(u64);

// SAFETY: Value is a NaN-boxed u64. Non-object values are trivially Send/Sync.
// Object values contain Arc<VMObject> — Arc<T> is Send+Sync when T: Send+Sync.
// VMObject variants are composed of Send+Sync types (String, Vec, Arc, fn pointers).
unsafe impl Send for Value {}
unsafe impl Sync for Value {}

impl Value {
    // --- Constructors ---

    #[inline]
    pub fn number(n: f64) -> Self {
        Value(n.to_bits())
    }

    #[inline]
    pub fn nil() -> Self {
        Value(QNAN | TAG_NIL)
    }

    #[inline]
    pub fn boolean(b: bool) -> Self {
        if b {
            Value(QNAN | TAG_TRUE)
        } else {
            Value(QNAN | TAG_FALSE)
        }
    }

    pub fn object(obj: VMObject) -> Self {
        let arc = Arc::new(obj);
        let ptr = Arc::into_raw(arc) as u64;
        debug_assert!(
            ptr & !(0x0000_FFFF_FFFF_FFFF) == 0,
            "pointer uses more than 48 bits"
        );
        Value(SIGN_BIT | QNAN | ptr)
    }

    pub fn string(s: impl Into<Arc<str>>) -> Self {
        Value::object(VMObject::String(s.into()))
    }

    pub fn symbol(name: impl Into<String>) -> Self {
        Value::object(VMObject::Symbol(name.into()))
    }

    // --- Type checks ---

    #[inline]
    pub fn is_number(&self) -> bool {
        (self.0 & QNAN) != QNAN
    }

    #[inline]
    pub fn is_nil(&self) -> bool {
        self.0 == (QNAN | TAG_NIL)
    }

    #[inline]
    pub fn is_true(&self) -> bool {
        self.0 == (QNAN | TAG_TRUE)
    }

    #[inline]
    pub fn is_false(&self) -> bool {
        self.0 == (QNAN | TAG_FALSE)
    }

    #[inline]
    pub fn is_bool(&self) -> bool {
        self.is_true() || self.is_false()
    }

    #[inline]
    pub fn is_object(&self) -> bool {
        (self.0 & (SIGN_BIT | QNAN)) == (SIGN_BIT | QNAN)
    }

    // --- Extractors ---

    #[inline]
    pub fn as_number(&self) -> Option<f64> {
        if self.is_number() {
            Some(f64::from_bits(self.0))
        } else {
            None
        }
    }

    #[inline]
    pub fn as_bool(&self) -> Option<bool> {
        if self.is_true() {
            Some(true)
        } else if self.is_false() {
            Some(false)
        } else {
            None
        }
    }

    pub fn as_object(&self) -> Option<&VMObject> {
        if self.is_object() {
            let ptr = (self.0 & 0x0000_FFFF_FFFF_FFFF) as *const VMObject;
            // Safety: we created this pointer via Arc::into_raw in Value::object,
            // and we maintain the Arc refcount via Clone/Drop.
            unsafe { Some(&*ptr) }
        } else {
            None
        }
    }

    pub fn as_string(&self) -> Option<&str> {
        self.as_object().and_then(|obj| match obj {
            VMObject::String(s) => Some(s.as_ref()),
            _ => None,
        })
    }

    // --- Truthiness ---

    /// Only nil and false are falsy, everything else is truthy.
    #[inline]
    pub fn is_truthy(&self) -> bool {
        !self.is_nil() && !self.is_false()
    }

    /// Get kind name for error messages.
    pub fn kind_name(&self) -> &'static str {
        if self.is_number() {
            "number"
        } else if self.is_nil() {
            "nil"
        } else if self.is_bool() {
            "boolean"
        } else if let Some(obj) = self.as_object() {
            obj.kind_name()
        } else {
            "unknown"
        }
    }

    /// Raw bits (for testing/debugging).
    #[inline]
    pub fn bits(&self) -> u64 {
        self.0
    }
}

// --- Clone: increment Arc refcount for objects ---

impl Clone for Value {
    fn clone(&self) -> Self {
        if self.is_object() {
            let ptr = (self.0 & 0x0000_FFFF_FFFF_FFFF) as *const VMObject;
            // Safety: ptr came from Arc::into_raw. We increment the refcount.
            unsafe {
                Arc::increment_strong_count(ptr);
            }
        }
        Value(self.0)
    }
}

// --- Drop: decrement Arc refcount for objects ---

impl Drop for Value {
    fn drop(&mut self) {
        if self.is_object() {
            let ptr = (self.0 & 0x0000_FFFF_FFFF_FFFF) as *const VMObject;
            // Safety: ptr came from Arc::into_raw. We decrement the refcount,
            // potentially freeing the object.
            unsafe {
                Arc::decrement_strong_count(ptr);
            }
        }
    }
}

// --- PartialEq ---

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        // Fast path: identical bits
        if self.0 == other.0 {
            return true;
        }
        // Number comparison (handles NaN != NaN correctly via bit equality above)
        if self.is_number() && other.is_number() {
            return f64::from_bits(self.0) == f64::from_bits(other.0);
        }
        // Object comparison — delegate to VMObject::PartialEq
        if let (Some(a), Some(b)) = (self.as_object(), other.as_object()) {
            return a == b;
        }
        false
    }
}

// --- Debug ---

impl std::fmt::Debug for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_number() {
            write!(f, "Value({})", f64::from_bits(self.0))
        } else if self.is_nil() {
            write!(f, "Value(nil)")
        } else if self.is_true() {
            write!(f, "Value(true)")
        } else if self.is_false() {
            write!(f, "Value(false)")
        } else if let Some(obj) = self.as_object() {
            write!(f, "Value({:?})", obj)
        } else {
            write!(f, "Value(unknown:0x{:016x})", self.0)
        }
    }
}

// --- Display formatting ---

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", format_value(self))
    }
}

/// Format a VM value for display (ports `formatVMValue` from TypeScript).
pub fn format_value(value: &Value) -> String {
    if let Some(n) = value.as_number() {
        if n == n.trunc() && n.abs() < 1e15 && !n.is_infinite() {
            return format!("{}", n as i64);
        }
        return format!("{}", n);
    }
    if value.is_nil() {
        return "nil".to_string();
    }
    if value.is_true() {
        return "#t".to_string();
    }
    if value.is_false() {
        return "#f".to_string();
    }
    if let Some(obj) = value.as_object() {
        return format_object(obj);
    }
    "unknown".to_string()
}

fn format_object(obj: &VMObject) -> String {
    match obj {
        VMObject::String(s) => format!("\"{}\"", s),
        VMObject::Symbol(s) => s.clone(),
        VMObject::List(elements) => {
            let parts: Vec<String> = elements.iter().map(format_value).collect();
            format!("({})", parts.join(" "))
        }
        VMObject::AdtValue { tag, fields } => {
            if fields.is_empty() {
                tag.clone()
            } else {
                let parts: Vec<String> = fields.iter().map(format_value).collect();
                format!("({} {})", tag, parts.join(" "))
            }
        }
        VMObject::AdtConstructor { name, arity } => {
            format!("#<constructor {}/{}>", name, arity)
        }
        VMObject::Closure(c) => {
            format!("#<vm-closure {}>", c.proto.name)
        }
        VMObject::Continuation(_) => "#<continuation>".to_string(),
        VMObject::Builtin { name, .. } => {
            format!("#<vm-builtin {}>", name)
        }
        VMObject::Pid(id) => format!("#<pid {}>", id),
        VMObject::Port(id) => format!("#<port {}>", id),
        #[cfg(feature = "ffi")]
        VMObject::FfiLibrary(lib) => format!("#<ffi-library {}>", lib.path),
        #[cfg(feature = "ffi")]
        VMObject::FfiPointer(addr) => format!("#<ffi-pointer 0x{:x}>", addr),
        #[cfg(feature = "ffi")]
        VMObject::FfiStructDef(def) => format!("#<ffi-struct-def {}>", def.name),
        #[cfg(feature = "ffi")]
        VMObject::FfiStruct(s) => format!("#<ffi-struct {}>", s.def.name),
        #[cfg(feature = "ffi")]
        VMObject::FfiBound {
            lib_global_name,
            fn_name,
        } => {
            format!("#<ffi-bound {}/{}>", lib_global_name, fn_name)
        }
    }
}

/// Format a VM value for display without quotes around strings.
/// Used by `show` and IO handlers.
pub fn format_value_unquoted(value: &Value) -> String {
    if let Some(s) = value.as_string() {
        return s.to_string();
    }
    format_value(value)
}

// --- Constant pool conversion ---

/// Pre-convert a compile-time constant pool to NaN-boxed Values.
pub fn preconvert_constants(constants: &[ConstantValue]) -> Vec<Value> {
    constants
        .iter()
        .map(|c| match c {
            ConstantValue::Number(n) => Value::number(*n),
            ConstantValue::String(s) => Value::string(s.as_str()),
            ConstantValue::Boolean(b) => Value::boolean(*b),
            ConstantValue::Nil => Value::nil(),
            ConstantValue::Prototype(_) => Value::nil(), // placeholder
            ConstantValue::FfiSigs(_) | ConstantValue::FfiBound { .. } => c.as_value(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_roundtrip() {
        let v = Value::number(42.0);
        assert!(v.is_number());
        assert_eq!(v.as_number(), Some(42.0));
        assert!(!v.is_nil());
        assert!(!v.is_object());
    }

    #[test]
    fn nil_value() {
        let v = Value::nil();
        assert!(v.is_nil());
        assert!(!v.is_number());
        assert!(!v.is_truthy());
    }

    #[test]
    fn boolean_values() {
        let t = Value::boolean(true);
        let f = Value::boolean(false);
        assert!(t.is_true());
        assert!(t.is_truthy());
        assert!(f.is_false());
        assert!(!f.is_truthy());
        assert_eq!(t.as_bool(), Some(true));
        assert_eq!(f.as_bool(), Some(false));
    }

    #[test]
    fn string_object() {
        let v = Value::string("hello");
        assert!(v.is_object());
        assert_eq!(v.as_string(), Some("hello"));
        assert!(v.is_truthy());
    }

    #[test]
    fn object_clone_drop() {
        let v = Value::string("test");
        let v2 = v.clone();
        assert_eq!(v.as_string(), Some("test"));
        assert_eq!(v2.as_string(), Some("test"));
        drop(v);
        assert_eq!(v2.as_string(), Some("test"));
    }

    #[test]
    fn equality() {
        assert_eq!(Value::number(1.0), Value::number(1.0));
        assert_ne!(Value::number(1.0), Value::number(2.0));
        assert_eq!(Value::nil(), Value::nil());
        assert_eq!(Value::boolean(true), Value::boolean(true));
        assert_ne!(Value::boolean(true), Value::boolean(false));
        assert_ne!(Value::nil(), Value::number(0.0));
        assert_eq!(Value::string("a"), Value::string("a"));
        assert_ne!(Value::string("a"), Value::string("b"));
    }

    #[test]
    fn negative_zero() {
        // -0.0 and 0.0 have different bit patterns but compare equal
        let a = Value::number(0.0);
        let b = Value::number(-0.0);
        assert_eq!(a, b);
    }

    #[test]
    fn preconvert() {
        let constants = vec![
            ConstantValue::Number(3.14),
            ConstantValue::String("hi".to_string()),
            ConstantValue::Boolean(true),
            ConstantValue::Nil,
        ];
        let values = preconvert_constants(&constants);
        assert_eq!(values.len(), 4);
        assert_eq!(values[0].as_number(), Some(3.14));
        assert_eq!(values[1].as_string(), Some("hi"));
        assert!(values[2].is_true());
        assert!(values[3].is_nil());
    }
}
