//! Runtime helper functions callable from JIT-compiled code.
//!
//! These are `extern "C"` functions registered as symbols in the Cranelift JIT module.
//! They provide the bridge between native code and the VM for operations that are
//! too complex or polymorphic to inline (ADT construction, global lookup, builtins,
//! effect handling, refcounting, etc.).

use crate::object::{Closure, VMObject};
use crate::value::Value;
use crate::vm::VM;
use std::sync::Arc;

// =============================================================================
// Value construction helpers
// =============================================================================

/// Construct a Value from raw tag + payload (called by JIT return sequences).
#[unsafe(no_mangle)]
pub extern "C" fn marlang_jit_value_from_raw(tag: u8, payload: u64) -> u64 {
    // This is just for documentation — the JIT constructs Values inline.
    // We keep it here as a potential fallback.
    let _ = (tag, payload);
    0
}

/// Clone a Value (increment Arc refcount if it's an object).
/// `src` points to a 16-byte Value in memory.
#[unsafe(no_mangle)]
pub extern "C" fn marlang_jit_value_clone(src: *const Value) {
    if src.is_null() {
        return;
    }
    unsafe {
        let val = &*src;
        if val.tag() == 6 {
            // TAG_OBJECT — increment Arc refcount
            // We read the payload directly to get the Arc pointer
            let ptr = std::ptr::read(src.cast::<u8>().add(8).cast::<u64>()) as *const VMObject;
            Arc::increment_strong_count(ptr);
        }
    }
}

/// Drop a Value (decrement Arc refcount if it's an object).
/// `src` points to a 16-byte Value in memory.
#[unsafe(no_mangle)]
pub extern "C" fn marlang_jit_value_drop(src: *mut Value) {
    if src.is_null() {
        return;
    }
    unsafe {
        let tag = std::ptr::read(src.cast::<u8>());
        if tag == 6 {
            // TAG_OBJECT — decrement Arc refcount
            let ptr = std::ptr::read(src.cast::<u8>().add(8).cast::<u64>()) as *const VMObject;
            Arc::decrement_strong_count(ptr);
        }
    }
}

// =============================================================================
// Global variable access
// =============================================================================

/// Look up a global variable by name. Returns a clone of the value.
/// `name_ptr` and `name_len` describe the global name string.
/// Writes the result (tag, payload) to `out_tag` and `out_payload`.
/// Returns 0 on success, 1 on failure (undefined variable).
#[unsafe(no_mangle)]
pub extern "C" fn marlang_jit_get_global(
    vm: *mut VM,
    module_id: u32,
    name_ptr: *const u8,
    name_len: u64,
    out_tag: *mut u8,
    out_payload: *mut u64,
) -> u32 {
    unsafe {
        let vm = &*vm;
        let name =
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(name_ptr, name_len as usize));
        match vm.resolve_global(name, module_id) {
            Some(v) => {
                let cloned = v.clone();
                let tag = cloned.tag();
                let payload = cloned.payload();
                std::mem::forget(cloned); // caller takes ownership
                *out_tag = tag;
                *out_payload = payload;
                0
            }
            None => {
                // Try stripping hygiene marks
                let stripped = crate::expand::strip_all_marks(name);
                if stripped != name
                    && let Some(v) = vm.resolve_global(stripped, module_id)
                {
                    let cloned = v.clone();
                    let tag = cloned.tag();
                    let payload = cloned.payload();
                    std::mem::forget(cloned);
                    *out_tag = tag;
                    *out_payload = payload;
                    return 0;
                }
                1
            }
        }
    }
}

/// Set a global variable.
#[unsafe(no_mangle)]
pub extern "C" fn marlang_jit_set_global(
    vm: *mut VM,
    _module_id: u32,
    name_ptr: *const u8,
    name_len: u64,
    tag: u8,
    payload: u64,
) {
    unsafe {
        let vm = &mut *vm;
        let name =
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(name_ptr, name_len as usize));

        // Reconstruct the Value (caller transferred ownership)
        let value = Value::from_raw(tag, payload);

        // Track dirty globals for module export detection
        if let Some(ref mut dirty) = vm.dirty_globals {
            dirty.insert(name.to_string());
        }

        let target_module = vm.current_module;

        // Check for N-ary constructor encoding: "\0ctor:Name:arity"
        if let Some(s) = value.as_string()
            && let Some(rest) = s.strip_prefix("\0ctor:")
            && let Some(colon_pos) = rest.rfind(':')
        {
            let ctor_name = &rest[..colon_pos];
            if let Ok(arity) = rest[colon_pos + 1..].parse::<u8>() {
                vm.module_globals[target_module as usize].insert(
                    name.to_string(),
                    Value::object(VMObject::AdtConstructor {
                        name: ctor_name.to_string(),
                        arity,
                    }),
                );
                return;
            }
        }
        vm.module_globals[target_module as usize].insert(name.to_string(), value);
    }
}

// =============================================================================
// Function calls from JIT code
// =============================================================================

/// Call a Marlang callable (closure, builtin, ADT constructor) from JIT-compiled code.
/// The callee and args are passed as raw Value components.
///
/// This is the main trampoline: JIT code calls this when it doesn't know the callee
/// at compile time or when the callee is not JIT-compiled.
///
/// `args_ptr` points to an array of `Value`s (16 bytes each).
/// Writes result to `out_tag`/`out_payload`.
/// Returns 0 on success, 1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn marlang_jit_call(
    vm: *mut VM,
    callee_tag: u8,
    callee_payload: u64,
    args_ptr: *const Value,
    num_args: u32,
    out_tag: *mut u8,
    out_payload: *mut u64,
) -> u32 {
    unsafe {
        let vm = &mut *vm;
        let callee = Value::from_raw_clone(callee_tag, callee_payload);
        let args = std::slice::from_raw_parts(args_ptr, num_args as usize);

        match call_value(vm, &callee, args) {
            Ok(result) => {
                let tag = result.tag();
                let payload = result.payload();
                std::mem::forget(result);
                *out_tag = tag;
                *out_payload = payload;
                0
            }
            Err(_) => 1,
        }
    }
}

/// Internal: call a Value as a function with the given args.
fn call_value(
    vm: &mut VM,
    callee: &Value,
    args: &[Value],
) -> Result<Value, crate::errors::VMError> {
    if let Some(obj) = callee.as_object() {
        match obj {
            VMObject::Closure(closure) => {
                // Use the VM's interpret machinery for non-JIT closures
                call_closure_from_jit(vm, closure, args)
            }
            VMObject::Builtin { func, .. } => func(vm, args),
            VMObject::AdtConstructor { name, arity } => {
                if args.len() != *arity as usize {
                    return Err(crate::errors::VMError::runtime(format!(
                        "Constructor {} expects {} arguments, got {}",
                        name,
                        arity,
                        args.len()
                    )));
                }
                Ok(Value::object(VMObject::AdtValue {
                    tag: name.clone(),
                    fields: args.to_vec(),
                }))
            }
            _ => Err(crate::errors::VMError::runtime(format!(
                "Cannot call {}",
                obj.kind_name()
            ))),
        }
    } else {
        Err(crate::errors::VMError::runtime(
            "Cannot call non-object value",
        ))
    }
}

/// Call a closure — uses JIT code if available, otherwise falls back to interpreter.
fn call_closure_from_jit(
    vm: &mut VM,
    closure: &Closure,
    args: &[Value],
) -> Result<Value, crate::errors::VMError> {
    // Fast path: if this closure's prototype has JIT code, call it directly
    let proto_key = Arc::as_ptr(&closure.proto) as usize;
    if let Some(&jit_fn) = vm.jit_code.get(&proto_key) {
        let local_count = closure.proto.chunk.local_count as usize;
        let mut locals = vec![Value::nil(); local_count];
        for (i, arg) in args.iter().enumerate() {
            locals[i] = arg.clone();
        }
        let result = unsafe {
            let vm_ptr = vm as *mut VM as *mut u8;
            let closure_ptr = closure as *const Closure as *mut u8;
            let locals_ptr = locals.as_mut_ptr() as *mut u8;
            jit_fn(vm_ptr, closure_ptr, locals_ptr)
        };
        drop(locals);
        let val = unsafe { Value::from_raw(result.tag, result.payload) };
        return Ok(val);
    }

    // Also profile and maybe compile (same logic as the VM dispatch)
    let count = vm.jit_call_counts.entry(proto_key).or_insert(0);
    *count += 1;
    if *count == crate::jit::JIT_THRESHOLD {
        let proto_clone = closure.proto.clone();
        if vm.jit_engine.is_none() {
            vm.jit_engine = crate::jit::JitEngine::new().ok();
        }
        if let Some(ref mut engine) = vm.jit_engine
            && let Some(jit_fn) = engine.compile(&proto_clone)
        {
            vm.jit_code.insert(proto_key, jit_fn);
            // Now call the freshly compiled function
            let local_count = closure.proto.chunk.local_count as usize;
            let mut locals = vec![Value::nil(); local_count];
            for (i, arg) in args.iter().enumerate() {
                locals[i] = arg.clone();
            }
            let result = unsafe {
                let vm_ptr = vm as *mut VM as *mut u8;
                let closure_ptr = closure as *const Closure as *mut u8;
                let locals_ptr = locals.as_mut_ptr() as *mut u8;
                jit_fn(vm_ptr, closure_ptr, locals_ptr)
            };
            drop(locals);
            let val = unsafe { Value::from_raw(result.tag, result.payload) };
            return Ok(val);
        }
    }

    // Slow path: interpret via the VM
    use crate::object::CallFrame;

    let local_count = closure.proto.chunk.local_count as usize;
    let mut locals = vec![Value::nil(); local_count];

    if closure.proto.is_variadic {
        let required = closure.proto.arity as usize;
        for (i, arg) in args.iter().enumerate().take(required) {
            locals[i] = arg.clone();
        }
        let rest: Vec<Value> = args[required..].to_vec();
        locals[required] = Value::object(VMObject::List(rest));
    } else {
        for (i, arg) in args.iter().enumerate() {
            locals[i] = arg.clone();
        }
    }

    let frame = CallFrame {
        closure: closure.clone(),
        ip: 0,
        locals,
        open_upvalues: Vec::new(),
    };

    let base = vm.frame_count;
    vm.push_frame(frame);

    match crate::vm::run(vm, base) {
        Ok(val) => Ok(val),
        Err(crate::vm::VMSignal::Error(e)) => Err(e),
        Err(crate::vm::VMSignal::Effect(sig)) => Err(crate::errors::VMError::runtime(format!(
            "Unhandled effect in JIT call: {}",
            sig.op_name,
        ))),
    }
}

// =============================================================================
// ADT operations
// =============================================================================

/// Construct an ADT value. `tag_ptr`/`tag_len` describe the constructor name.
/// `fields_ptr` points to an array of Values.
#[unsafe(no_mangle)]
pub extern "C" fn marlang_jit_construct_adt(
    tag_ptr: *const u8,
    tag_len: u64,
    fields_ptr: *const Value,
    num_fields: u32,
    out_tag: *mut u8,
    out_payload: *mut u64,
) {
    unsafe {
        let tag_name =
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(tag_ptr, tag_len as usize));
        let fields: Vec<Value> =
            std::slice::from_raw_parts(fields_ptr, num_fields as usize).to_vec();
        let result = Value::object(VMObject::AdtValue {
            tag: tag_name.to_string(),
            fields,
        });
        let t = result.tag();
        let p = result.payload();
        std::mem::forget(result);
        *out_tag = t;
        *out_payload = p;
    }
}

/// Get a field from an ADT value or list. Returns the field value.
/// `value_tag`/`value_payload` is the ADT value.
/// `field_idx` is the field index.
/// Writes result to out_tag/out_payload.
/// Returns 0 on success, 1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn marlang_jit_get_field(
    value_tag: u8,
    value_payload: u64,
    field_idx: u32,
    out_tag: *mut u8,
    out_payload: *mut u64,
) -> u32 {
    unsafe {
        let value = Value::from_raw_clone(value_tag, value_payload);
        if let Some(obj) = value.as_object() {
            match obj {
                VMObject::AdtValue { fields, .. } => {
                    if (field_idx as usize) < fields.len() {
                        let field = fields[field_idx as usize].clone();
                        let t = field.tag();
                        let p = field.payload();
                        std::mem::forget(field);
                        *out_tag = t;
                        *out_payload = p;
                        return 0;
                    }
                }
                VMObject::List(elements) => {
                    if field_idx == 0 && !elements.is_empty() {
                        let field = elements[0].clone();
                        let t = field.tag();
                        let p = field.payload();
                        std::mem::forget(field);
                        *out_tag = t;
                        *out_payload = p;
                        return 0;
                    } else if field_idx == 1 {
                        let rest = Value::object(VMObject::List(elements[1..].to_vec()));
                        let t = rest.tag();
                        let p = rest.payload();
                        std::mem::forget(rest);
                        *out_tag = t;
                        *out_payload = p;
                        return 0;
                    }
                }
                _ => {}
            }
        }
        1
    }
}

/// Check if a Value matches a given ADT tag. Returns 1 if match, 0 if no match.
/// `tag_ptr`/`tag_len` is the expected tag name.
#[unsafe(no_mangle)]
pub extern "C" fn marlang_jit_match_tag(
    value_tag: u8,
    value_payload: u64,
    tag_ptr: *const u8,
    tag_len: u64,
) -> u32 {
    unsafe {
        let value = Value::from_raw_clone(value_tag, value_payload);
        let tag_name =
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(tag_ptr, tag_len as usize));

        let matches = if value.is_nil() {
            tag_name == "Nil"
        } else if value.is_int() || value.is_float() {
            false
        } else if let Some(obj) = value.as_object() {
            match obj {
                VMObject::AdtValue { tag, .. } => tag == tag_name,
                VMObject::List(elements) => {
                    (tag_name == "Cons" && !elements.is_empty())
                        || (tag_name == "Nil" && elements.is_empty())
                }
                _ => false,
            }
        } else {
            false
        };

        if matches { 1 } else { 0 }
    }
}

// =============================================================================
// Equality
// =============================================================================

/// Polymorphic equality check — mirrors `values_equal` from the VM.
#[unsafe(no_mangle)]
pub extern "C" fn marlang_jit_values_equal(
    a_tag: u8,
    a_payload: u64,
    b_tag: u8,
    b_payload: u64,
) -> u32 {
    unsafe {
        let a = Value::from_raw_clone(a_tag, a_payload);
        let b = Value::from_raw_clone(b_tag, b_payload);
        if a == b { 1 } else { 0 }
    }
}

// =============================================================================
// Upvalue access
// =============================================================================

/// Read an immutable upvalue from a closure's upvalue array.
/// `closure_ptr` points to a `Closure` struct.
/// `upvalue_idx` is the index into the upvalues array.
/// Writes result to out_tag/out_payload.
#[unsafe(no_mangle)]
pub extern "C" fn marlang_jit_get_upvalue(
    closure_ptr: *const Closure,
    upvalue_idx: u32,
    out_tag: *mut u8,
    out_payload: *mut u64,
) {
    unsafe {
        let closure = &*closure_ptr;
        let val = closure.upvalues[upvalue_idx as usize].get();
        let tag = val.tag();
        let payload = val.payload();
        std::mem::forget(val);
        *out_tag = tag;
        *out_payload = payload;
    }
}

/// Write to a mutable upvalue (Cell variant) in a closure's upvalue array.
#[unsafe(no_mangle)]
pub extern "C" fn marlang_jit_set_upvalue(
    closure_ptr: *const Closure,
    upvalue_idx: u32,
    tag: u8,
    payload: u64,
) {
    unsafe {
        let closure = &*closure_ptr;
        let val = Value::from_raw_clone(tag, payload);
        closure.upvalues[upvalue_idx as usize].set(val);
    }
}

// =============================================================================
// Closure creation
// =============================================================================

/// Create a closure from a prototype index in the constant pool.
/// This is called by JIT code for Op::Closure.
/// `proto_ptr` points to the FunctionPrototype.
/// `parent_closure_ptr` points to the enclosing Closure (for upvalue resolution).
/// Returns the resulting closure as a Value (tag, payload written to out params).
#[unsafe(no_mangle)]
pub extern "C" fn marlang_jit_create_closure(
    parent_locals_ptr: *const Value,
    parent_locals_len: u32,
    parent_closure_ptr: *const Closure,
    proto_ptr: *const crate::bytecode::FunctionPrototype,
    out_tag: *mut u8,
    out_payload: *mut u64,
) {
    unsafe {
        let proto = &*proto_ptr;
        let parent_closure = &*parent_closure_ptr;
        let parent_locals =
            std::slice::from_raw_parts(parent_locals_ptr, parent_locals_len as usize);

        let mut upvalues = Vec::with_capacity(proto.upvalues.len());
        for desc in &proto.upvalues {
            if desc.is_local {
                // Capture from parent's locals
                let val = parent_locals[desc.index as usize].clone();
                if desc.is_mutable {
                    upvalues.push(crate::object::Upvalue::Cell(Arc::new(
                        std::sync::Mutex::new(val),
                    )));
                } else {
                    upvalues.push(crate::object::Upvalue::Value(val));
                }
            } else {
                // Capture from parent closure's upvalues
                upvalues.push(parent_closure.upvalues[desc.index as usize].clone());
            }
        }

        let closure = Closure {
            proto: Arc::new(proto.clone()),
            upvalues,
            module_id: proto.module_id,
        };
        let result = Value::object(VMObject::Closure(Box::new(closure)));
        let tag = result.tag();
        let payload = result.payload();
        std::mem::forget(result);
        *out_tag = tag;
        *out_payload = payload;
    }
}

// =============================================================================
// Division by zero check
// =============================================================================

/// Check for integer division by zero. Returns 1 if divisor is zero.
#[unsafe(no_mangle)]
pub extern "C" fn marlang_jit_check_div_zero(divisor: i64) -> u32 {
    if divisor == 0 { 1 } else { 0 }
}
