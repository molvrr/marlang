use crate::errors::VMError;
use crate::object::VMObject;
use crate::value::Value;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

// --- Global struct registry ---

pub static STRUCT_REGISTRY: LazyLock<Mutex<HashMap<String, Arc<FfiStructDef>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

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
    Struct(String),
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
        _ => Ok(FfiType::Struct(s.to_string())),
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
        libloading::Library::new(&lib_name).map_err(|e| {
            VMError::runtime(format!("Failed to load library '{}': {}", lib_name, e))
        })?
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

// --- libffi type conversion ---

fn to_libffi_type(
    ffi_type: &FfiType,
    registry: &HashMap<String, Arc<FfiStructDef>>,
) -> Result<libffi::middle::Type, VMError> {
    use libffi::middle::Type;
    match ffi_type {
        FfiType::Void => Ok(Type::void()),
        FfiType::I8 => Ok(Type::i8()),
        FfiType::U8 => Ok(Type::u8()),
        FfiType::I16 => Ok(Type::i16()),
        FfiType::U16 => Ok(Type::u16()),
        FfiType::I32 => Ok(Type::i32()),
        FfiType::U32 => Ok(Type::u32()),
        FfiType::I64 => Ok(Type::i64()),
        FfiType::U64 => Ok(Type::u64()),
        FfiType::F32 => Ok(Type::f32()),
        FfiType::F64 => Ok(Type::f64()),
        FfiType::Pointer | FfiType::CString => Ok(Type::pointer()),
        FfiType::Bool => Ok(Type::u8()),
        FfiType::Struct(name) => {
            let def = registry.get(name).ok_or_else(|| {
                VMError::runtime(format!("FFI struct type '{}' not found in registry", name))
            })?;
            let field_types: Result<Vec<Type>, VMError> = def
                .fields
                .iter()
                .map(|f| to_libffi_type(&f.ffi_type, registry))
                .collect();
            Ok(Type::structure(field_types?))
        }
    }
}

// --- Argument storage for libffi calls ---

// We need stable memory addresses for libffi::middle::Arg, so we store
// each converted argument in a typed enum variant.
enum ArgStorage {
    I8(i8),
    U8(u8),
    I16(i16),
    U16(u16),
    I32(i32),
    U32(u32),
    I64(i64),
    U64(u64),
    F32(f32),
    F64(f64),
    Bool(u8),
    Ptr(*const u8),
    StructBuf(Vec<u8>),
}

/// Call an FFI function using libffi for arbitrary signatures.
pub fn ffi_call(
    lib: &FfiLibrary,
    fn_name: &str,
    args: &[Value],
    registry: &HashMap<String, Arc<FfiStructDef>>,
) -> Result<Value, VMError> {
    use libffi::middle::{Arg, Cif, CodePtr, Ret, Type};

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

    // Look up the function symbol
    let func_ptr: libloading::Symbol<unsafe extern "C" fn()> = unsafe {
        lib.lib
            .get(fn_name.as_bytes())
            .map_err(|e| VMError::runtime(format!("Symbol '{}' not found: {}", fn_name, e)))?
    };
    let code_ptr = CodePtr::from_ptr(*func_ptr as *const _);

    // Build libffi types
    let arg_types: Result<Vec<Type>, VMError> = sig
        .args
        .iter()
        .map(|t| to_libffi_type(t, registry))
        .collect();
    let arg_types = arg_types?;
    let ret_type = to_libffi_type(&sig.ret, registry)?;
    let cif = Cif::new(arg_types, ret_type);

    // Convert args to storage — CStrings must live until after the call
    let mut storage: Vec<ArgStorage> = Vec::with_capacity(args.len());
    let mut cstring_storage: Vec<std::ffi::CString> = Vec::new();

    for (i, (val, ffi_t)) in args.iter().zip(sig.args.iter()).enumerate() {
        match ffi_t {
            FfiType::I8 => {
                let n = value_to_f64(val, fn_name, i)?;
                storage.push(ArgStorage::I8(n as i8));
            }
            FfiType::U8 => {
                let n = value_to_f64(val, fn_name, i)?;
                storage.push(ArgStorage::U8(n as u8));
            }
            FfiType::I16 => {
                let n = value_to_f64(val, fn_name, i)?;
                storage.push(ArgStorage::I16(n as i16));
            }
            FfiType::U16 => {
                let n = value_to_f64(val, fn_name, i)?;
                storage.push(ArgStorage::U16(n as u16));
            }
            FfiType::I32 => {
                let n = value_to_f64(val, fn_name, i)?;
                storage.push(ArgStorage::I32(n as i32));
            }
            FfiType::U32 => {
                let n = value_to_f64(val, fn_name, i)?;
                storage.push(ArgStorage::U32(n as u32));
            }
            FfiType::I64 => {
                let n = value_to_f64(val, fn_name, i)?;
                storage.push(ArgStorage::I64(n as i64));
            }
            FfiType::U64 => {
                let n = value_to_f64(val, fn_name, i)?;
                storage.push(ArgStorage::U64(n as u64));
            }
            FfiType::F32 => {
                let n = value_to_f64(val, fn_name, i)?;
                storage.push(ArgStorage::F32(n as f32));
            }
            FfiType::F64 => {
                let n = value_to_f64(val, fn_name, i)?;
                storage.push(ArgStorage::F64(n));
            }
            FfiType::Bool => {
                let n = value_to_f64(val, fn_name, i)?;
                storage.push(ArgStorage::Bool(if n != 0.0 { 1 } else { 0 }));
            }
            FfiType::CString => {
                let s = val.as_string().ok_or_else(|| {
                    VMError::runtime(format!(
                        "FFI {}: arg {} expected string for cstring, got {}",
                        fn_name,
                        i,
                        val.kind_name()
                    ))
                })?;
                let cs = std::ffi::CString::new(s.as_bytes()).map_err(|e| {
                    VMError::runtime(format!("FFI {}: invalid cstring arg {}: {}", fn_name, i, e))
                })?;
                let ptr = cs.as_ptr() as *const u8;
                cstring_storage.push(cs);
                storage.push(ArgStorage::Ptr(ptr));
            }
            FfiType::Pointer => {
                let ptr = value_to_pointer(val, fn_name, i)?;
                storage.push(ArgStorage::Ptr(ptr as *const u8));
            }
            FfiType::Struct(name) => {
                let def = registry.get(name).ok_or_else(|| {
                    VMError::runtime(format!(
                        "FFI {}: struct type '{}' not found in registry",
                        fn_name, name
                    ))
                })?;
                match val.as_object() {
                    Some(VMObject::FfiStruct(s)) => {
                        if s.def.name != *name {
                            return Err(VMError::runtime(format!(
                                "FFI {}: arg {} expected struct '{}', got struct '{}'",
                                fn_name, i, name, s.def.name
                            )));
                        }
                        // Copy buffer for stable address
                        let mut buf = vec![0u8; def.size];
                        buf[..s.buffer.len().min(def.size)]
                            .copy_from_slice(&s.buffer[..s.buffer.len().min(def.size)]);
                        storage.push(ArgStorage::StructBuf(buf));
                    }
                    _ => {
                        return Err(VMError::runtime(format!(
                            "FFI {}: arg {} expected struct '{}', got {}",
                            fn_name,
                            i,
                            name,
                            val.kind_name()
                        )));
                    }
                }
            }
            FfiType::Void => {
                // Shouldn't appear as arg type, skip
            }
        }
    }

    // Build Arg references from stable storage
    let ffi_args: Vec<Arg> = storage
        .iter()
        .map(|s| match s {
            ArgStorage::I8(v) => Arg::new(v),
            ArgStorage::U8(v) => Arg::new(v),
            ArgStorage::I16(v) => Arg::new(v),
            ArgStorage::U16(v) => Arg::new(v),
            ArgStorage::I32(v) => Arg::new(v),
            ArgStorage::U32(v) => Arg::new(v),
            ArgStorage::I64(v) => Arg::new(v),
            ArgStorage::U64(v) => Arg::new(v),
            ArgStorage::F32(v) => Arg::new(v),
            ArgStorage::F64(v) => Arg::new(v),
            ArgStorage::Bool(v) => Arg::new(v),
            ArgStorage::Ptr(v) => Arg::new(v),
            ArgStorage::StructBuf(buf) => Arg::new(&buf[0]),
        })
        .collect();

    // Call and convert return value
    unsafe {
        match &sig.ret {
            FfiType::Void => {
                cif.call::<()>(code_ptr, &ffi_args);
                Ok(Value::nil())
            }
            FfiType::I8 => {
                let r: i8 = cif.call(code_ptr, &ffi_args);
                Ok(Value::int(r as i64))
            }
            FfiType::U8 => {
                let r: u8 = cif.call(code_ptr, &ffi_args);
                Ok(Value::int(r as i64))
            }
            FfiType::I16 => {
                let r: i16 = cif.call(code_ptr, &ffi_args);
                Ok(Value::int(r as i64))
            }
            FfiType::U16 => {
                let r: u16 = cif.call(code_ptr, &ffi_args);
                Ok(Value::int(r as i64))
            }
            FfiType::I32 => {
                let r: i32 = cif.call(code_ptr, &ffi_args);
                Ok(Value::int(r as i64))
            }
            FfiType::U32 => {
                let r: u32 = cif.call(code_ptr, &ffi_args);
                Ok(Value::int(r as i64))
            }
            FfiType::I64 => {
                let r: i64 = cif.call(code_ptr, &ffi_args);
                Ok(Value::int(r))
            }
            FfiType::U64 => {
                let r: u64 = cif.call(code_ptr, &ffi_args);
                Ok(Value::int(r as i64))
            }
            FfiType::F32 => {
                let r: f32 = cif.call(code_ptr, &ffi_args);
                Ok(Value::float(r as f64))
            }
            FfiType::F64 => {
                let r: f64 = cif.call(code_ptr, &ffi_args);
                Ok(Value::float(r))
            }
            FfiType::Bool => {
                let r: u8 = cif.call(code_ptr, &ffi_args);
                Ok(Value::boolean(r != 0))
            }
            FfiType::Pointer | FfiType::CString => {
                let r: *const u8 = cif.call(code_ptr, &ffi_args);
                if r.is_null() {
                    Ok(Value::nil())
                } else {
                    Ok(Value::object(VMObject::FfiPointer(r as u64)))
                }
            }
            FfiType::Struct(name) => {
                let def = registry.get(name).ok_or_else(|| {
                    VMError::runtime(format!(
                        "FFI {}: return struct type '{}' not found in registry",
                        fn_name, name
                    ))
                })?;
                // Allocate aligned buffer for the return value
                let mut buf = vec![0u8; def.size];
                let ret = Ret::new(&mut buf[0]);
                cif.call_return_into(code_ptr, &ffi_args, ret);
                Ok(Value::object(VMObject::FfiStruct(Box::new(FfiStruct {
                    def: Arc::clone(def),
                    buffer: buf,
                }))))
            }
        }
    }
}

fn value_to_f64(v: &Value, fn_name: &str, arg_idx: usize) -> Result<f64, VMError> {
    v.as_number_f64().ok_or_else(|| {
        VMError::runtime(format!(
            "FFI {}: arg {} expected number, got {}",
            fn_name,
            arg_idx,
            v.kind_name()
        ))
    })
}

fn value_to_pointer(v: &Value, fn_name: &str, arg_idx: usize) -> Result<u64, VMError> {
    // Accept: FfiPointer, FfiStruct (buffer pointer), ByteArray, nil (null), or number
    match v.as_object() {
        Some(VMObject::FfiPointer(addr)) => Ok(*addr),
        Some(VMObject::FfiStruct(s)) => Ok(s.buffer.as_ptr() as u64),
        Some(VMObject::ByteArray(bytes)) => Ok(bytes.as_ptr() as u64),
        _ => {
            if v.is_nil() {
                Ok(0)
            } else if let Some(n) = v.as_number_f64() {
                Ok(n as u64)
            } else {
                Err(VMError::runtime(format!(
                    "FFI {}: arg {} expected pointer, got {}",
                    fn_name,
                    arg_idx,
                    v.kind_name()
                )))
            }
        }
    }
}

// --- Struct operations ---

pub fn type_size(t: &FfiType) -> usize {
    match t {
        FfiType::Void => 0,
        FfiType::I8 | FfiType::U8 | FfiType::Bool => 1,
        FfiType::I16 | FfiType::U16 => 2,
        FfiType::I32 | FfiType::U32 | FfiType::F32 => 4,
        FfiType::I64 | FfiType::U64 | FfiType::F64 | FfiType::Pointer | FfiType::CString => 8,
        FfiType::Struct(name) => {
            // Look up in registry; fall back to pointer size
            STRUCT_REGISTRY
                .lock()
                .unwrap()
                .get(name)
                .map(|d| d.size)
                .unwrap_or(8)
        }
    }
}

fn type_alignment(t: &FfiType) -> usize {
    match t {
        FfiType::Struct(name) => STRUCT_REGISTRY
            .lock()
            .unwrap()
            .get(name)
            .map(|d| {
                d.fields
                    .iter()
                    .map(|f| type_alignment(&f.ffi_type))
                    .max()
                    .unwrap_or(1)
            })
            .unwrap_or(8),
        _ => type_size(t).max(1),
    }
}

fn align_up(offset: usize, alignment: usize) -> usize {
    (offset + alignment - 1) & !(alignment - 1)
}

pub fn compute_struct_layout(name: &str, fields: &[(String, FfiType)]) -> FfiStructDef {
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

pub fn write_field(
    buffer: &mut [u8],
    field: &FfiStructField,
    value: &Value,
) -> Result<(), VMError> {
    let offset = field.offset;
    match field.ffi_type {
        FfiType::I8 => {
            let n = value.as_number_f64().ok_or_else(|| {
                VMError::runtime(format!(
                    "struct-set: field '{}' expects a number",
                    field.name
                ))
            })?;
            buffer[offset] = n as i8 as u8;
        }
        FfiType::U8 => {
            let n = value.as_number_f64().ok_or_else(|| {
                VMError::runtime(format!(
                    "struct-set: field '{}' expects a number",
                    field.name
                ))
            })?;
            buffer[offset] = n as u8;
        }
        FfiType::Bool => {
            let n = value.as_number_f64().ok_or_else(|| {
                VMError::runtime(format!(
                    "struct-set: field '{}' expects a number",
                    field.name
                ))
            })?;
            buffer[offset] = if n != 0.0 { 1 } else { 0 };
        }
        FfiType::I16 => {
            let n = value.as_number_f64().ok_or_else(|| {
                VMError::runtime(format!(
                    "struct-set: field '{}' expects a number",
                    field.name
                ))
            })?;
            let bytes = (n as i16).to_ne_bytes();
            buffer[offset..offset + 2].copy_from_slice(&bytes);
        }
        FfiType::U16 => {
            let n = value.as_number_f64().ok_or_else(|| {
                VMError::runtime(format!(
                    "struct-set: field '{}' expects a number",
                    field.name
                ))
            })?;
            let bytes = (n as u16).to_ne_bytes();
            buffer[offset..offset + 2].copy_from_slice(&bytes);
        }
        FfiType::I32 => {
            let n = value.as_number_f64().ok_or_else(|| {
                VMError::runtime(format!(
                    "struct-set: field '{}' expects a number",
                    field.name
                ))
            })?;
            let bytes = (n as i32).to_ne_bytes();
            buffer[offset..offset + 4].copy_from_slice(&bytes);
        }
        FfiType::U32 => {
            let n = value.as_number_f64().ok_or_else(|| {
                VMError::runtime(format!(
                    "struct-set: field '{}' expects a number",
                    field.name
                ))
            })?;
            let bytes = (n as u32).to_ne_bytes();
            buffer[offset..offset + 4].copy_from_slice(&bytes);
        }
        FfiType::F32 => {
            let n = value.as_number_f64().ok_or_else(|| {
                VMError::runtime(format!(
                    "struct-set: field '{}' expects a number",
                    field.name
                ))
            })?;
            let bytes = (n as f32).to_ne_bytes();
            buffer[offset..offset + 4].copy_from_slice(&bytes);
        }
        FfiType::I64 => {
            let n = value.as_number_f64().ok_or_else(|| {
                VMError::runtime(format!(
                    "struct-set: field '{}' expects a number",
                    field.name
                ))
            })?;
            let bytes = (n as i64).to_ne_bytes();
            buffer[offset..offset + 8].copy_from_slice(&bytes);
        }
        FfiType::U64 => {
            let n = value.as_number_f64().ok_or_else(|| {
                VMError::runtime(format!(
                    "struct-set: field '{}' expects a number",
                    field.name
                ))
            })?;
            let bytes = (n as u64).to_ne_bytes();
            buffer[offset..offset + 8].copy_from_slice(&bytes);
        }
        FfiType::F64 => {
            let n = value.as_number_f64().ok_or_else(|| {
                VMError::runtime(format!(
                    "struct-set: field '{}' expects a number",
                    field.name
                ))
            })?;
            let bytes = n.to_ne_bytes();
            buffer[offset..offset + 8].copy_from_slice(&bytes);
        }
        FfiType::Pointer | FfiType::CString => {
            let n = value.as_number_f64().ok_or_else(|| {
                VMError::runtime(format!(
                    "struct-set: field '{}' expects a number",
                    field.name
                ))
            })?;
            let bytes = (n as u64).to_ne_bytes();
            buffer[offset..offset + 8].copy_from_slice(&bytes);
        }
        FfiType::Void | FfiType::Struct(_) => {}
    }
    Ok(())
}

pub fn read_field(buffer: &[u8], field: &FfiStructField) -> Value {
    let offset = field.offset;
    match field.ffi_type {
        FfiType::I8 => Value::int(buffer[offset] as i8 as i64),
        FfiType::U8 => Value::int(buffer[offset] as i64),
        FfiType::Bool => Value::boolean(buffer[offset] != 0),
        FfiType::I16 => {
            let mut bytes = [0u8; 2];
            bytes.copy_from_slice(&buffer[offset..offset + 2]);
            Value::int(i16::from_ne_bytes(bytes) as i64)
        }
        FfiType::U16 => {
            let mut bytes = [0u8; 2];
            bytes.copy_from_slice(&buffer[offset..offset + 2]);
            Value::int(u16::from_ne_bytes(bytes) as i64)
        }
        FfiType::I32 => {
            let mut bytes = [0u8; 4];
            bytes.copy_from_slice(&buffer[offset..offset + 4]);
            Value::int(i32::from_ne_bytes(bytes) as i64)
        }
        FfiType::U32 => {
            let mut bytes = [0u8; 4];
            bytes.copy_from_slice(&buffer[offset..offset + 4]);
            Value::int(u32::from_ne_bytes(bytes) as i64)
        }
        FfiType::F32 => {
            let mut bytes = [0u8; 4];
            bytes.copy_from_slice(&buffer[offset..offset + 4]);
            Value::float(f32::from_ne_bytes(bytes) as f64)
        }
        FfiType::I64 => {
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&buffer[offset..offset + 8]);
            Value::int(i64::from_ne_bytes(bytes))
        }
        FfiType::U64 => {
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&buffer[offset..offset + 8]);
            Value::int(u64::from_ne_bytes(bytes) as i64)
        }
        FfiType::F64 => {
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&buffer[offset..offset + 8]);
            Value::float(f64::from_ne_bytes(bytes))
        }
        FfiType::Pointer | FfiType::CString => {
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&buffer[offset..offset + 8]);
            Value::int(u64::from_ne_bytes(bytes) as i64)
        }
        FfiType::Void | FfiType::Struct(_) => Value::nil(),
    }
}
