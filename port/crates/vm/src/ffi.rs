use std::collections::HashMap;
use std::sync::Arc;

use crate::errors::VMError;
use crate::value::Value;

// --- FFI Types ---

#[derive(Debug, Clone, PartialEq)]
pub enum FfiType {
    Void,
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    I64,
    U64,
    F32,
    F64,
    Pointer,
    CString,
    Bool,
}

pub fn parse_ffi_type(s: &str) -> Result<FfiType, VMError> {
    match s {
        "void" => Ok(FfiType::Void),
        "i8" => Ok(FfiType::I8),
        "u8" => Ok(FfiType::U8),
        "i16" => Ok(FfiType::I16),
        "u16" => Ok(FfiType::U16),
        "i32" => Ok(FfiType::I32),
        "u32" => Ok(FfiType::U32),
        "i64" => Ok(FfiType::I64),
        "u64" => Ok(FfiType::U64),
        "f32" => Ok(FfiType::F32),
        "f64" | "double" => Ok(FfiType::F64),
        "pointer" | "ptr" => Ok(FfiType::Pointer),
        "cstring" => Ok(FfiType::CString),
        "bool" => Ok(FfiType::Bool),
        _ => Err(VMError::runtime(format!("Unknown FFI type: {}", s))),
    }
}

// --- FFI Signature ---

#[derive(Debug, Clone)]
pub struct FfiSignature {
    pub args: Vec<FfiType>,
    pub ret: FfiType,
}

// --- FFI Library ---

pub struct FfiLibrary {
    pub lib: libloading::Library,
    pub path: String,
    pub signatures: HashMap<String, FfiSignature>,
}

impl std::fmt::Debug for FfiLibrary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FfiLibrary({})", self.path)
    }
}

// --- FFI Struct types ---

#[derive(Debug, Clone)]
pub struct FfiStructField {
    pub name: String,
    pub ffi_type: FfiType,
    pub offset: usize,
    pub size: usize,
}

#[derive(Debug, Clone)]
pub struct FfiStructDef {
    pub name: String,
    pub fields: Vec<FfiStructField>,
    pub size: usize,
}

#[derive(Debug, Clone)]
pub struct FfiStruct {
    pub def: Arc<FfiStructDef>,
    pub buffer: Vec<u8>,
}

// --- Platform-specific library name resolution ---

pub fn resolve_library_name(name: &str) -> String {
    // If it already has an extension, use as-is
    if name.contains('.') {
        return name.to_string();
    }

    if cfg!(target_os = "macos") {
        format!("lib{}.dylib", name)
    } else if cfg!(target_os = "windows") {
        format!("{}.dll", name)
    } else {
        format!("lib{}.so", name)
    }
}

// --- Library operations ---

pub fn ffi_open(path: &str, sig_list: &[(String, FfiSignature)]) -> Result<FfiLibrary, VMError> {
    let lib_name = resolve_library_name(path);
    let lib = unsafe {
        libloading::Library::new(&lib_name)
            .map_err(|e| VMError::runtime(format!("Failed to load library '{}': {}", lib_name, e)))?
    };
    let mut signatures = HashMap::new();
    for (name, sig) in sig_list {
        signatures.insert(name.clone(), sig.clone());
    }
    Ok(FfiLibrary {
        lib,
        path: lib_name,
        signatures,
    })
}

/// Call an FFI function. Uses transmute for known simple signatures.
pub fn ffi_call(lib: &FfiLibrary, fn_name: &str, args: &[Value]) -> Result<Value, VMError> {
    let sig = lib
        .signatures
        .get(fn_name)
        .ok_or_else(|| VMError::runtime(format!("No signature registered for '{}'", fn_name)))?;

    if args.len() != sig.args.len() {
        return Err(VMError::runtime(format!(
            "FFI {}: expected {} arguments, got {}",
            fn_name,
            sig.args.len(),
            args.len()
        )));
    }

    // Dispatch based on signature pattern
    match (sig.args.as_slice(), &sig.ret) {
        // () -> f64
        ([], FfiType::F64) => {
            let func: libloading::Symbol<unsafe extern "C" fn() -> f64> = unsafe {
                lib.lib
                    .get(fn_name.as_bytes())
                    .map_err(|e| VMError::runtime(format!("Symbol '{}' not found: {}", fn_name, e)))?
            };
            let result = unsafe { func() };
            Ok(Value::number(result))
        }
        // (f64) -> f64
        ([FfiType::F64], FfiType::F64) | ([FfiType::F32], FfiType::F64) => {
            let a = value_to_f64(&args[0], fn_name)?;
            let func: libloading::Symbol<unsafe extern "C" fn(f64) -> f64> = unsafe {
                lib.lib
                    .get(fn_name.as_bytes())
                    .map_err(|e| VMError::runtime(format!("Symbol '{}' not found: {}", fn_name, e)))?
            };
            let result = unsafe { func(a) };
            Ok(Value::number(result))
        }
        // (f64, f64) -> f64
        ([FfiType::F64, FfiType::F64], FfiType::F64) => {
            let a = value_to_f64(&args[0], fn_name)?;
            let b = value_to_f64(&args[1], fn_name)?;
            let func: libloading::Symbol<unsafe extern "C" fn(f64, f64) -> f64> = unsafe {
                lib.lib
                    .get(fn_name.as_bytes())
                    .map_err(|e| VMError::runtime(format!("Symbol '{}' not found: {}", fn_name, e)))?
            };
            let result = unsafe { func(a, b) };
            Ok(Value::number(result))
        }
        // (i32) -> i32
        ([FfiType::I32], FfiType::I32) => {
            let a = value_to_i32(&args[0], fn_name)?;
            let func: libloading::Symbol<unsafe extern "C" fn(i32) -> i32> = unsafe {
                lib.lib
                    .get(fn_name.as_bytes())
                    .map_err(|e| VMError::runtime(format!("Symbol '{}' not found: {}", fn_name, e)))?
            };
            let result = unsafe { func(a) };
            Ok(Value::number(result as f64))
        }
        // (i32, i32) -> i32
        ([FfiType::I32, FfiType::I32], FfiType::I32) => {
            let a = value_to_i32(&args[0], fn_name)?;
            let b = value_to_i32(&args[1], fn_name)?;
            let func: libloading::Symbol<unsafe extern "C" fn(i32, i32) -> i32> = unsafe {
                lib.lib
                    .get(fn_name.as_bytes())
                    .map_err(|e| VMError::runtime(format!("Symbol '{}' not found: {}", fn_name, e)))?
            };
            let result = unsafe { func(a, b) };
            Ok(Value::number(result as f64))
        }
        // (...) -> void
        (_, FfiType::Void) => {
            // For void returns, just call and return nil
            // Currently only support no-arg void
            if sig.args.is_empty() {
                let func: libloading::Symbol<unsafe extern "C" fn()> = unsafe {
                    lib.lib
                        .get(fn_name.as_bytes())
                        .map_err(|e| VMError::runtime(format!("Symbol '{}' not found: {}", fn_name, e)))?
                };
                unsafe { func() };
                Ok(Value::nil())
            } else {
                Err(VMError::runtime(format!(
                    "Unsupported FFI signature for '{}': {:?} -> {:?}",
                    fn_name, sig.args, sig.ret
                )))
            }
        }
        _ => Err(VMError::runtime(format!(
            "Unsupported FFI signature for '{}': {:?} -> {:?}",
            fn_name, sig.args, sig.ret
        ))),
    }
}

fn value_to_f64(v: &Value, fn_name: &str) -> Result<f64, VMError> {
    v.as_number().ok_or_else(|| {
        VMError::runtime(format!(
            "FFI {}: expected number, got {}",
            fn_name,
            v.kind_name()
        ))
    })
}

fn value_to_i32(v: &Value, fn_name: &str) -> Result<i32, VMError> {
    let n = v.as_number().ok_or_else(|| {
        VMError::runtime(format!(
            "FFI {}: expected number, got {}",
            fn_name,
            v.kind_name()
        ))
    })?;
    Ok(n as i32)
}

// --- Struct operations ---

pub fn type_size(t: &FfiType) -> usize {
    match t {
        FfiType::Void => 0,
        FfiType::I8 | FfiType::U8 | FfiType::Bool => 1,
        FfiType::I16 | FfiType::U16 => 2,
        FfiType::I32 | FfiType::U32 | FfiType::F32 => 4,
        FfiType::I64 | FfiType::U64 | FfiType::F64 | FfiType::Pointer | FfiType::CString => 8,
    }
}

fn type_alignment(t: &FfiType) -> usize {
    type_size(t).max(1)
}

fn align_up(offset: usize, alignment: usize) -> usize {
    (offset + alignment - 1) & !(alignment - 1)
}

pub fn compute_struct_layout(
    name: &str,
    fields: &[(String, FfiType)],
) -> FfiStructDef {
    let mut result_fields = Vec::new();
    let mut offset = 0usize;

    for (fname, ftype) in fields {
        let align = type_alignment(ftype);
        offset = align_up(offset, align);
        let size = type_size(ftype);
        result_fields.push(FfiStructField {
            name: fname.clone(),
            ffi_type: ftype.clone(),
            offset,
            size,
        });
        offset += size;
    }

    // Final struct size aligned to max field alignment
    let max_align = fields
        .iter()
        .map(|(_, t)| type_alignment(t))
        .max()
        .unwrap_or(1);
    let total_size = align_up(offset, max_align);

    FfiStructDef {
        name: name.to_string(),
        fields: result_fields,
        size: total_size,
    }
}

pub fn write_field(buffer: &mut [u8], field: &FfiStructField, value: &Value) -> Result<(), VMError> {
    let n = value.as_number().ok_or_else(|| {
        VMError::runtime(format!(
            "struct-set: field '{}' expects a number",
            field.name
        ))
    })?;

    let offset = field.offset;
    match field.ffi_type {
        FfiType::I8 => buffer[offset] = n as i8 as u8,
        FfiType::U8 => buffer[offset] = n as u8,
        FfiType::Bool => buffer[offset] = if n != 0.0 { 1 } else { 0 },
        FfiType::I16 => {
            let bytes = (n as i16).to_ne_bytes();
            buffer[offset..offset + 2].copy_from_slice(&bytes);
        }
        FfiType::U16 => {
            let bytes = (n as u16).to_ne_bytes();
            buffer[offset..offset + 2].copy_from_slice(&bytes);
        }
        FfiType::I32 => {
            let bytes = (n as i32).to_ne_bytes();
            buffer[offset..offset + 4].copy_from_slice(&bytes);
        }
        FfiType::U32 => {
            let bytes = (n as u32).to_ne_bytes();
            buffer[offset..offset + 4].copy_from_slice(&bytes);
        }
        FfiType::F32 => {
            let bytes = (n as f32).to_ne_bytes();
            buffer[offset..offset + 4].copy_from_slice(&bytes);
        }
        FfiType::I64 => {
            let bytes = (n as i64).to_ne_bytes();
            buffer[offset..offset + 8].copy_from_slice(&bytes);
        }
        FfiType::U64 => {
            let bytes = (n as u64).to_ne_bytes();
            buffer[offset..offset + 8].copy_from_slice(&bytes);
        }
        FfiType::F64 => {
            let bytes = n.to_ne_bytes();
            buffer[offset..offset + 8].copy_from_slice(&bytes);
        }
        FfiType::Pointer | FfiType::CString => {
            let bytes = (n as u64).to_ne_bytes();
            buffer[offset..offset + 8].copy_from_slice(&bytes);
        }
        FfiType::Void => {}
    }
    Ok(())
}

pub fn read_field(buffer: &[u8], field: &FfiStructField) -> Value {
    let offset = field.offset;
    match field.ffi_type {
        FfiType::I8 => Value::number(buffer[offset] as i8 as f64),
        FfiType::U8 => Value::number(buffer[offset] as f64),
        FfiType::Bool => Value::boolean(buffer[offset] != 0),
        FfiType::I16 => {
            let mut bytes = [0u8; 2];
            bytes.copy_from_slice(&buffer[offset..offset + 2]);
            Value::number(i16::from_ne_bytes(bytes) as f64)
        }
        FfiType::U16 => {
            let mut bytes = [0u8; 2];
            bytes.copy_from_slice(&buffer[offset..offset + 2]);
            Value::number(u16::from_ne_bytes(bytes) as f64)
        }
        FfiType::I32 => {
            let mut bytes = [0u8; 4];
            bytes.copy_from_slice(&buffer[offset..offset + 4]);
            Value::number(i32::from_ne_bytes(bytes) as f64)
        }
        FfiType::U32 => {
            let mut bytes = [0u8; 4];
            bytes.copy_from_slice(&buffer[offset..offset + 4]);
            Value::number(u32::from_ne_bytes(bytes) as f64)
        }
        FfiType::F32 => {
            let mut bytes = [0u8; 4];
            bytes.copy_from_slice(&buffer[offset..offset + 4]);
            Value::number(f32::from_ne_bytes(bytes) as f64)
        }
        FfiType::I64 => {
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&buffer[offset..offset + 8]);
            Value::number(i64::from_ne_bytes(bytes) as f64)
        }
        FfiType::U64 => {
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&buffer[offset..offset + 8]);
            Value::number(u64::from_ne_bytes(bytes) as f64)
        }
        FfiType::F64 => {
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&buffer[offset..offset + 8]);
            Value::number(f64::from_ne_bytes(bytes))
        }
        FfiType::Pointer | FfiType::CString => {
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&buffer[offset..offset + 8]);
            Value::number(u64::from_ne_bytes(bytes) as f64)
        }
        FfiType::Void => Value::nil(),
    }
}
