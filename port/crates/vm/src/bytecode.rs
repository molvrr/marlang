/// Opcodes for the Marlang bytecode VM.
///
/// Numeric values match the TypeScript VM for cross-reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Op {
    // Stack manipulation
    PushConst = 0,
    PushLocal = 1,
    SetLocal = 2,
    PushUpvalue = 3,
    PushGlobal = 4,
    SetGlobal = 5,
    Pop = 6,
    Dup = 7,

    // Arithmetic (binary: pop 2, push 1)
    Add = 10,
    Sub = 11,
    Mul = 12,
    Div = 13,
    Neg = 14, // Unary: pop 1, push 1

    // Comparison (binary: pop 2, push boolean)
    Eq = 20,
    Lt = 21,
    Gt = 22,
    Le = 23,
    Ge = 24,
    Not = 25, // Unary boolean not

    // Control flow
    Jump = 30,
    JumpIfFalse = 31,
    Return = 33,

    // Functions
    Call = 40,
    TailCall = 41,
    Closure = 42,

    // ADTs
    Construct = 50,
    GetField = 51,
    MatchTag = 52,

    // Effects
    PushHandler = 60,
    PopHandler = 61,
    Perform = 62,
    Resume = 63,

    Halt = 99,
}

impl Op {
    pub fn name(&self) -> &'static str {
        match self {
            Op::PushConst => "PUSH_CONST",
            Op::PushLocal => "PUSH_LOCAL",
            Op::SetLocal => "SET_LOCAL",
            Op::PushUpvalue => "PUSH_UPVALUE",
            Op::PushGlobal => "PUSH_GLOBAL",
            Op::SetGlobal => "SET_GLOBAL",
            Op::Pop => "POP",
            Op::Dup => "DUP",
            Op::Add => "ADD",
            Op::Sub => "SUB",
            Op::Mul => "MUL",
            Op::Div => "DIV",
            Op::Neg => "NEG",
            Op::Eq => "EQ",
            Op::Lt => "LT",
            Op::Gt => "GT",
            Op::Le => "LE",
            Op::Ge => "GE",
            Op::Not => "NOT",
            Op::Jump => "JUMP",
            Op::JumpIfFalse => "JUMP_IF_FALSE",
            Op::Return => "RETURN",
            Op::Call => "CALL",
            Op::TailCall => "TAIL_CALL",
            Op::Closure => "CLOSURE",
            Op::Construct => "CONSTRUCT",
            Op::GetField => "GET_FIELD",
            Op::MatchTag => "MATCH_TAG",
            Op::PushHandler => "PUSH_HANDLER",
            Op::PopHandler => "POP_HANDLER",
            Op::Perform => "PERFORM",
            Op::Resume => "RESUME",
            Op::Halt => "HALT",
        }
    }
}

/// A single bytecode instruction.
#[derive(Debug, Clone, Copy)]
pub struct Instruction {
    pub op: Op,
    pub operand: u32,
    pub operand2: u32,
    pub line: u32,
}

impl Instruction {
    pub fn new(op: Op, line: u32) -> Self {
        Instruction {
            op,
            operand: 0,
            operand2: 0,
            line,
        }
    }

    pub fn with_operand(op: Op, line: u32, operand: u32) -> Self {
        Instruction {
            op,
            operand,
            operand2: 0,
            line,
        }
    }

    pub fn with_operands(op: Op, line: u32, operand: u32, operand2: u32) -> Self {
        Instruction {
            op,
            operand,
            operand2,
            line,
        }
    }
}

/// An FFI function signature entry for foreign-lib.
#[derive(Debug, Clone)]
pub struct FfiSigEntry {
    pub name: String,
    pub arg_types: Vec<String>, // C type strings: "i32", "f32", "cstring", etc.
    pub ret_type: String,
}

/// Compile-time constant values stored in a chunk's constant pool.
#[derive(Debug, Clone)]
pub enum ConstantValue {
    Number(f64),
    String(String),
    Boolean(bool),
    Nil,
    Prototype(FunctionPrototype),
    /// FFI function signatures for ffi-open. Converted to nested list at runtime.
    FfiSigs(Vec<FfiSigEntry>),
    /// A bound FFI function reference: lib global name + function name.
    FfiBound {
        lib_name: String,
        fn_name: String,
    },
}

impl ConstantValue {
    /// Check equality for constant deduplication.
    pub fn dedup_eq(&self, other: &ConstantValue) -> bool {
        match (self, other) {
            (ConstantValue::Number(a), ConstantValue::Number(b)) => a == b,
            (ConstantValue::String(a), ConstantValue::String(b)) => a == b,
            (ConstantValue::Boolean(a), ConstantValue::Boolean(b)) => a == b,
            (ConstantValue::Nil, ConstantValue::Nil) => true,
            (
                ConstantValue::FfiBound {
                    lib_name: l1,
                    fn_name: f1,
                },
                ConstantValue::FfiBound {
                    lib_name: l2,
                    fn_name: f2,
                },
            ) => l1 == l2 && f1 == f2,
            _ => false,
        }
    }

    /// Convert to a VM Value. Prototypes become nil.
    pub fn as_value(&self) -> crate::value::Value {
        use crate::object::VMObject;
        use crate::value::Value;
        use std::sync::Arc;

        match self {
            ConstantValue::Number(n) => Value::number(*n),
            ConstantValue::String(s) => Value::string(s.as_str()),
            ConstantValue::Boolean(b) => Value::boolean(*b),
            ConstantValue::Nil => Value::nil(),
            ConstantValue::Prototype(_) => Value::nil(),
            ConstantValue::FfiSigs(sigs) => {
                // Convert to nested list matching ffi-open's expected format:
                // (list (list "FnName" (list "arg1" "arg2") "ret") ...)
                let entries: Vec<Value> = sigs
                    .iter()
                    .map(|sig| {
                        let arg_values: Vec<Value> = sig
                            .arg_types
                            .iter()
                            .map(|a| Value::string(a.as_str()))
                            .collect();
                        Value::object(VMObject::List(vec![
                            Value::string(sig.name.as_str()),
                            Value::object(VMObject::List(arg_values)),
                            Value::string(sig.ret_type.as_str()),
                        ]))
                    })
                    .collect();
                Value::object(VMObject::List(entries))
            }
            ConstantValue::FfiBound { lib_name, fn_name } => Value::object(VMObject::FfiBound {
                lib_global_name: Arc::from(lib_name.as_str()),
                fn_name: Arc::from(fn_name.as_str()),
            }),
        }
    }
}

/// A bytecode chunk — a sequence of instructions plus a constant pool.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub code: Vec<Instruction>,
    pub constants: Vec<ConstantValue>,
    pub local_count: u32,
    pub source_file: Option<String>,
}

impl Chunk {
    pub fn new() -> Self {
        Chunk {
            code: Vec::new(),
            constants: Vec::new(),
            local_count: 0,
            source_file: None,
        }
    }
}

impl Default for Chunk {
    fn default() -> Self {
        Self::new()
    }
}

/// A compiled function template (not yet a closure — no captured upvalues).
#[derive(Debug, Clone)]
pub struct FunctionPrototype {
    pub name: String,
    pub arity: u8,
    pub upvalues: Vec<UpvalueDescriptor>,
    pub chunk: Chunk,
}

/// Describes how an upvalue is captured: from an enclosing local or from an
/// enclosing upvalue.
#[derive(Debug, Clone)]
pub struct UpvalueDescriptor {
    pub is_local: bool,
    pub index: u16,
}
