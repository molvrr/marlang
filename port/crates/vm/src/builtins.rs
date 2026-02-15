use crate::errors::VMError;
use crate::object::VMObject;
use crate::value::{format_value, Value};
use crate::vm::VM;

/// Register all built-in globals into the VM.
pub fn register_builtins(vm: &mut VM) {
    // --- Built-in values ---
    vm.globals.insert("nil".to_string(), Value::nil());
    vm.globals.insert("#t".to_string(), Value::boolean(true));
    vm.globals.insert("#f".to_string(), Value::boolean(false));

    // --- Built-in functions ---

    fn builtin(vm: &mut VM, name: &str, func: fn(&mut VM, &[Value]) -> Result<Value, VMError>) {
        vm.globals.insert(
            name.to_string(),
            Value::object(VMObject::Builtin {
                name: name.to_string(),
                func,
            }),
        );
    }

    // Variadic arithmetic (for non-binary calls)
    builtin(vm, "+", |_vm, args| {
        let mut sum = 0.0_f64;
        for a in args {
            match a.as_number() {
                Some(n) => sum += n,
                None => return Err(VMError::runtime("+ expects numbers")),
            }
        }
        Ok(Value::number(sum))
    });

    builtin(vm, "-", |_vm, args| {
        if args.is_empty() {
            return Err(VMError::runtime("- expects at least one argument"));
        }
        let first = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("- expects numbers"))?;
        if args.len() == 1 {
            return Ok(Value::number(-first));
        }
        let mut result = first;
        for a in &args[1..] {
            result -= a
                .as_number()
                .ok_or_else(|| VMError::runtime("- expects numbers"))?;
        }
        Ok(Value::number(result))
    });

    builtin(vm, "*", |_vm, args| {
        let mut product = 1.0_f64;
        for a in args {
            match a.as_number() {
                Some(n) => product *= n,
                None => return Err(VMError::runtime("* expects numbers")),
            }
        }
        Ok(Value::number(product))
    });

    builtin(vm, "/", |_vm, args| {
        if args.len() < 2 {
            return Err(VMError::runtime("/ expects at least two arguments"));
        }
        let first = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("/ expects numbers"))?;
        let mut result = first;
        for a in &args[1..] {
            result /= a
                .as_number()
                .ok_or_else(|| VMError::runtime("/ expects numbers"))?;
        }
        Ok(Value::number(result))
    });

    builtin(vm, "negate", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("negate expects one number"));
        }
        let n = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("negate expects one number"))?;
        Ok(Value::number(-n))
    });

    // Comparison (for non-binary calls)
    builtin(vm, "<", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("< expects two numbers"));
        }
        let a = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("< expects two numbers"))?;
        let b = args[1]
            .as_number()
            .ok_or_else(|| VMError::runtime("< expects two numbers"))?;
        Ok(Value::boolean(a < b))
    });

    builtin(vm, ">", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("> expects two numbers"));
        }
        let a = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("> expects two numbers"))?;
        let b = args[1]
            .as_number()
            .ok_or_else(|| VMError::runtime("> expects two numbers"))?;
        Ok(Value::boolean(a > b))
    });

    builtin(vm, "=", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("= expects two numbers"));
        }
        let a = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("= expects two numbers"))?;
        let b = args[1]
            .as_number()
            .ok_or_else(|| VMError::runtime("= expects two numbers"))?;
        Ok(Value::boolean(a == b))
    });

    builtin(vm, "<=", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("<= expects two numbers"));
        }
        let a = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("<= expects two numbers"))?;
        let b = args[1]
            .as_number()
            .ok_or_else(|| VMError::runtime("<= expects two numbers"))?;
        Ok(Value::boolean(a <= b))
    });

    builtin(vm, ">=", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime(">= expects two numbers"));
        }
        let a = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime(">= expects two numbers"))?;
        let b = args[1]
            .as_number()
            .ok_or_else(|| VMError::runtime(">= expects two numbers"))?;
        Ok(Value::boolean(a >= b))
    });

    // List
    builtin(vm, "list", |_vm, args| {
        Ok(Value::object(VMObject::List(args.to_vec())))
    });

    // Show
    builtin(vm, "show", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("show expects exactly 1 argument"));
        }
        Ok(Value::string(format_value(&args[0]).as_str()))
    });

    // String operations
    builtin(vm, "string-length", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("string-length expects 1 argument"));
        }
        let s = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("string-length expects a string"))?;
        Ok(Value::number(s.len() as f64))
    });

    builtin(vm, "string-append", |_vm, args| {
        let mut result = String::new();
        for a in args {
            let s = a
                .as_string()
                .ok_or_else(|| VMError::runtime("string-append expects strings"))?;
            result.push_str(s);
        }
        Ok(Value::string(result.as_str()))
    });

    builtin(vm, "number->string", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("number->string expects 1 argument"));
        }
        let n = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("number->string expects a number"))?;
        let s = if n == n.trunc() && n.abs() < 1e15 && !n.is_infinite() {
            format!("{}", n as i64)
        } else {
            format!("{}", n)
        };
        Ok(Value::string(s.as_str()))
    });

    builtin(vm, "string->number", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("string->number expects 1 argument"));
        }
        let s = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("string->number expects a string"))?;
        match s.parse::<f64>() {
            Ok(n) => Ok(Value::number(n)),
            Err(_) => Ok(Value::nil()),
        }
    });

    // --- Symbol and list operations (for macro system) ---

    builtin(vm, "make-symbol", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("make-symbol expects 1 argument"));
        }
        let s = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("make-symbol expects a string"))?;
        Ok(Value::symbol(s))
    });

    // Type predicates
    builtin(vm, "symbol?", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("symbol? expects 1 argument"));
        }
        Ok(Value::boolean(matches!(args[0].as_object(), Some(VMObject::Symbol(_)))))
    });

    builtin(vm, "pair?", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("pair? expects 1 argument"));
        }
        Ok(Value::boolean(matches!(
            args[0].as_object(),
            Some(VMObject::AdtValue { tag, fields }) if tag == "Cons" && fields.len() == 2
        )))
    });

    builtin(vm, "nil?", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("nil? expects 1 argument"));
        }
        let is_nil = args[0].is_nil() || matches!(
            args[0].as_object(),
            Some(VMObject::AdtValue { tag, fields }) if tag == "Nil" && fields.is_empty()
        );
        Ok(Value::boolean(is_nil))
    });

    builtin(vm, "number?", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("number? expects 1 argument"));
        }
        Ok(Value::boolean(args[0].is_number()))
    });

    builtin(vm, "string?", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("string? expects 1 argument"));
        }
        Ok(Value::boolean(args[0].as_string().is_some()))
    });

    builtin(vm, "list?", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("list? expects 1 argument"));
        }
        let is_list = args[0].is_nil()
            || matches!(args[0].as_object(), Some(VMObject::AdtValue { tag, .. }) if tag == "Nil" || tag == "Cons");
        Ok(Value::boolean(is_list))
    });

    builtin(vm, "symbol->string", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("symbol->string expects 1 argument"));
        }
        match args[0].as_object() {
            Some(VMObject::Symbol(s)) => Ok(Value::string(s.as_str())),
            _ => Err(VMError::runtime("symbol->string expects a symbol")),
        }
    });

    builtin(vm, "string->symbol", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("string->symbol expects 1 argument"));
        }
        let s = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("string->symbol expects a string"))?;
        Ok(Value::symbol(s))
    });

    {
        use std::sync::atomic::{AtomicU64, Ordering};
        static GENSYM_COUNTER: AtomicU64 = AtomicU64::new(0);
        builtin(vm, "gensym", |_vm, _args| {
            let id = GENSYM_COUNTER.fetch_add(1, Ordering::Relaxed);
            Ok(Value::symbol(format!("__gensym_{}", id)))
        });
    }

    // Error
    builtin(vm, "error", |_vm, args| {
        let msg = if let Some(s) = args.first().and_then(|a| a.as_string()) {
            s.to_string()
        } else {
            args.first()
                .map(|a| format_value(a))
                .unwrap_or_else(|| "error".to_string())
        };
        Err(VMError::runtime(msg))
    });

    // Concurrency
    builtin(vm, "run-fibers", |vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime(
                "run-fibers expects one argument (a thunk)",
            ));
        }
        let closure = match args[0].as_object() {
            Some(VMObject::Closure(c)) => (**c).clone(),
            _ => return Err(VMError::runtime("run-fibers expects a closure")),
        };
        crate::scheduler::run_fibers(vm, &closure)
    });

    // Parallel concurrency (multi-threaded scheduler)
    builtin(vm, "run-fibers-parallel", |vm, args| {
        if args.is_empty() || args.len() > 2 {
            return Err(VMError::runtime(
                "run-fibers-parallel expects 1-2 arguments (thunk [num-threads])",
            ));
        }
        let closure = match args[0].as_object() {
            Some(VMObject::Closure(c)) => (**c).clone(),
            _ => return Err(VMError::runtime("run-fibers-parallel expects a closure")),
        };
        let threads = args
            .get(1)
            .and_then(|v| v.as_number())
            .map(|n| n as usize)
            .unwrap_or(0);
        let threads = if threads == 0 {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
        } else {
            threads
        };
        crate::runtime::run_fibers_parallel(vm, &closure, threads)
    });

    // Profiling: time a thunk and return (result ms)
    builtin(vm, "time-it", |vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("time-it expects 1 argument (a thunk)"));
        }
        let thunk = &args[0];
        let start = std::time::Instant::now();
        // Call the thunk via the VM
        let result = crate::vm::call_value(vm, thunk, &[])?;
        let elapsed = start.elapsed().as_micros() as f64 / 1000.0;
        // Return pair (result, ms)
        let pair = crate::object::VMObject::AdtValue {
            tag: "MkPair".to_string(),
            fields: vec![result, crate::value::Value::number(elapsed)],
        };
        Ok(crate::value::Value::object(pair))
    });

    // FFI builtins
    #[cfg(feature = "ffi")]
    register_ffi_builtins(vm, builtin);
}

#[cfg(feature = "ffi")]
fn register_ffi_builtins(
    vm: &mut VM,
    builtin: fn(&mut VM, &str, fn(&mut VM, &[Value]) -> Result<Value, VMError>),
) {
    use crate::ffi;
    use std::sync::Arc;

    // ffi-open: (ffi-open "libname" (list (list "fn_name" (list "arg_type" ...) "ret_type") ...))
    builtin(vm, "ffi-open", |_vm, args| {
        if args.len() < 2 {
            return Err(VMError::runtime(
                "ffi-open expects library name and signatures list",
            ));
        }
        let lib_name = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("ffi-open: first arg must be string"))?;

        // Parse signatures from the list
        let sig_list_val = &args[1];
        let sig_entries = match sig_list_val.as_object() {
            Some(VMObject::List(items)) => items,
            _ => {
                return Err(VMError::runtime(
                    "ffi-open: second arg must be a list of signature specs",
                ))
            }
        };

        let mut sigs = Vec::new();
        for entry in sig_entries {
            let parts = match entry.as_object() {
                Some(VMObject::List(p)) => p,
                _ => return Err(VMError::runtime("ffi-open: each signature must be a list")),
            };
            if parts.len() != 3 {
                return Err(VMError::runtime(
                    "ffi-open: signature must be (name args-list ret-type)",
                ));
            }
            let fn_name = parts[0]
                .as_string()
                .ok_or_else(|| VMError::runtime("ffi-open: fn name must be string"))?;
            let arg_types_val = match parts[1].as_object() {
                Some(VMObject::List(a)) => a,
                _ => return Err(VMError::runtime("ffi-open: arg types must be a list")),
            };
            let mut arg_types = Vec::new();
            for at in arg_types_val {
                let type_str = at
                    .as_string()
                    .ok_or_else(|| VMError::runtime("ffi-open: arg type must be string"))?;
                if type_str == "void" {
                    continue;
                }
                arg_types.push(ffi::parse_ffi_type(type_str)?);
            }
            let ret_str = parts[2]
                .as_string()
                .ok_or_else(|| VMError::runtime("ffi-open: ret type must be string"))?;
            let ret_type = ffi::parse_ffi_type(ret_str)?;
            sigs.push((
                fn_name.to_string(),
                ffi::FfiSignature {
                    args: arg_types,
                    ret: ret_type,
                },
            ));
        }

        let lib = ffi::ffi_open(lib_name, &sigs)?;
        Ok(Value::object(VMObject::FfiLibrary(Arc::new(lib))))
    });

    // ffi-call: (ffi-call lib "fn_name" arg1 arg2 ...)
    builtin(vm, "ffi-call", |_vm, args| {
        if args.len() < 2 {
            return Err(VMError::runtime(
                "ffi-call expects library and function name",
            ));
        }
        let lib = match args[0].as_object() {
            Some(VMObject::FfiLibrary(lib)) => lib,
            _ => {
                return Err(VMError::runtime(
                    "ffi-call: first arg must be an FFI library",
                ))
            }
        };
        let fn_name = args[1]
            .as_string()
            .ok_or_else(|| VMError::runtime("ffi-call: second arg must be function name string"))?;
        let call_args = &args[2..];
        let registry = ffi::STRUCT_REGISTRY.lock().unwrap();
        ffi::ffi_call(lib, fn_name, call_args, &registry)
    });

    // ffi-close: (ffi-close lib) → nil (library dropped when last reference is gone)
    builtin(vm, "ffi-close", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("ffi-close expects one argument"));
        }
        match args[0].as_object() {
            Some(VMObject::FfiLibrary(_)) => Ok(Value::nil()),
            _ => Err(VMError::runtime(
                "ffi-close: argument must be an FFI library",
            )),
        }
    });

    // ffi-struct: (ffi-struct "name" (list (list "field" "type") ...))
    builtin(vm, "ffi-struct", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("ffi-struct expects name and fields list"));
        }
        let name = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("ffi-struct: first arg must be string"))?;
        let fields_val = match args[1].as_object() {
            Some(VMObject::List(items)) => items,
            _ => return Err(VMError::runtime("ffi-struct: second arg must be a list")),
        };

        let mut fields = Vec::new();
        for f in fields_val {
            let parts = match f.as_object() {
                Some(VMObject::List(p)) => p,
                _ => return Err(VMError::runtime("ffi-struct: each field must be a list")),
            };
            if parts.len() != 2 {
                return Err(VMError::runtime("ffi-struct: field must be (name type)"));
            }
            let fname = parts[0]
                .as_string()
                .ok_or_else(|| VMError::runtime("ffi-struct: field name must be string"))?;
            let ftype_str = parts[1]
                .as_string()
                .ok_or_else(|| VMError::runtime("ffi-struct: field type must be string"))?;
            let ftype = ffi::parse_ffi_type(ftype_str)?;
            fields.push((fname.to_string(), ftype));
        }

        let def = ffi::compute_struct_layout(name, &fields);
        let def = Arc::new(def);
        ffi::STRUCT_REGISTRY
            .lock()
            .unwrap()
            .insert(name.to_string(), Arc::clone(&def));
        Ok(Value::object(VMObject::FfiStructDef(def)))
    });

    // struct-new: (struct-new def field_val1 field_val2 ...)
    builtin(vm, "struct-new", |_vm, args| {
        if args.is_empty() {
            return Err(VMError::runtime("struct-new expects a struct def"));
        }
        let def = match args[0].as_object() {
            Some(VMObject::FfiStructDef(d)) => Arc::clone(d),
            _ => {
                return Err(VMError::runtime(
                    "struct-new: first arg must be a struct def",
                ))
            }
        };
        let field_vals = &args[1..];
        if field_vals.len() != def.fields.len() {
            return Err(VMError::runtime(format!(
                "struct-new: {} expects {} fields, got {}",
                def.name,
                def.fields.len(),
                field_vals.len()
            )));
        }
        let mut buffer = vec![0u8; def.size];
        for (i, field) in def.fields.iter().enumerate() {
            ffi::write_field(&mut buffer, field, &field_vals[i])?;
        }
        Ok(Value::object(VMObject::FfiStruct(Box::new(
            ffi::FfiStruct { def, buffer },
        ))))
    });

    // struct-get: (struct-get s "field_name")
    builtin(vm, "struct-get", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("struct-get expects struct and field name"));
        }
        let s = match args[0].as_object() {
            Some(VMObject::FfiStruct(s)) => s,
            _ => return Err(VMError::runtime("struct-get: first arg must be a struct")),
        };
        let field_name = args[1]
            .as_string()
            .ok_or_else(|| VMError::runtime("struct-get: second arg must be string"))?;
        let field = s
            .def
            .fields
            .iter()
            .find(|f| f.name == field_name)
            .ok_or_else(|| {
                VMError::runtime(format!(
                    "struct-get: no field '{}' in {}",
                    field_name, s.def.name
                ))
            })?;
        Ok(ffi::read_field(&s.buffer, field))
    });

    // struct-set: (struct-set s "field_name" value)
    builtin(vm, "struct-set", |_vm, args| {
        if args.len() != 3 {
            return Err(VMError::runtime(
                "struct-set expects struct, field name, and value",
            ));
        }
        // We need to clone the struct to mutate it (since Values are immutable/refcounted)
        let mut s = match args[0].as_object() {
            Some(VMObject::FfiStruct(s)) => (**s).clone(),
            _ => return Err(VMError::runtime("struct-set: first arg must be a struct")),
        };
        let field_name = args[1]
            .as_string()
            .ok_or_else(|| VMError::runtime("struct-set: second arg must be string"))?;
        let field = s
            .def
            .fields
            .iter()
            .find(|f| f.name == field_name)
            .ok_or_else(|| {
                VMError::runtime(format!(
                    "struct-set: no field '{}' in {}",
                    field_name, s.def.name
                ))
            })?
            .clone();
        ffi::write_field(&mut s.buffer, &field, &args[2])?;
        Ok(Value::object(VMObject::FfiStruct(Box::new(s))))
    });
}
