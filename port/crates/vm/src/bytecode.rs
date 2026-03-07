/// Opcodes for the Marlang bytecode VM.
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
    SetUpvalue = 8,

    // Integer arithmetic (binary: pop 2, push 1)
    AddInt = 10,
    SubInt = 11,
    MulInt = 12,
    DivInt = 13,
    NegInt = 14, // Unary: pop 1, push 1

    // Float arithmetic (binary: pop 2, push 1)
    AddFloat = 15,
    SubFloat = 16,
    MulFloat = 17,
    DivFloat = 18,
    NegFloat = 19, // Unary: pop 1, push 1

    // Comparison — Eq and Not are polymorphic
    Eq = 20,
    Not = 25,

    // Integer comparison (binary: pop 2, push boolean)
    LtInt = 21,
    GtInt = 22,
    LeInt = 23,
    GeInt = 24,

    // Float comparison (binary: pop 2, push boolean)
    LtFloat = 26,
    GtFloat = 27,
    LeFloat = 28,
    GeFloat = 29,

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
            Op::SetUpvalue => "SET_UPVALUE",
            Op::PushGlobal => "PUSH_GLOBAL",
            Op::SetGlobal => "SET_GLOBAL",
            Op::Pop => "POP",
            Op::Dup => "DUP",
            Op::AddInt => "ADD_INT",
            Op::SubInt => "SUB_INT",
            Op::MulInt => "MUL_INT",
            Op::DivInt => "DIV_INT",
            Op::NegInt => "NEG_INT",
            Op::AddFloat => "ADD_FLOAT",
            Op::SubFloat => "SUB_FLOAT",
            Op::MulFloat => "MUL_FLOAT",
            Op::DivFloat => "DIV_FLOAT",
            Op::NegFloat => "NEG_FLOAT",
            Op::Eq => "EQ",
            Op::Not => "NOT",
            Op::LtInt => "LT_INT",
            Op::GtInt => "GT_INT",
            Op::LeInt => "LE_INT",
            Op::GeInt => "GE_INT",
            Op::LtFloat => "LT_FLOAT",
            Op::GtFloat => "GT_FLOAT",
            Op::LeFloat => "LE_FLOAT",
            Op::GeFloat => "GE_FLOAT",
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
    Int(i64),
    Float(f64),
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
            (ConstantValue::Int(a), ConstantValue::Int(b)) => a == b,
            (ConstantValue::Float(a), ConstantValue::Float(b)) => a == b,
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
            ConstantValue::Int(n) => Value::int(*n),
            ConstantValue::Float(n) => Value::float(*n),
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
                        crate::object::vec_to_cons_list(vec![
                            Value::string(sig.name.as_str()),
                            crate::object::vec_to_cons_list(arg_values),
                            Value::string(sig.ret_type.as_str()),
                        ])
                    })
                    .collect();
                crate::object::vec_to_cons_list(entries)
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

/// Module identifier — index into the VM's `module_globals` table.
/// `0` is the root module (builtins + prelude + main program).
pub type ModuleId = u32;

/// The root module ID (builtins, prelude, and the main program).
pub const ROOT_MODULE: ModuleId = 0;

/// A compiled function template (not yet a closure — no captured upvalues).
#[derive(Debug, Clone)]
pub struct FunctionPrototype {
    pub name: String,
    pub arity: u8,
    /// If true, this function accepts `arity` required params plus a rest parameter.
    /// At call time, excess arguments are packed into a List and placed in
    /// local slot `arity` (the rest param slot).
    pub is_variadic: bool,
    pub upvalues: Vec<UpvalueDescriptor>,
    pub chunk: Chunk,
    /// The module this function was compiled in. Used for module-scoped global resolution.
    pub module_id: ModuleId,
}

/// Describes how an upvalue is captured: from an enclosing local or from an
/// enclosing upvalue.
#[derive(Debug, Clone)]
pub struct UpvalueDescriptor {
    pub is_local: bool,
    pub index: u16,
    /// If true, this upvalue captures a mutable cell (for letrec-style local defines).
    /// If false, it captures a plain value copy (the common, efficient case).
    pub is_mutable: bool,
}
