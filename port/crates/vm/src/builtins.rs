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

    // --- Built-in ADT constructors ---
    vm.globals.insert(
        "None".to_string(),
        Value::object(VMObject::AdtValue { tag: "None".to_string(), fields: vec![] }),
    );
    vm.globals.insert(
        "Some".to_string(),
        Value::object(VMObject::AdtConstructor { name: "Some".to_string(), arity: 1 }),
    );
    vm.globals.insert(
        "Nil".to_string(),
        Value::object(VMObject::AdtValue { tag: "Nil".to_string(), fields: vec![] }),
    );
    vm.globals.insert(
        "Cons".to_string(),
        Value::object(VMObject::AdtConstructor { name: "Cons".to_string(), arity: 2 }),
    );
    vm.globals.insert(
        "MkPair".to_string(),
        Value::object(VMObject::AdtConstructor { name: "MkPair".to_string(), arity: 2 }),
    );
    vm.globals.insert(
        "Ok".to_string(),
        Value::object(VMObject::AdtConstructor { name: "Ok".to_string(), arity: 1 }),
    );
    vm.globals.insert(
        "Err".to_string(),
        Value::object(VMObject::AdtConstructor { name: "Err".to_string(), arity: 1 }),
    );

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
        let first = args[0].as_number().ok_or_else(|| VMError::runtime("- expects numbers"))?;
        if args.len() == 1 {
            return Ok(Value::number(-first));
        }
        let mut result = first;
        for a in &args[1..] {
            result -= a.as_number().ok_or_else(|| VMError::runtime("- expects numbers"))?;
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
        let first = args[0].as_number().ok_or_else(|| VMError::runtime("/ expects numbers"))?;
        let mut result = first;
        for a in &args[1..] {
            result /= a.as_number().ok_or_else(|| VMError::runtime("/ expects numbers"))?;
        }
        Ok(Value::number(result))
    });

    builtin(vm, "negate", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("negate expects one number"));
        }
        let n = args[0].as_number().ok_or_else(|| VMError::runtime("negate expects one number"))?;
        Ok(Value::number(-n))
    });

    // Comparison (for non-binary calls)
    builtin(vm, "<", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("< expects two numbers"));
        }
        let a = args[0].as_number().ok_or_else(|| VMError::runtime("< expects two numbers"))?;
        let b = args[1].as_number().ok_or_else(|| VMError::runtime("< expects two numbers"))?;
        Ok(Value::boolean(a < b))
    });

    builtin(vm, ">", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("> expects two numbers"));
        }
        let a = args[0].as_number().ok_or_else(|| VMError::runtime("> expects two numbers"))?;
        let b = args[1].as_number().ok_or_else(|| VMError::runtime("> expects two numbers"))?;
        Ok(Value::boolean(a > b))
    });

    builtin(vm, "=", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("= expects two numbers"));
        }
        let a = args[0].as_number().ok_or_else(|| VMError::runtime("= expects two numbers"))?;
        let b = args[1].as_number().ok_or_else(|| VMError::runtime("= expects two numbers"))?;
        Ok(Value::boolean(a == b))
    });

    builtin(vm, "<=", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("<= expects two numbers"));
        }
        let a = args[0].as_number().ok_or_else(|| VMError::runtime("<= expects two numbers"))?;
        let b = args[1].as_number().ok_or_else(|| VMError::runtime("<= expects two numbers"))?;
        Ok(Value::boolean(a <= b))
    });

    builtin(vm, ">=", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime(">= expects two numbers"));
        }
        let a = args[0].as_number().ok_or_else(|| VMError::runtime(">= expects two numbers"))?;
        let b = args[1].as_number().ok_or_else(|| VMError::runtime(">= expects two numbers"))?;
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

    // Not
    builtin(vm, "not", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("not expects exactly 1 argument"));
        }
        Ok(Value::boolean(!args[0].is_truthy()))
    });

    // String operations
    builtin(vm, "string-length", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("string-length expects 1 argument"));
        }
        let s = args[0].as_string().ok_or_else(|| VMError::runtime("string-length expects a string"))?;
        Ok(Value::number(s.len() as f64))
    });

    builtin(vm, "string-append", |_vm, args| {
        let mut result = String::new();
        for a in args {
            let s = a.as_string().ok_or_else(|| VMError::runtime("string-append expects strings"))?;
            result.push_str(s);
        }
        Ok(Value::string(result.as_str()))
    });

    builtin(vm, "number->string", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("number->string expects 1 argument"));
        }
        let n = args[0].as_number().ok_or_else(|| VMError::runtime("number->string expects a number"))?;
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
        let s = args[0].as_string().ok_or_else(|| VMError::runtime("string->number expects a string"))?;
        match s.parse::<f64>() {
            Ok(n) => Ok(Value::number(n)),
            Err(_) => Ok(Value::nil()),
        }
    });

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

    // Stubs for Phase 5+
    builtin(vm, "run-fibers", |_vm, _args| {
        Err(VMError::runtime("run-fibers is not yet supported in the Rust VM"))
    });
}
