use crate::bytecode::ConstantValue;
use crate::object::VMObject;
use std::sync::Arc;

// Tag constants for the compact tagged union.
const TAG_INT: u8 = 0;
const TAG_FLOAT: u8 = 1;
const TAG_NIL: u8 = 2;
const TAG_TRUE: u8 = 3;
const TAG_FALSE: u8 = 4;
const TAG_CHAR: u8 = 5;
const TAG_OBJECT: u8 = 6;

/// A compact tagged-union value. 16 bytes: u8 tag + 7 bytes padding + u64 payload.
///
/// Stores either:
/// - An i64 integer (tag=0, payload = i64 as u64)
/// - An f64 float (tag=1, payload = f64 bits as u64)
/// - Nil (tag=2)
/// - Boolean true/false (tag=3 or 4)
/// - A Unicode char (tag=5, payload = char as u32 zero-extended)
/// - A heap-allocated VMObject via Arc (tag=6, payload = Arc raw pointer)
#[repr(C)]
pub struct Value {
    tag: u8,
    _pad: [u8; 7],
    payload: u64,
}

// SAFETY: Non-object values are trivially Send/Sync.
// Object values contain Arc<VMObject> — Arc<T> is Send+Sync when T: Send+Sync.
// VMObject variants are composed of Send+Sync types (String, Vec, Arc, fn pointers).
unsafe impl Send for Value {}
unsafe impl Sync for Value {}

impl Value {
    // --- Constructors ---

    #[inline]
    pub fn int(n: i64) -> Self {
        Value {
            tag: TAG_INT,
            _pad: [0; 7],
            payload: n as u64,
        }
    }

    #[inline]
    pub fn float(n: f64) -> Self {
        Value {
            tag: TAG_FLOAT,
            _pad: [0; 7],
            payload: n.to_bits(),
        }
    }

    #[inline]
    pub fn nil() -> Self {
        Value {
            tag: TAG_NIL,
            _pad: [0; 7],
            payload: 0,
        }
    }

    #[inline]
    pub fn boolean(b: bool) -> Self {
        Value {
            tag: if b { TAG_TRUE } else { TAG_FALSE },
            _pad: [0; 7],
            payload: 0,
        }
    }

    pub fn object(obj: VMObject) -> Self {
        let arc = Arc::new(obj);
        let ptr = Arc::into_raw(arc) as u64;
        Value {
            tag: TAG_OBJECT,
            _pad: [0; 7],
            payload: ptr,
        }
    }

    pub fn string(s: impl Into<Arc<str>>) -> Self {
        Value::object(VMObject::String(s.into()))
    }

    pub fn symbol(name: impl Into<String>) -> Self {
        Value::object(VMObject::Symbol(name.into()))
    }

    pub fn bytes(data: Vec<u8>) -> Self {
        Value::object(VMObject::ByteArray(data))
    }

    /// Create a Char value (inline, no heap allocation).
    #[inline]
    pub fn char(c: char) -> Self {
        Value {
            tag: TAG_CHAR,
            _pad: [0; 7],
            payload: c as u64,
        }
    }

    // --- Type checks ---

    #[inline]
    pub fn is_int(&self) -> bool {
        self.tag == TAG_INT
    }

    #[inline]
    pub fn is_float(&self) -> bool {
        self.tag == TAG_FLOAT
    }

    #[inline]
    pub fn is_number(&self) -> bool {
        self.tag == TAG_INT || self.tag == TAG_FLOAT
    }

    #[inline]
    pub fn is_nil(&self) -> bool {
        self.tag == TAG_NIL
    }

    #[inline]
    pub fn is_true(&self) -> bool {
        self.tag == TAG_TRUE
    }

    #[inline]
    pub fn is_false(&self) -> bool {
        self.tag == TAG_FALSE
    }

    #[inline]
    pub fn is_bool(&self) -> bool {
        self.tag == TAG_TRUE || self.tag == TAG_FALSE
    }

    #[inline]
    pub fn is_char(&self) -> bool {
        self.tag == TAG_CHAR
    }

    #[inline]
    pub fn is_object(&self) -> bool {
        self.tag == TAG_OBJECT
    }

    // --- Extractors ---

    #[inline]
    pub fn as_int(&self) -> Option<i64> {
        if self.tag == TAG_INT {
            Some(self.payload as i64)
        } else {
            None
        }
    }

    #[inline]
    pub fn as_float(&self) -> Option<f64> {
        if self.tag == TAG_FLOAT {
            Some(f64::from_bits(self.payload))
        } else {
            None
        }
    }

    /// Extract as f64 regardless of whether the value is Int or Float.
    /// Useful for builtins that accept either numeric type.
    #[inline]
    pub fn as_number_f64(&self) -> Option<f64> {
        match self.tag {
            TAG_INT => Some(self.payload as i64 as f64),
            TAG_FLOAT => Some(f64::from_bits(self.payload)),
            _ => None,
        }
    }

    // --- Unchecked extractors (zero overhead in release) ---
    // The type checker guarantees these are only called on the correct type.
    // In debug builds, the assertion catches VM/compiler bugs.

    /// Extract i64 without runtime check. Panics in debug if not an Int.
    #[inline(always)]
    pub fn get_int(&self) -> i64 {
        debug_assert!(self.tag == TAG_INT, "get_int called on non-Int value");
        self.payload as i64
    }

    /// Extract f64 without runtime check. Panics in debug if not a Float.
    #[inline(always)]
    pub fn get_float(&self) -> f64 {
        debug_assert!(self.tag == TAG_FLOAT, "get_float called on non-Float value");
        f64::from_bits(self.payload)
    }

    #[inline]
    pub fn as_bool(&self) -> Option<bool> {
        match self.tag {
            TAG_TRUE => Some(true),
            TAG_FALSE => Some(false),
            _ => None,
        }
    }

    #[inline]
    pub fn as_char(&self) -> Option<char> {
        if self.tag == TAG_CHAR {
            char::from_u32(self.payload as u32)
        } else {
            None
        }
    }

    pub fn as_object(&self) -> Option<&VMObject> {
        if self.tag == TAG_OBJECT {
            let ptr = self.payload as *const VMObject;
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

    pub fn as_bytes(&self) -> Option<&[u8]> {
        self.as_object().and_then(|obj| match obj {
            VMObject::ByteArray(b) => Some(b.as_slice()),
            _ => None,
        })
    }

    /// Attempt to take ownership of the inner VMObject by consuming this Value.
    /// Succeeds if this Value holds the only Arc reference (refcount == 1).
    /// On failure, returns the Value back unchanged.
    ///
    /// This is used to avoid deep-cloning one-shot continuations on resume:
    /// if we're the sole owner, we can move the data instead of copying it.
    pub(crate) fn try_take_object(self) -> Result<VMObject, Value> {
        if self.tag != TAG_OBJECT {
            return Err(self);
        }
        let ptr = self.payload as *const VMObject;
        // Reconstruct the Arc without incrementing refcount —
        // Arc::from_raw reclaims the Arc that was leaked by Arc::into_raw
        // in Value::object(). We must prevent our Drop impl from also
        // decrementing the refcount, so we forget self.
        let tag = self.tag;
        let payload = self.payload;
        std::mem::forget(self);
        let arc = unsafe { Arc::from_raw(ptr) };
        match Arc::try_unwrap(arc) {
            Ok(obj) => Ok(obj),
            Err(arc) => {
                // Put the Arc back — into_raw leaks it again so the
                // refcount stays balanced with the Value's Drop.
                let _ = Arc::into_raw(arc);
                Err(Value {
                    tag,
                    _pad: [0; 7],
                    payload,
                })
            }
        }
    }

    // --- Truthiness ---

    /// Only nil and false are falsy, everything else is truthy.
    #[inline]
    pub fn is_truthy(&self) -> bool {
        self.tag != TAG_NIL && self.tag != TAG_FALSE
    }

    /// Get kind name for error messages.
    pub fn kind_name(&self) -> &'static str {
        match self.tag {
            TAG_INT => "int",
            TAG_FLOAT => "float",
            TAG_NIL => "nil",
            TAG_TRUE | TAG_FALSE => "boolean",
            TAG_CHAR => "char",
            TAG_OBJECT => {
                if let Some(obj) = self.as_object() {
                    obj.kind_name()
                } else {
                    "object"
                }
            }
            _ => "unknown",
        }
    }

    /// Raw tag (for testing/debugging and JIT).
    #[inline]
    pub fn tag(&self) -> u8 {
        self.tag
    }

    /// Raw payload (for JIT).
    #[inline]
    pub fn payload(&self) -> u64 {
        self.payload
    }

    /// Construct a Value from raw tag and payload, taking ownership.
    /// The caller must ensure that if tag == TAG_OBJECT, the payload
    /// is a valid Arc<VMObject> raw pointer with an owned reference
    /// (i.e., the caller has transferred ownership and will not drop it).
    ///
    /// # Safety
    /// For object values (tag == 6), the payload must be a valid pointer
    /// obtained from `Arc::into_raw` with a live reference count.
    #[inline]
    pub unsafe fn from_raw(tag: u8, payload: u64) -> Self {
        Value {
            tag,
            _pad: [0; 7],
            payload,
        }
    }

    /// Construct a Value from raw tag and payload, cloning (incrementing
    /// refcount for objects). Use this when the caller retains ownership
    /// of the original.
    ///
    /// # Safety
    /// For object values (tag == 6), the payload must be a valid pointer
    /// obtained from `Arc::into_raw` with a live reference count.
    #[inline]
    pub unsafe fn from_raw_clone(tag: u8, payload: u64) -> Self {
        if tag == TAG_OBJECT {
            let ptr = payload as *const VMObject;
            unsafe { Arc::increment_strong_count(ptr) };
        }
        Value {
            tag,
            _pad: [0; 7],
            payload,
        }
    }
}

// --- Clone: increment Arc refcount for objects ---

impl Clone for Value {
    fn clone(&self) -> Self {
        if self.tag == TAG_OBJECT {
            let ptr = self.payload as *const VMObject;
            // Safety: ptr came from Arc::into_raw. We increment the refcount.
            unsafe {
                Arc::increment_strong_count(ptr);
            }
        }
        Value {
            tag: self.tag,
            _pad: [0; 7],
            payload: self.payload,
        }
    }
}

// --- Drop: decrement Arc refcount for objects ---

impl Drop for Value {
    fn drop(&mut self) {
        if self.tag != TAG_OBJECT {
            return;
        }

        // Iteratively drop Cons chains to avoid stack overflow on long lists.
        // A Cons list like (Cons a (Cons b (Cons c Nil))) would normally
        // recurse through Drop: dropping the outer Cons drops its tail field,
        // which drops the next Cons, etc. For 10K+ elements this overflows.
        //
        // Instead, we walk the chain iteratively: if this Value is the sole
        // owner of a Cons, we extract the tail before dropping the node,
        // then continue with the tail.
        let ptr = self.payload as *const VMObject;
        // Reconstruct the Arc from the raw pointer
        let mut arc = unsafe { Arc::from_raw(ptr) };
        // Clear self to prevent double-free from the implicit drop
        self.tag = TAG_NIL;
        self.payload = 0;

        loop {
            // Try to take sole ownership. If other references exist, just drop
            // our Arc reference (decrement refcount) and stop.
            let mut obj = match Arc::try_unwrap(arc) {
                Ok(obj) => obj,
                Err(_shared_arc) => break, // drop _shared_arc decrements refcount
            };

            // We have sole ownership. Check if it's a Cons node.
            match &mut obj {
                VMObject::AdtValue { tag, fields } if tag == "Cons" && fields.len() == 2 => {
                    // Take the tail value before dropping the node.
                    // Replace the tail with Nil so it doesn't get recursively dropped.
                    let tail = std::mem::replace(&mut fields[1], Value::nil());
                    // Drop the node (drops the head field, the Nil tail, and the tag string)
                    drop(obj);

                    if tail.tag == TAG_OBJECT {
                        // Continue with the tail
                        let tail_ptr = tail.payload as *const VMObject;
                        arc = unsafe { Arc::from_raw(tail_ptr) };
                        // Prevent tail's Drop from also decrementing the refcount
                        std::mem::forget(tail);
                    } else {
                        // Tail is not an object (e.g., Nil) — nothing more to iterate
                        break;
                    }
                }
                _ => {
                    // Not a Cons chain — let normal Drop handle it
                    drop(obj);
                    break;
                }
            }
        }
    }
}

// --- PartialEq ---

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        if self.tag != other.tag {
            return false;
        }
        match self.tag {
            TAG_INT => self.payload == other.payload,
            TAG_FLOAT => f64::from_bits(self.payload) == f64::from_bits(other.payload),
            TAG_NIL => true,
            TAG_TRUE | TAG_FALSE => true, // same tag ⇒ same bool
            TAG_CHAR => self.payload == other.payload,
            TAG_OBJECT => {
                // Fast path: same pointer
                if self.payload == other.payload {
                    return true;
                }
                // Delegate to VMObject::PartialEq
                if let (Some(a), Some(b)) = (self.as_object(), other.as_object()) {
                    return a == b;
                }
                false
            }
            _ => false,
        }
    }
}

// --- Debug ---

impl std::fmt::Debug for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.tag {
            TAG_INT => write!(f, "Value({})", self.payload as i64),
            TAG_FLOAT => write!(f, "Value({})", f64::from_bits(self.payload)),
            TAG_NIL => write!(f, "Value(nil)"),
            TAG_TRUE => write!(f, "Value(true)"),
            TAG_FALSE => write!(f, "Value(false)"),
            TAG_CHAR => {
                if let Some(c) = self.as_char() {
                    write!(f, "Value('{}')", c)
                } else {
                    write!(f, "Value(char:invalid)")
                }
            }
            TAG_OBJECT => {
                if let Some(obj) = self.as_object() {
                    write!(f, "Value({:?})", obj)
                } else {
                    write!(f, "Value(object:invalid)")
                }
            }
            _ => write!(f, "Value(unknown:tag={})", self.tag),
        }
    }
}

// --- Display formatting ---

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", format_value(self))
    }
}

/// Format a VM value for display.
pub fn format_value(value: &Value) -> String {
    if let Some(n) = value.as_int() {
        return format!("{}", n);
    }
    if let Some(n) = value.as_float() {
        // Always show decimal point for floats to distinguish from ints
        if n == n.trunc() && !n.is_infinite() && !n.is_nan() {
            return format!("{:.1}", n);
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
    if let Some(c) = value.as_char() {
        return format!("'{}'", c);
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
        VMObject::AdtValue { tag, fields } => {
            // Detect Cons/Nil chains and pretty-print as flat lists: (1 2 3)
            if tag == "Cons" && fields.len() == 2 {
                let mut parts = Vec::new();
                parts.push(format_value(&fields[0]));
                let mut cur = &fields[1];
                loop {
                    match cur.as_object() {
                        Some(VMObject::AdtValue { tag, fields })
                            if tag == "Nil" && fields.is_empty() =>
                        {
                            break;
                        }
                        Some(VMObject::AdtValue { tag, fields })
                            if tag == "Cons" && fields.len() == 2 =>
                        {
                            parts.push(format_value(&fields[0]));
                            cur = &fields[1];
                        }
                        _ => {
                            // Improper list: (1 2 . tail)
                            parts.push(".".to_string());
                            parts.push(format_value(cur));
                            break;
                        }
                    }
                }
                format!("({})", parts.join(" "))
            } else if tag == "Nil" && fields.is_empty() {
                "()".to_string()
            } else if fields.is_empty() {
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
        VMObject::MutableBox(cell) => {
            let inner = cell.lock().unwrap();
            format!("#<box {}>", format_value(&inner))
        }
        VMObject::Builtin { name, .. } => {
            format!("#<vm-builtin {}>", name)
        }
        VMObject::Pid(id) => format!("#<pid {}>", id),
        VMObject::Port(id) => format!("#<port {}>", id),
        VMObject::ByteArray(bytes) => {
            if bytes.len() <= 32 {
                // Show hex for small byte arrays
                let hex: Vec<String> = bytes.iter().map(|b| format!("{:02x}", b)).collect();
                format!("#<bytes {} [{}]>", bytes.len(), hex.join(" "))
            } else {
                format!("#<bytes {}>", bytes.len())
            }
        }
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
    if let Some(c) = value.as_char() {
        return c.to_string();
    }
    format_value(value)
}

// --- Constant pool conversion ---

/// Pre-convert a compile-time constant pool to runtime Values.
pub fn preconvert_constants(constants: &[ConstantValue]) -> Vec<Value> {
    constants
        .iter()
        .map(|c| match c {
            ConstantValue::Int(n) => Value::int(*n),
            ConstantValue::Float(n) => Value::float(*n),
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
    fn int_roundtrip() {
        let v = Value::int(42);
        assert!(v.is_int());
        assert!(!v.is_float());
        assert!(v.is_number());
        assert_eq!(v.as_int(), Some(42));
        assert_eq!(v.as_number_f64(), Some(42.0));
        assert!(!v.is_nil());
        assert!(!v.is_object());
    }

    #[test]
    fn float_roundtrip() {
        let v = Value::float(3.14);
        assert!(v.is_float());
        assert!(!v.is_int());
        assert!(v.is_number());
        assert_eq!(v.as_float(), Some(3.14));
        assert_eq!(v.as_number_f64(), Some(3.14));
    }

    #[test]
    fn int_negative() {
        let v = Value::int(-100);
        assert_eq!(v.as_int(), Some(-100));
    }

    #[test]
    fn int_extremes() {
        let v_max = Value::int(i64::MAX);
        assert_eq!(v_max.as_int(), Some(i64::MAX));
        let v_min = Value::int(i64::MIN);
        assert_eq!(v_min.as_int(), Some(i64::MIN));
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
        assert_eq!(Value::int(1), Value::int(1));
        assert_ne!(Value::int(1), Value::int(2));
        assert_eq!(Value::float(1.0), Value::float(1.0));
        assert_ne!(Value::float(1.0), Value::float(2.0));
        // Int and Float are never equal
        assert_ne!(Value::int(1), Value::float(1.0));
        assert_eq!(Value::nil(), Value::nil());
        assert_eq!(Value::boolean(true), Value::boolean(true));
        assert_ne!(Value::boolean(true), Value::boolean(false));
        assert_ne!(Value::nil(), Value::int(0));
        assert_eq!(Value::string("a"), Value::string("a"));
        assert_ne!(Value::string("a"), Value::string("b"));
    }

    #[test]
    fn negative_zero_float() {
        // -0.0 and 0.0 compare equal for floats (IEEE 754 semantics)
        let a = Value::float(0.0);
        let b = Value::float(-0.0);
        assert_eq!(a, b);
    }

    #[test]
    fn char_value() {
        let v = Value::char('A');
        assert!(v.is_char());
        assert_eq!(v.as_char(), Some('A'));
        assert!(v.is_truthy());
    }

    #[test]
    fn preconvert() {
        let constants = vec![
            ConstantValue::Int(42),
            ConstantValue::Float(3.14),
            ConstantValue::String("hi".to_string()),
            ConstantValue::Boolean(true),
            ConstantValue::Nil,
        ];
        let values = preconvert_constants(&constants);
        assert_eq!(values.len(), 5);
        assert_eq!(values[0].as_int(), Some(42));
        assert_eq!(values[1].as_float(), Some(3.14));
        assert_eq!(values[2].as_string(), Some("hi"));
        assert!(values[3].is_true());
        assert!(values[4].is_nil());
    }

    #[test]
    fn format_int_display() {
        assert_eq!(format_value(&Value::int(42)), "42");
        assert_eq!(format_value(&Value::int(-7)), "-7");
        assert_eq!(format_value(&Value::int(0)), "0");
    }

    #[test]
    fn format_float_display() {
        assert_eq!(format_value(&Value::float(3.14)), "3.14");
        // Whole floats show decimal point
        assert_eq!(format_value(&Value::float(42.0)), "42.0");
        assert_eq!(format_value(&Value::float(0.0)), "0.0");
    }
}
