use crate::errors::VMError;
use crate::object::VMObject;
use crate::value::{Value, format_value};
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

    // Float arithmetic (same runtime behavior — everything is f64)
    builtin(vm, "+.", |_vm, args| {
        let mut sum = 0.0_f64;
        for a in args {
            match a.as_number() {
                Some(n) => sum += n,
                None => return Err(VMError::runtime("+. expects numbers")),
            }
        }
        Ok(Value::number(sum))
    });

    builtin(vm, "-.", |_vm, args| {
        if args.is_empty() {
            return Err(VMError::runtime("-. expects at least one argument"));
        }
        let first = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("-. expects numbers"))?;
        if args.len() == 1 {
            return Ok(Value::number(-first));
        }
        let mut result = first;
        for a in &args[1..] {
            result -= a
                .as_number()
                .ok_or_else(|| VMError::runtime("-. expects numbers"))?;
        }
        Ok(Value::number(result))
    });

    builtin(vm, "*.", |_vm, args| {
        let mut product = 1.0_f64;
        for a in args {
            match a.as_number() {
                Some(n) => product *= n,
                None => return Err(VMError::runtime("*. expects numbers")),
            }
        }
        Ok(Value::number(product))
    });

    builtin(vm, "/.", |_vm, args| {
        if args.len() < 2 {
            return Err(VMError::runtime("/. expects at least two arguments"));
        }
        let first = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("/. expects numbers"))?;
        let mut result = first;
        for a in &args[1..] {
            result /= a
                .as_number()
                .ok_or_else(|| VMError::runtime("/. expects numbers"))?;
        }
        Ok(Value::number(result))
    });

    builtin(vm, "negate.", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("negate. expects one number"));
        }
        let n = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("negate. expects one number"))?;
        Ok(Value::number(-n))
    });

    // Math builtins
    builtin(vm, "rem", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("rem expects two numbers"));
        }
        let a = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("rem expects numbers"))?;
        let b = args[1]
            .as_number()
            .ok_or_else(|| VMError::runtime("rem expects numbers"))?;
        Ok(Value::number(a % b))
    });

    builtin(vm, "rem.", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("rem. expects two numbers"));
        }
        let a = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("rem. expects numbers"))?;
        let b = args[1]
            .as_number()
            .ok_or_else(|| VMError::runtime("rem. expects numbers"))?;
        Ok(Value::number(a % b))
    });

    builtin(vm, "mod", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("mod expects two numbers"));
        }
        let a = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("mod expects numbers"))?;
        let b = args[1]
            .as_number()
            .ok_or_else(|| VMError::runtime("mod expects numbers"))?;
        Ok(Value::number(((a % b) + b) % b))
    });

    builtin(vm, "mod.", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("mod. expects two numbers"));
        }
        let a = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("mod. expects numbers"))?;
        let b = args[1]
            .as_number()
            .ok_or_else(|| VMError::runtime("mod. expects numbers"))?;
        Ok(Value::number(((a % b) + b) % b))
    });

    builtin(vm, "abs", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("abs expects one number"));
        }
        let n = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("abs expects one number"))?;
        Ok(Value::number(n.abs()))
    });

    builtin(vm, "abs.", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("abs. expects one number"));
        }
        let n = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("abs. expects one number"))?;
        Ok(Value::number(n.abs()))
    });

    builtin(vm, "sign", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("sign expects one number"));
        }
        let n = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("sign expects one number"))?;
        Ok(Value::number(if n > 0.0 {
            1.0
        } else if n < 0.0 {
            -1.0
        } else {
            0.0
        }))
    });

    builtin(vm, "sign.", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("sign. expects one number"));
        }
        let n = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("sign. expects one number"))?;
        Ok(Value::number(if n > 0.0 {
            1.0
        } else if n < 0.0 {
            -1.0
        } else {
            0.0
        }))
    });

    // Math builtins — Float functions
    builtin(vm, "pow", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("pow expects two numbers"));
        }
        let base = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("pow expects numbers"))?;
        let exp = args[1]
            .as_number()
            .ok_or_else(|| VMError::runtime("pow expects numbers"))?;
        Ok(Value::number(base.powf(exp)))
    });

    builtin(vm, "sqrt", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("sqrt expects one number"));
        }
        let n = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("sqrt expects a number"))?;
        Ok(Value::number(n.sqrt()))
    });

    builtin(vm, "exp", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("exp expects one number"));
        }
        let n = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("exp expects a number"))?;
        Ok(Value::number(n.exp()))
    });

    builtin(vm, "log", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("log expects one number"));
        }
        let n = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("log expects a number"))?;
        Ok(Value::number(n.ln()))
    });

    builtin(vm, "log10", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("log10 expects one number"));
        }
        let n = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("log10 expects a number"))?;
        Ok(Value::number(n.log10()))
    });

    builtin(vm, "log2", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("log2 expects one number"));
        }
        let n = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("log2 expects a number"))?;
        Ok(Value::number(n.log2()))
    });

    builtin(vm, "sin", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("sin expects one number"));
        }
        let n = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("sin expects a number"))?;
        Ok(Value::number(n.sin()))
    });

    builtin(vm, "cos", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("cos expects one number"));
        }
        let n = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("cos expects a number"))?;
        Ok(Value::number(n.cos()))
    });

    builtin(vm, "tan", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("tan expects one number"));
        }
        let n = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("tan expects a number"))?;
        Ok(Value::number(n.tan()))
    });

    builtin(vm, "asin", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("asin expects one number"));
        }
        let n = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("asin expects a number"))?;
        Ok(Value::number(n.asin()))
    });

    builtin(vm, "acos", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("acos expects one number"));
        }
        let n = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("acos expects a number"))?;
        Ok(Value::number(n.acos()))
    });

    builtin(vm, "atan", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("atan expects one number"));
        }
        let n = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("atan expects a number"))?;
        Ok(Value::number(n.atan()))
    });

    builtin(vm, "sinh", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("sinh expects one number"));
        }
        let n = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("sinh expects a number"))?;
        Ok(Value::number(n.sinh()))
    });

    builtin(vm, "cosh", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("cosh expects one number"));
        }
        let n = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("cosh expects a number"))?;
        Ok(Value::number(n.cosh()))
    });

    builtin(vm, "tanh", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("tanh expects one number"));
        }
        let n = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("tanh expects a number"))?;
        Ok(Value::number(n.tanh()))
    });

    builtin(vm, "atan2", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("atan2 expects two numbers"));
        }
        let y = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("atan2 expects numbers"))?;
        let x = args[1]
            .as_number()
            .ok_or_else(|| VMError::runtime("atan2 expects numbers"))?;
        Ok(Value::number(y.atan2(x)))
    });

    // Conversion builtins — Float -> Int
    builtin(vm, "floor", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("floor expects one number"));
        }
        let n = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("floor expects a number"))?;
        Ok(Value::number(n.floor()))
    });

    builtin(vm, "ceil", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("ceil expects one number"));
        }
        let n = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("ceil expects a number"))?;
        Ok(Value::number(n.ceil()))
    });

    builtin(vm, "round", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("round expects one number"));
        }
        let n = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("round expects a number"))?;
        Ok(Value::number(n.round()))
    });

    builtin(vm, "truncate", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("truncate expects one number"));
        }
        let n = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("truncate expects a number"))?;
        Ok(Value::number(n.trunc()))
    });

    // Float check builtins
    builtin(vm, "is-nan", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("is-nan expects one number"));
        }
        let n = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("is-nan expects a number"))?;
        Ok(Value::boolean(n.is_nan()))
    });

    builtin(vm, "is-infinite", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("is-infinite expects one number"));
        }
        let n = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("is-infinite expects a number"))?;
        Ok(Value::boolean(n.is_infinite()))
    });

    builtin(vm, "is-finite", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("is-finite expects one number"));
        }
        let n = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("is-finite expects a number"))?;
        Ok(Value::boolean(n.is_finite()))
    });

    // Float comparison builtins
    builtin(vm, "=.", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("=. expects two numbers"));
        }
        let a = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("=. expects two numbers"))?;
        let b = args[1]
            .as_number()
            .ok_or_else(|| VMError::runtime("=. expects two numbers"))?;
        Ok(Value::boolean(a == b))
    });

    builtin(vm, "<.", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("<. expects two numbers"));
        }
        let a = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("<. expects two numbers"))?;
        let b = args[1]
            .as_number()
            .ok_or_else(|| VMError::runtime("<. expects two numbers"))?;
        Ok(Value::boolean(a < b))
    });

    builtin(vm, ">.", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime(">. expects two numbers"));
        }
        let a = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime(">. expects two numbers"))?;
        let b = args[1]
            .as_number()
            .ok_or_else(|| VMError::runtime(">. expects two numbers"))?;
        Ok(Value::boolean(a > b))
    });

    builtin(vm, "<=.", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("<=. expects two numbers"));
        }
        let a = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("<=. expects two numbers"))?;
        let b = args[1]
            .as_number()
            .ok_or_else(|| VMError::runtime("<=. expects two numbers"))?;
        Ok(Value::boolean(a <= b))
    });

    builtin(vm, ">=.", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime(">=. expects two numbers"));
        }
        let a = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime(">=. expects two numbers"))?;
        let b = args[1]
            .as_number()
            .ok_or_else(|| VMError::runtime(">=. expects two numbers"))?;
        Ok(Value::boolean(a >= b))
    });

    // String comparison builtins
    builtin(vm, "string=", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("string= expects two strings"));
        }
        let a = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("string= expects two strings"))?;
        let b = args[1]
            .as_string()
            .ok_or_else(|| VMError::runtime("string= expects two strings"))?;
        Ok(Value::boolean(a == b))
    });

    builtin(vm, "string<", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("string< expects two strings"));
        }
        let a = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("string< expects two strings"))?;
        let b = args[1]
            .as_string()
            .ok_or_else(|| VMError::runtime("string< expects two strings"))?;
        Ok(Value::boolean(a < b))
    });

    builtin(vm, "string>", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("string> expects two strings"));
        }
        let a = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("string> expects two strings"))?;
        let b = args[1]
            .as_string()
            .ok_or_else(|| VMError::runtime("string> expects two strings"))?;
        Ok(Value::boolean(a > b))
    });

    builtin(vm, "string<=", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("string<= expects two strings"));
        }
        let a = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("string<= expects two strings"))?;
        let b = args[1]
            .as_string()
            .ok_or_else(|| VMError::runtime("string<= expects two strings"))?;
        Ok(Value::boolean(a <= b))
    });

    builtin(vm, "string>=", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("string>= expects two strings"));
        }
        let a = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("string>= expects two strings"))?;
        let b = args[1]
            .as_string()
            .ok_or_else(|| VMError::runtime("string>= expects two strings"))?;
        Ok(Value::boolean(a >= b))
    });

    // Polymorphic equality
    builtin(vm, "equal?", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("equal? expects two arguments"));
        }
        Ok(Value::boolean(args[0] == args[1]))
    });

    // Conversion
    builtin(vm, "int->float", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("int->float expects one argument"));
        }
        Ok(args[0].clone())
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

    builtin(vm, "string-is-empty", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("string-is-empty expects 1 argument"));
        }
        let s = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("string-is-empty expects a string"))?;
        Ok(Value::boolean(s.is_empty()))
    });

    builtin(vm, "string-reverse", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("string-reverse expects 1 argument"));
        }
        let s = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("string-reverse expects a string"))?;
        let reversed: String = s.chars().rev().collect();
        Ok(Value::string(reversed.as_str()))
    });

    builtin(vm, "string-repeat", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("string-repeat expects 2 arguments"));
        }
        let s = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("string-repeat: first arg must be a string"))?;
        let n = args[1]
            .as_number()
            .ok_or_else(|| VMError::runtime("string-repeat: second arg must be a number"))?;
        let n = n as usize;
        Ok(Value::string(s.repeat(n).as_str()))
    });

    builtin(vm, "string-to-upper", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("string-to-upper expects 1 argument"));
        }
        let s = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("string-to-upper expects a string"))?;
        Ok(Value::string(s.to_uppercase().as_str()))
    });

    builtin(vm, "string-to-lower", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("string-to-lower expects 1 argument"));
        }
        let s = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("string-to-lower expects a string"))?;
        Ok(Value::string(s.to_lowercase().as_str()))
    });

    builtin(vm, "string-trim", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("string-trim expects 1 argument"));
        }
        let s = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("string-trim expects a string"))?;
        Ok(Value::string(s.trim()))
    });

    builtin(vm, "string-trim-start", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("string-trim-start expects 1 argument"));
        }
        let s = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("string-trim-start expects a string"))?;
        Ok(Value::string(s.trim_start()))
    });

    builtin(vm, "string-trim-end", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("string-trim-end expects 1 argument"));
        }
        let s = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("string-trim-end expects a string"))?;
        Ok(Value::string(s.trim_end()))
    });

    builtin(vm, "string-starts-with", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("string-starts-with expects 2 arguments"));
        }
        let s = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("string-starts-with: first arg must be a string"))?;
        let prefix = args[1]
            .as_string()
            .ok_or_else(|| VMError::runtime("string-starts-with: second arg must be a string"))?;
        Ok(Value::boolean(s.starts_with(prefix)))
    });

    builtin(vm, "string-ends-with", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("string-ends-with expects 2 arguments"));
        }
        let s = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("string-ends-with: first arg must be a string"))?;
        let suffix = args[1]
            .as_string()
            .ok_or_else(|| VMError::runtime("string-ends-with: second arg must be a string"))?;
        Ok(Value::boolean(s.ends_with(suffix)))
    });

    builtin(vm, "string-contains", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("string-contains expects 2 arguments"));
        }
        let s = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("string-contains: first arg must be a string"))?;
        let sub = args[1]
            .as_string()
            .ok_or_else(|| VMError::runtime("string-contains: second arg must be a string"))?;
        Ok(Value::boolean(s.contains(sub)))
    });

    builtin(vm, "string-index-of", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("string-index-of expects 2 arguments"));
        }
        let s = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("string-index-of: first arg must be a string"))?;
        let sub = args[1]
            .as_string()
            .ok_or_else(|| VMError::runtime("string-index-of: second arg must be a string"))?;
        match s.find(sub) {
            Some(byte_idx) => {
                // Convert byte index to char index
                let char_idx = s[..byte_idx].chars().count();
                Ok(Value::object(VMObject::AdtValue {
                    tag: "Some".to_string(),
                    fields: vec![Value::number(char_idx as f64)],
                }))
            }
            None => Ok(Value::object(VMObject::AdtValue {
                tag: "None".to_string(),
                fields: vec![],
            })),
        }
    });

    builtin(vm, "string-substring", |_vm, args| {
        if args.len() != 3 {
            return Err(VMError::runtime("string-substring expects 3 arguments"));
        }
        let s = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("string-substring: first arg must be a string"))?;
        let start = args[1]
            .as_number()
            .ok_or_else(|| VMError::runtime("string-substring: second arg must be a number"))?
            as usize;
        let end = args[2]
            .as_number()
            .ok_or_else(|| VMError::runtime("string-substring: third arg must be a number"))?
            as usize;
        // Work with char indices, not byte indices
        let chars: Vec<char> = s.chars().collect();
        let start = start.min(chars.len());
        let end = end.min(chars.len());
        let result: String = chars[start..end].iter().collect();
        Ok(Value::string(result.as_str()))
    });

    builtin(vm, "string-char-at", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("string-char-at expects 2 arguments"));
        }
        let s = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("string-char-at: first arg must be a string"))?;
        let idx = args[1]
            .as_number()
            .ok_or_else(|| VMError::runtime("string-char-at: second arg must be a number"))?
            as usize;
        match s.chars().nth(idx) {
            Some(c) => Ok(Value::object(VMObject::AdtValue {
                tag: "Some".to_string(),
                fields: vec![Value::char(c)],
            })),
            None => Ok(Value::object(VMObject::AdtValue {
                tag: "None".to_string(),
                fields: vec![],
            })),
        }
    });

    builtin(vm, "string-split", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("string-split expects 2 arguments"));
        }
        let s = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("string-split: first arg must be a string"))?;
        let sep = args[1]
            .as_string()
            .ok_or_else(|| VMError::runtime("string-split: second arg must be a string"))?;
        // Build a Cons-list from the split parts
        let parts: Vec<&str> = s.split(sep).collect();
        let mut result = Value::object(VMObject::AdtValue {
            tag: "Nil".to_string(),
            fields: vec![],
        });
        for part in parts.into_iter().rev() {
            result = Value::object(VMObject::AdtValue {
                tag: "Cons".to_string(),
                fields: vec![Value::string(part), result],
            });
        }
        Ok(result)
    });

    builtin(vm, "string-join", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("string-join expects 2 arguments"));
        }
        let sep = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("string-join: first arg must be a string"))?
            .to_string();
        // Walk the Cons-list
        let mut parts: Vec<String> = Vec::new();
        let mut current = &args[1];
        loop {
            match current.as_object() {
                Some(VMObject::AdtValue { tag, fields }) if tag == "Cons" && fields.len() == 2 => {
                    let s = fields[0].as_string().ok_or_else(|| {
                        VMError::runtime("string-join: list elements must be strings")
                    })?;
                    parts.push(s.to_string());
                    current = &fields[1];
                }
                Some(VMObject::AdtValue { tag, fields }) if tag == "Nil" && fields.is_empty() => {
                    break;
                }
                _ => {
                    return Err(VMError::runtime(
                        "string-join: second arg must be a list of strings",
                    ));
                }
            }
        }
        Ok(Value::string(parts.join(&sep).as_str()))
    });

    builtin(vm, "string-replace", |_vm, args| {
        if args.len() != 3 {
            return Err(VMError::runtime("string-replace expects 3 arguments"));
        }
        let s = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("string-replace: first arg must be a string"))?;
        let from = args[1]
            .as_string()
            .ok_or_else(|| VMError::runtime("string-replace: second arg must be a string"))?;
        let to = args[2]
            .as_string()
            .ok_or_else(|| VMError::runtime("string-replace: third arg must be a string"))?;
        Ok(Value::string(s.replacen(from, to, 1).as_str()))
    });

    builtin(vm, "string-replace-all", |_vm, args| {
        if args.len() != 3 {
            return Err(VMError::runtime("string-replace-all expects 3 arguments"));
        }
        let s = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("string-replace-all: first arg must be a string"))?;
        let from = args[1]
            .as_string()
            .ok_or_else(|| VMError::runtime("string-replace-all: second arg must be a string"))?;
        let to = args[2]
            .as_string()
            .ok_or_else(|| VMError::runtime("string-replace-all: third arg must be a string"))?;
        Ok(Value::string(s.replace(from, to).as_str()))
    });

    builtin(vm, "string-pad-start", |_vm, args| {
        if args.len() != 3 {
            return Err(VMError::runtime("string-pad-start expects 3 arguments"));
        }
        let s = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("string-pad-start: first arg must be a string"))?;
        let target_len = args[1]
            .as_number()
            .ok_or_else(|| VMError::runtime("string-pad-start: second arg must be a number"))?
            as usize;
        let pad = args[2]
            .as_string()
            .ok_or_else(|| VMError::runtime("string-pad-start: third arg must be a string"))?;
        let current_len = s.chars().count();
        if current_len >= target_len {
            Ok(Value::string(s))
        } else {
            let pad_chars: Vec<char> = pad.chars().collect();
            if pad_chars.is_empty() {
                return Ok(Value::string(s));
            }
            let needed = target_len - current_len;
            let mut prefix = String::new();
            for i in 0..needed {
                prefix.push(pad_chars[i % pad_chars.len()]);
            }
            prefix.push_str(s);
            Ok(Value::string(prefix.as_str()))
        }
    });

    builtin(vm, "string-pad-end", |_vm, args| {
        if args.len() != 3 {
            return Err(VMError::runtime("string-pad-end expects 3 arguments"));
        }
        let s = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("string-pad-end: first arg must be a string"))?;
        let target_len = args[1]
            .as_number()
            .ok_or_else(|| VMError::runtime("string-pad-end: second arg must be a number"))?
            as usize;
        let pad = args[2]
            .as_string()
            .ok_or_else(|| VMError::runtime("string-pad-end: third arg must be a string"))?;
        let current_len = s.chars().count();
        if current_len >= target_len {
            Ok(Value::string(s))
        } else {
            let pad_chars: Vec<char> = pad.chars().collect();
            if pad_chars.is_empty() {
                return Ok(Value::string(s));
            }
            let needed = target_len - current_len;
            let mut result = s.to_string();
            for i in 0..needed {
                result.push(pad_chars[i % pad_chars.len()]);
            }
            Ok(Value::string(result.as_str()))
        }
    });

    // Char builtins
    builtin(vm, "char-code", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("char-code expects 1 argument"));
        }
        let c = args[0]
            .as_char()
            .ok_or_else(|| VMError::runtime("char-code expects a char"))?;
        Ok(Value::number(c as u32 as f64))
    });

    builtin(vm, "char-from-code", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("char-from-code expects 1 argument"));
        }
        let n = args[0]
            .as_number()
            .ok_or_else(|| VMError::runtime("char-from-code expects a number"))?;
        let codepoint = n as u32;
        match char::from_u32(codepoint) {
            Some(c) => Ok(Value::char(c)),
            None => Err(VMError::runtime(format!(
                "char-from-code: invalid codepoint {}",
                codepoint
            ))),
        }
    });

    builtin(vm, "char->string", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("char->string expects 1 argument"));
        }
        let c = args[0]
            .as_char()
            .ok_or_else(|| VMError::runtime("char->string expects a char"))?;
        Ok(Value::string(c.to_string().as_str()))
    });

    builtin(vm, "chars->string", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("chars->string expects 1 argument"));
        }
        // Walk the Cons-list of chars
        let mut result = String::new();
        let mut current = &args[0];
        loop {
            match current.as_object() {
                Some(VMObject::AdtValue { tag, fields }) if tag == "Cons" && fields.len() == 2 => {
                    let c = fields[0].as_char().ok_or_else(|| {
                        VMError::runtime("chars->string: list elements must be chars")
                    })?;
                    result.push(c);
                    current = &fields[1];
                }
                Some(VMObject::AdtValue { tag, fields }) if tag == "Nil" && fields.is_empty() => {
                    break;
                }
                _ => {
                    return Err(VMError::runtime(
                        "chars->string: argument must be a list of chars",
                    ));
                }
            }
        }
        Ok(Value::string(result.as_str()))
    });

    builtin(vm, "string->chars", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("string->chars expects 1 argument"));
        }
        let s = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("string->chars expects a string"))?;
        // Build a Cons-list of chars
        let chars: Vec<char> = s.chars().collect();
        let mut result = Value::object(VMObject::AdtValue {
            tag: "Nil".to_string(),
            fields: vec![],
        });
        for c in chars.into_iter().rev() {
            result = Value::object(VMObject::AdtValue {
                tag: "Cons".to_string(),
                fields: vec![Value::char(c), result],
            });
        }
        Ok(result)
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

    builtin(vm, "__try-string->number", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("__try-string->number expects 1 argument"));
        }
        let s = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("__try-string->number expects a string"))?;
        match s.parse::<f64>() {
            Ok(n) => Ok(Value::object(VMObject::AdtValue {
                tag: "Some".to_string(),
                fields: vec![Value::number(n)],
            })),
            Err(_) => Ok(Value::object(VMObject::AdtValue {
                tag: "None".to_string(),
                fields: vec![],
            })),
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
        Ok(Value::boolean(matches!(
            args[0].as_object(),
            Some(VMObject::Symbol(_))
        )))
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
        let is_nil = args[0].is_nil()
            || matches!(
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
                ));
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
                ));
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
                ));
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
