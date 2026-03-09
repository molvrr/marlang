use crate::errors::VMError;
use crate::object::{VMObject, make_err, make_ok};
use crate::value::{Value, format_value};
use crate::vm::VM;

/// Type alias for a builtin function pointer.
type BuiltinFn = fn(&mut VM, &[Value]) -> Result<Value, VMError>;

/// Type alias for the builtin registration function.
type RegisterFn = fn(&mut VM, &str, BuiltinFn);

/// Register a unary int builtin: extracts one i64, applies `$op`, returns Value::int.
/// Type checker guarantees the argument is Int, so we use unchecked extraction.
macro_rules! unary_int {
    ($vm:expr, $builtin:ident, $name:expr, $op:expr) => {
        $builtin($vm, $name, |_vm, args| {
            debug_assert!(args.len() == 1, concat!($name, " expects one integer"));
            Ok(Value::int($op(args[0].get_int())))
        });
    };
}

/// Register a unary float builtin: extracts one f64, applies `$op`, returns Value::float.
/// Type checker guarantees the argument is Float.
macro_rules! unary_float {
    ($vm:expr, $builtin:ident, $name:expr, $op:expr) => {
        $builtin($vm, $name, |_vm, args| {
            debug_assert!(args.len() == 1, concat!($name, " expects one float"));
            Ok(Value::float($op(args[0].get_float())))
        });
    };
}

/// Register a binary int comparison: extracts two i64, applies `$op`, returns Value::boolean.
macro_rules! bin_int_cmp {
    ($vm:expr, $builtin:ident, $name:expr, $op:expr) => {
        $builtin($vm, $name, |_vm, args| {
            debug_assert!(args.len() == 2, concat!($name, " expects two integers"));
            Ok(Value::boolean($op(args[0].get_int(), args[1].get_int())))
        });
    };
}

/// Register a binary float comparison: extracts two f64, applies `$op`, returns Value::boolean.
macro_rules! bin_float_cmp {
    ($vm:expr, $builtin:ident, $name:expr, $op:expr) => {
        $builtin($vm, $name, |_vm, args| {
            debug_assert!(args.len() == 2, concat!($name, " expects two floats"));
            Ok(Value::boolean($op(
                args[0].get_float(),
                args[1].get_float(),
            )))
        });
    };
}

/// Register a binary string comparison: extracts two strings, applies `$op`, returns Value::boolean.
macro_rules! bin_str_cmp {
    ($vm:expr, $builtin:ident, $name:expr, $op:expr) => {
        $builtin($vm, $name, |_vm, args| {
            if args.len() != 2 {
                return Err(VMError::runtime(concat!($name, " expects two strings")));
            }
            let a = args[0]
                .as_string()
                .ok_or_else(|| VMError::runtime(concat!($name, " expects two strings")))?;
            let b = args[1]
                .as_string()
                .ok_or_else(|| VMError::runtime(concat!($name, " expects two strings")))?;
            Ok(Value::boolean($op(a, b)))
        });
    };
}

/// Register a unary float predicate: extracts one f64, applies `$pred`, returns Value::boolean.
macro_rules! unary_float_pred {
    ($vm:expr, $builtin:ident, $name:expr, $pred:expr) => {
        $builtin($vm, $name, |_vm, args| {
            debug_assert!(args.len() == 1, concat!($name, " expects one float"));
            Ok(Value::boolean($pred(args[0].get_float())))
        });
    };
}

/// Register a unary string transform: extracts one string, applies `$transform`, returns Value::string.
macro_rules! unary_str_transform {
    ($vm:expr, $builtin:ident, $name:expr, $transform:expr) => {
        $builtin($vm, $name, |_vm, args| {
            if args.len() != 1 {
                return Err(VMError::runtime(concat!($name, " expects 1 argument")));
            }
            let s = args[0]
                .as_string()
                .ok_or_else(|| VMError::runtime(concat!($name, " expects a string")))?;
            let result: String = $transform(s);
            Ok(Value::string(&*result))
        });
    };
}

/// Register a binary int op: extracts two i64, applies `$op`, returns Value::int.
macro_rules! bin_int_op {
    ($vm:expr, $builtin:ident, $name:expr, $op:expr) => {
        $builtin($vm, $name, |_vm, args| {
            debug_assert!(args.len() == 2, concat!($name, " expects two integers"));
            Ok(Value::int($op(args[0].get_int(), args[1].get_int())))
        });
    };
}

/// Register a binary float op: extracts two f64, applies `$op`, returns Value::float.
macro_rules! bin_float_op {
    ($vm:expr, $builtin:ident, $name:expr, $op:expr) => {
        $builtin($vm, $name, |_vm, args| {
            debug_assert!(args.len() == 2, concat!($name, " expects two floats"));
            Ok(Value::float($op(args[0].get_float(), args[1].get_float())))
        });
    };
}

/// Collect a Cons/Nil ADT chain into a Vec<Value>.
/// Returns Err if the value is not a list.
fn collect_list(val: &Value, context: &str) -> Result<Vec<Value>, VMError> {
    let mut result = Vec::new();
    let mut cur = val.clone();
    loop {
        if cur.is_nil() {
            return Ok(result);
        }
        match cur.as_object() {
            Some(VMObject::AdtValue { tag, fields }) if tag == "Nil" => {
                return Ok(result);
            }
            Some(VMObject::AdtValue { tag, fields }) if tag == "Cons" && fields.len() == 2 => {
                result.push(fields[0].clone());
                cur = fields[1].clone();
            }
            _ => {
                return Err(VMError::runtime(format!(
                    "{}: expected a list (Cons/Nil)",
                    context
                )));
            }
        }
    }
}

/// Register all built-in globals into the VM's root module.
pub fn register_builtins(vm: &mut VM) {
    // --- Built-in values ---
    vm.globals_mut().insert("nil".to_string(), Value::nil());
    vm.globals_mut()
        .insert("#t".to_string(), Value::boolean(true));
    vm.globals_mut()
        .insert("#f".to_string(), Value::boolean(false));

    // --- Built-in functions ---

    fn builtin(vm: &mut VM, name: &str, func: BuiltinFn) {
        vm.globals_mut().insert(
            name.to_string(),
            Value::object(VMObject::Builtin {
                name: name.to_string(),
                func,
            }),
        );
    }

    // Variadic integer arithmetic (for non-binary calls, e.g. (+ 1 2 3))
    builtin(vm, "+", |_vm, args| {
        let mut sum: i64 = 0;
        for a in args {
            sum = sum.wrapping_add(a.get_int());
        }
        Ok(Value::int(sum))
    });

    builtin(vm, "-", |_vm, args| {
        debug_assert!(!args.is_empty(), "- expects at least one argument");
        let first = args[0].get_int();
        if args.len() == 1 {
            return Ok(Value::int(first.wrapping_neg()));
        }
        let mut result = first;
        for a in &args[1..] {
            result = result.wrapping_sub(a.get_int());
        }
        Ok(Value::int(result))
    });

    builtin(vm, "*", |_vm, args| {
        let mut product: i64 = 1;
        for a in args {
            product = product.wrapping_mul(a.get_int());
        }
        Ok(Value::int(product))
    });

    builtin(vm, "/", |_vm, args| {
        debug_assert!(args.len() >= 2, "/ expects at least two arguments");
        let mut result = args[0].get_int();
        for a in &args[1..] {
            let divisor = a.get_int();
            if divisor == 0 {
                return Err(VMError::runtime("Division by zero"));
            }
            result = result.wrapping_div(divisor);
        }
        Ok(Value::int(result))
    });

    unary_int!(vm, builtin, "negate", |n: i64| n.wrapping_neg());

    // Integer comparison (for non-binary calls)
    bin_int_cmp!(vm, builtin, "<", |a: i64, b: i64| a < b);
    bin_int_cmp!(vm, builtin, ">", |a: i64, b: i64| a > b);
    bin_int_cmp!(vm, builtin, "=", |a: i64, b: i64| a == b);
    bin_int_cmp!(vm, builtin, "<=", |a: i64, b: i64| a <= b);
    bin_int_cmp!(vm, builtin, ">=", |a: i64, b: i64| a >= b);

    // Variadic float arithmetic
    builtin(vm, "+.", |_vm, args| {
        let mut sum = 0.0_f64;
        for a in args {
            sum += a.get_float();
        }
        Ok(Value::float(sum))
    });

    builtin(vm, "-.", |_vm, args| {
        debug_assert!(!args.is_empty(), "-. expects at least one argument");
        let first = args[0].get_float();
        if args.len() == 1 {
            return Ok(Value::float(-first));
        }
        let mut result = first;
        for a in &args[1..] {
            result -= a.get_float();
        }
        Ok(Value::float(result))
    });

    builtin(vm, "*.", |_vm, args| {
        let mut product = 1.0_f64;
        for a in args {
            product *= a.get_float();
        }
        Ok(Value::float(product))
    });

    builtin(vm, "/.", |_vm, args| {
        debug_assert!(args.len() >= 2, "/. expects at least two arguments");
        let mut result = args[0].get_float();
        for a in &args[1..] {
            result /= a.get_float();
        }
        Ok(Value::float(result))
    });

    unary_float!(vm, builtin, "negate.", |n: f64| -n);

    // Integer math builtins
    bin_int_op!(vm, builtin, "rem", |a: i64, b: i64| a % b);
    bin_int_op!(vm, builtin, "mod", |a: i64, b: i64| ((a % b) + b) % b);
    unary_int!(vm, builtin, "abs", |n: i64| n.wrapping_abs());

    fn sign_int(n: i64) -> i64 {
        if n > 0 {
            1
        } else if n < 0 {
            -1
        } else {
            0
        }
    }
    unary_int!(vm, builtin, "sign", sign_int);

    // Float math builtins
    bin_float_op!(vm, builtin, "rem.", |a: f64, b: f64| a % b);
    bin_float_op!(vm, builtin, "mod.", |a: f64, b: f64| ((a % b) + b) % b);
    unary_float!(vm, builtin, "abs.", f64::abs);

    fn sign_float(n: f64) -> f64 {
        if n > 0.0 {
            1.0
        } else if n < 0.0 {
            -1.0
        } else {
            0.0
        }
    }
    unary_float!(vm, builtin, "sign.", sign_float);

    // Math builtins — Float-only functions
    bin_float_op!(vm, builtin, "pow", f64::powf);
    unary_float!(vm, builtin, "sqrt", f64::sqrt);
    unary_float!(vm, builtin, "exp", f64::exp);
    unary_float!(vm, builtin, "log", f64::ln);
    unary_float!(vm, builtin, "log10", f64::log10);
    unary_float!(vm, builtin, "log2", f64::log2);
    unary_float!(vm, builtin, "sin", f64::sin);
    unary_float!(vm, builtin, "cos", f64::cos);
    unary_float!(vm, builtin, "tan", f64::tan);
    unary_float!(vm, builtin, "asin", f64::asin);
    unary_float!(vm, builtin, "acos", f64::acos);
    unary_float!(vm, builtin, "atan", f64::atan);
    unary_float!(vm, builtin, "sinh", f64::sinh);
    unary_float!(vm, builtin, "cosh", f64::cosh);
    unary_float!(vm, builtin, "tanh", f64::tanh);
    bin_float_op!(vm, builtin, "atan2", f64::atan2);

    // Conversion builtins — Float -> Int (return Int)
    builtin(vm, "floor", |_vm, args| {
        debug_assert!(args.len() == 1);
        Ok(Value::int(args[0].get_float().floor() as i64))
    });
    builtin(vm, "ceil", |_vm, args| {
        debug_assert!(args.len() == 1);
        Ok(Value::int(args[0].get_float().ceil() as i64))
    });
    builtin(vm, "round", |_vm, args| {
        debug_assert!(args.len() == 1);
        Ok(Value::int(args[0].get_float().round() as i64))
    });
    builtin(vm, "truncate", |_vm, args| {
        debug_assert!(args.len() == 1);
        Ok(Value::int(args[0].get_float().trunc() as i64))
    });

    // Float check builtins
    unary_float_pred!(vm, builtin, "is-nan", f64::is_nan);
    unary_float_pred!(vm, builtin, "is-infinite", f64::is_infinite);
    unary_float_pred!(vm, builtin, "is-finite", f64::is_finite);

    // Float comparison builtins
    bin_float_cmp!(vm, builtin, "=.", |a: f64, b: f64| a == b);
    bin_float_cmp!(vm, builtin, "<.", |a: f64, b: f64| a < b);
    bin_float_cmp!(vm, builtin, ">.", |a: f64, b: f64| a > b);
    bin_float_cmp!(vm, builtin, "<=.", |a: f64, b: f64| a <= b);
    bin_float_cmp!(vm, builtin, ">=.", |a: f64, b: f64| a >= b);

    // String comparison builtins
    bin_str_cmp!(vm, builtin, "string=", |a: &str, b: &str| a == b);
    bin_str_cmp!(vm, builtin, "string<", |a: &str, b: &str| a < b);
    bin_str_cmp!(vm, builtin, "string>", |a: &str, b: &str| a > b);
    bin_str_cmp!(vm, builtin, "string<=", |a: &str, b: &str| a <= b);
    bin_str_cmp!(vm, builtin, "string>=", |a: &str, b: &str| a >= b);

    // Polymorphic equality
    builtin(vm, "equal?", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("equal? expects two arguments"));
        }
        Ok(Value::boolean(args[0] == args[1]))
    });

    // Conversion: Int -> Float
    builtin(vm, "int->float", |_vm, args| {
        debug_assert!(args.len() == 1, "int->float expects one argument");
        Ok(Value::float(args[0].get_int() as f64))
    });

    // List
    builtin(vm, "list", |_vm, args| {
        Ok(crate::object::vec_to_cons_list(args.to_vec()))
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
        Ok(Value::int(s.len() as i64))
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

    unary_str_transform!(vm, builtin, "string-reverse", |s: &str| s
        .chars()
        .rev()
        .collect::<String>(
    ));

    builtin(vm, "string-repeat", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("string-repeat expects 2 arguments"));
        }
        let s = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("string-repeat: first arg must be a string"))?;
        let n = args[1].get_int() as usize;
        Ok(Value::string(s.repeat(n).as_str()))
    });

    unary_str_transform!(vm, builtin, "string-to-upper", |s: &str| s.to_uppercase());
    unary_str_transform!(vm, builtin, "string-to-lower", |s: &str| s.to_lowercase());
    unary_str_transform!(vm, builtin, "string-trim", |s: &str| s.trim().to_string());
    unary_str_transform!(vm, builtin, "string-trim-start", |s: &str| s
        .trim_start()
        .to_string());
    unary_str_transform!(vm, builtin, "string-trim-end", |s: &str| s
        .trim_end()
        .to_string());

    bin_str_cmp!(vm, builtin, "string-starts-with", |s: &str, p: &str| s
        .starts_with(p));
    bin_str_cmp!(vm, builtin, "string-ends-with", |s: &str, p: &str| s
        .ends_with(p));
    bin_str_cmp!(vm, builtin, "string-contains", |s: &str, p: &str| s
        .contains(p));

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
                    fields: vec![Value::int(char_idx as i64)],
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
        let start = args[1].get_int() as usize;
        let end = args[2].get_int() as usize;
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
        let idx = args[1].get_int() as usize;
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
        let items = collect_list(&args[1], "string-join")?;
        let mut parts: Vec<String> = Vec::new();
        for item in &items {
            let s = item
                .as_string()
                .ok_or_else(|| VMError::runtime("string-join: list elements must be strings"))?;
            parts.push(s.to_string());
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
        let target_len = args[1].get_int() as usize;
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
        let target_len = args[1].get_int() as usize;
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
        Ok(Value::int(c as u32 as i64))
    });

    builtin(vm, "char-from-code", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("char-from-code expects 1 argument"));
        }
        let n = args[0].get_int();
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
        let items = collect_list(&args[0], "chars->string")?;
        let mut result = String::new();
        for item in &items {
            let c = item
                .as_char()
                .ok_or_else(|| VMError::runtime("chars->string: list elements must be chars"))?;
            result.push(c);
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
        let s = if let Some(n) = args[0].as_int() {
            format!("{}", n)
        } else if let Some(n) = args[0].as_float() {
            if n == n.trunc() && !n.is_infinite() && !n.is_nan() {
                format!("{:.1}", n)
            } else {
                format!("{}", n)
            }
        } else {
            return Err(VMError::runtime("number->string expects a number"));
        };
        Ok(Value::string(s.as_str()))
    });

    builtin(vm, "__try-string->int", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("__try-string->int expects 1 argument"));
        }
        let s = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("__try-string->int expects a string"))?;
        match s.parse::<i64>() {
            Ok(n) => Ok(Value::object(VMObject::AdtValue {
                tag: "Some".to_string(),
                fields: vec![Value::int(n)],
            })),
            Err(_) => Ok(Value::object(VMObject::AdtValue {
                tag: "None".to_string(),
                fields: vec![],
            })),
        }
    });

    builtin(vm, "__try-string->float", |_vm, args| {
        if args.len() != 1 {
            return Err(VMError::runtime("__try-string->float expects 1 argument"));
        }
        let s = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("__try-string->float expects a string"))?;
        match s.parse::<f64>() {
            Ok(n) => Ok(Value::object(VMObject::AdtValue {
                tag: "Some".to_string(),
                fields: vec![Value::float(n)],
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

    // (unshadow context-symbol datum-symbol)
    // Copies the hygiene marks from context-symbol onto datum-symbol.
    // Use this in macros that intentionally introduce new bindings visible
    // at the call site (e.g. defrecord generating accessor functions).
    builtin(vm, "unshadow", |_vm, args| {
        if args.len() != 2 {
            return Err(VMError::runtime("unshadow expects 2 arguments"));
        }
        let ctx = match args[0].as_object() {
            Some(VMObject::Symbol(s)) => s.as_str().to_string(),
            _ => {
                return Err(VMError::runtime(
                    "unshadow: first argument must be a symbol",
                ));
            }
        };
        let datum = match args[1].as_object() {
            Some(VMObject::Symbol(s)) => s.as_str().to_string(),
            _ => {
                return Err(VMError::runtime(
                    "unshadow: second argument must be a symbol",
                ));
            }
        };
        // Extract the mark suffix from the context symbol (e.g. "name~1" → "~1")
        let mark_suffix = match ctx.find('~') {
            Some(pos) => {
                let rest = &ctx[pos..];
                if rest[1..].bytes().all(|b| b.is_ascii_digit() || b == b'~') {
                    rest
                } else {
                    ""
                }
            }
            None => "",
        };
        Ok(Value::symbol(format!("{}{}", datum, mark_suffix)))
    });

    // Error
    builtin(vm, "error", |_vm, args| {
        let msg = if let Some(s) = args.first().and_then(|a| a.as_string()) {
            s.to_string()
        } else {
            args.first()
                .map(format_value)
                .unwrap_or_else(|| "error".to_string())
        };
        Err(VMError::runtime(msg))
    });

    // run-fibers and run-fibers-parallel removed — all effects flow through
    // the always-on scheduler. Fibers, channels, actors, IO, TCP, and HTTP
    // effects work at the top level without any wrapper.

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
            fields: vec![result, crate::value::Value::float(elapsed)],
        };
        Ok(crate::value::Value::object(pair))
    });

    // === Bytes builtins ===

    // bytes-new: create zero-filled Bytes of given length
    builtin(vm, "bytes-new", |_vm, args| {
        debug_assert!(args.len() == 1, "bytes-new expects one integer");
        let len = args[0].get_int();
        if len < 0 {
            return Err(VMError::runtime("bytes-new: length must be non-negative"));
        }
        Ok(Value::bytes(vec![0u8; len as usize]))
    });

    // bytes-length: length in bytes
    builtin(vm, "bytes-length", |_vm, args| {
        debug_assert!(args.len() == 1, "bytes-length expects one argument");
        let b = args[0]
            .as_bytes()
            .ok_or_else(|| VMError::runtime("bytes-length: expected Bytes"))?;
        Ok(Value::int(b.len() as i64))
    });

    // bytes-get: get byte at index as Int
    builtin(vm, "bytes-get", |_vm, args| {
        debug_assert!(args.len() == 2, "bytes-get expects two arguments");
        let b = args[0]
            .as_bytes()
            .ok_or_else(|| VMError::runtime("bytes-get: expected Bytes"))?;
        let idx = args[1].get_int();
        if idx < 0 || idx as usize >= b.len() {
            return Err(VMError::runtime(format!(
                "bytes-get: index {} out of bounds (length {})",
                idx,
                b.len()
            )));
        }
        Ok(Value::int(b[idx as usize] as i64))
    });

    // bytes-set: set byte at index, returns new Bytes
    builtin(vm, "bytes-set", |_vm, args| {
        debug_assert!(args.len() == 3, "bytes-set expects three arguments");
        let b = args[0]
            .as_bytes()
            .ok_or_else(|| VMError::runtime("bytes-set: expected Bytes"))?;
        let idx = args[1].get_int();
        let val = args[2].get_int();
        if idx < 0 || idx as usize >= b.len() {
            return Err(VMError::runtime(format!(
                "bytes-set: index {} out of bounds (length {})",
                idx,
                b.len()
            )));
        }
        if !(0..=255).contains(&val) {
            return Err(VMError::runtime(format!(
                "bytes-set: byte value {} out of range (0-255)",
                val
            )));
        }
        let mut new = b.to_vec();
        new[idx as usize] = val as u8;
        Ok(Value::bytes(new))
    });

    // bytes-slice: slice [start, end)
    builtin(vm, "bytes-slice", |_vm, args| {
        debug_assert!(args.len() == 3, "bytes-slice expects three arguments");
        let b = args[0]
            .as_bytes()
            .ok_or_else(|| VMError::runtime("bytes-slice: expected Bytes"))?;
        let start = args[1].get_int();
        let end = args[2].get_int();
        if start < 0 || end < 0 || start as usize > b.len() || end as usize > b.len() || start > end
        {
            return Err(VMError::runtime(format!(
                "bytes-slice: invalid range [{}, {}) for length {}",
                start,
                end,
                b.len()
            )));
        }
        Ok(Value::bytes(b[start as usize..end as usize].to_vec()))
    });

    // bytes-concat: concatenate two Bytes
    builtin(vm, "bytes-concat", |_vm, args| {
        debug_assert!(args.len() == 2, "bytes-concat expects two arguments");
        let a = args[0]
            .as_bytes()
            .ok_or_else(|| VMError::runtime("bytes-concat: expected Bytes"))?;
        let b_val = args[1]
            .as_bytes()
            .ok_or_else(|| VMError::runtime("bytes-concat: expected Bytes"))?;
        let mut result = Vec::with_capacity(a.len() + b_val.len());
        result.extend_from_slice(a);
        result.extend_from_slice(b_val);
        Ok(Value::bytes(result))
    });

    // bytes-from-string: UTF-8 encode a String to Bytes
    builtin(vm, "bytes-from-string", |_vm, args| {
        debug_assert!(args.len() == 1, "bytes-from-string expects one argument");
        let s = args[0]
            .as_string()
            .ok_or_else(|| VMError::runtime("bytes-from-string: expected String"))?;
        Ok(Value::bytes(s.as_bytes().to_vec()))
    });

    // bytes-to-string: UTF-8 decode Bytes to String, returns (Result String String)
    builtin(vm, "bytes-to-string", |_vm, args| {
        debug_assert!(args.len() == 1, "bytes-to-string expects one argument");
        let b = args[0]
            .as_bytes()
            .ok_or_else(|| VMError::runtime("bytes-to-string: expected Bytes"))?;
        match std::str::from_utf8(b) {
            Ok(s) => Ok(make_ok(Value::string(s))),
            Err(e) => Ok(make_err(&format!("invalid UTF-8: {}", e))),
        }
    });

    // bytes-from-list: convert (List Int) to Bytes
    builtin(vm, "bytes-from-list", |_vm, args| {
        debug_assert!(args.len() == 1, "bytes-from-list expects one argument");
        let items = collect_list(&args[0], "bytes-from-list")?;
        let mut bytes = Vec::with_capacity(items.len());
        for (i, item) in items.iter().enumerate() {
            let v = item
                .as_int()
                .ok_or_else(|| VMError::runtime("bytes-from-list: expected Int elements"))?;
            if !(0..=255).contains(&v) {
                return Err(VMError::runtime(format!(
                    "bytes-from-list: element {} value {} out of range (0-255)",
                    i, v
                )));
            }
            bytes.push(v as u8);
        }
        Ok(Value::bytes(bytes))
    });

    // bytes-to-list: convert Bytes to (List Int)
    builtin(vm, "bytes-to-list", |_vm, args| {
        debug_assert!(args.len() == 1, "bytes-to-list expects one argument");
        let b = args[0]
            .as_bytes()
            .ok_or_else(|| VMError::runtime("bytes-to-list: expected Bytes"))?;
        let items: Vec<Value> = b.iter().map(|&byte| Value::int(byte as i64)).collect();
        Ok(crate::object::vec_to_cons_list(items))
    });

    // --- Parse state builtins ---

    // __parse-push-input: (List Char) -> Unit
    // Push a char list as mutable parse state. ParseState effects (peek, consume,
    // get-input, set-input) are then handled inline by the VM — no continuation
    // capture, no handler recursion.
    builtin(vm, "__parse-push-input", |vm, args| {
        debug_assert!(args.len() == 1, "__parse-push-input expects one argument");
        let chars = crate::vm::list_to_values(&args[0]);
        vm.parse_state_stack
            .push(crate::vm::ParseState { chars, pos: 0 });
        Ok(Value::nil())
    });

    // __parse-pop-input: Unit -> (List Char)
    // Pop the current parse state and return remaining input as a char list.
    builtin(vm, "__parse-pop-input", |vm, _args| {
        match vm.parse_state_stack.pop() {
            Some(crate::vm::ParseState { chars, pos }) => {
                Ok(crate::vm::values_to_list(&chars[pos..]))
            }
            None => Err(VMError::runtime("__parse-pop-input: no active parse state")),
        }
    });

    // FFI builtins
    #[cfg(feature = "ffi")]
    register_ffi_builtins(vm, builtin);
}

#[cfg(feature = "ffi")]
fn register_ffi_builtins(vm: &mut VM, builtin: RegisterFn) {
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
        let sig_entries = collect_list(&args[1], "ffi-open: second arg")?;

        let mut sigs = Vec::new();
        for entry in &sig_entries {
            let parts = collect_list(entry, "ffi-open: each signature")?;
            if parts.len() != 3 {
                return Err(VMError::runtime(
                    "ffi-open: signature must be (name args-list ret-type)",
                ));
            }
            let fn_name = parts[0]
                .as_string()
                .ok_or_else(|| VMError::runtime("ffi-open: fn name must be string"))?;
            let arg_types_val = collect_list(&parts[1], "ffi-open: arg types")?;
            let mut arg_types = Vec::new();
            for at in &arg_types_val {
                let type_str = at
                    .as_string()
                    .ok_or_else(|| VMError::runtime("ffi-open: arg type must be string"))?;
                if type_str == "void" || type_str == "Unit" {
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
        let fields_val = collect_list(&args[1], "ffi-struct: second arg")?;

        let mut fields = Vec::new();
        for f in &fields_val {
            let parts = collect_list(f, "ffi-struct: each field")?;
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
