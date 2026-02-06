use std::sync::Arc;

use crate::bytecode::FunctionPrototype;
use crate::value::Value;

/// Heap-allocated VM objects, stored behind `Arc` in NaN-boxed Values.
#[derive(Debug, Clone)]
pub enum VMObject {
    String(Arc<str>),
    List(Vec<Value>),
    AdtValue {
        tag: String,
        fields: Vec<Value>,
    },
    AdtConstructor {
        name: String,
        arity: u8,
    },
    Closure(Box<Closure>),
    Continuation(Box<VMContinuation>),
    Builtin {
        name: String,
        func: BuiltinFn,
    },
    Pid(u64),
    Channel(u64),
    // Network types (Phase 4+)
    TcpSocket(u64),
    TcpListener(u64),
    HttpServer(u64),
    HttpRequest(u64),
}

impl VMObject {
    pub fn kind_name(&self) -> &'static str {
        match self {
            VMObject::String(_) => "string",
            VMObject::List(_) => "list",
            VMObject::AdtValue { .. } => "adt-value",
            VMObject::AdtConstructor { .. } => "adt-constructor",
            VMObject::Closure(_) => "closure",
            VMObject::Continuation(_) => "continuation",
            VMObject::Builtin { .. } => "builtin",
            VMObject::Pid(_) => "pid",
            VMObject::Channel(_) => "channel",
            VMObject::TcpSocket(_) => "tcp-socket",
            VMObject::TcpListener(_) => "tcp-listener",
            VMObject::HttpServer(_) => "http-server",
            VMObject::HttpRequest(_) => "http-request",
        }
    }
}

impl PartialEq for VMObject {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (VMObject::String(a), VMObject::String(b)) => a == b,
            (VMObject::List(a), VMObject::List(b)) => a == b,
            (VMObject::AdtValue { tag: t1, fields: f1 }, VMObject::AdtValue { tag: t2, fields: f2 }) => {
                t1 == t2 && f1 == f2
            }
            (VMObject::AdtConstructor { name: n1, arity: a1 }, VMObject::AdtConstructor { name: n2, arity: a2 }) => {
                n1 == n2 && a1 == a2
            }
            (VMObject::Pid(a), VMObject::Pid(b)) => a == b,
            (VMObject::Channel(a), VMObject::Channel(b)) => a == b,
            _ => false,
        }
    }
}

/// A function type for built-in operations.
///
/// Takes a mutable reference to the VM (for globals, stack access) and argument slice.
/// Returns `Result<Value, VMError>` to propagate runtime errors.
pub type BuiltinFn = fn(&mut crate::vm::VM, &[Value]) -> Result<Value, crate::errors::VMError>;

/// A closure: a function prototype plus captured upvalues.
#[derive(Debug, Clone)]
pub struct Closure {
    pub proto: FunctionPrototype,
    pub upvalues: Vec<Value>,
}

/// A call frame on the VM's call stack.
#[derive(Debug, Clone)]
pub struct CallFrame {
    pub closure: Closure,
    pub ip: usize,
    pub locals: Vec<Value>,
}

/// An entry in a handler frame mapping an effect operation to its handler closure.
#[derive(Debug, Clone)]
pub struct HandlerEntry {
    pub op_name: String,
    pub closure: Closure,
    pub arity: u8,
}

/// A handler frame installed on the handler stack by `handle`.
#[derive(Debug, Clone)]
pub struct VMHandlerFrame {
    pub handlers: Vec<HandlerEntry>,
    pub return_handler: Option<Closure>,
    // Unwind snapshots (immutable — set when handler is installed)
    pub unwind_frame_start: usize,
    pub unwind_stack_start: usize,
    // Capture snapshots (mutable — updated by RESUME)
    pub capture_frame_start: usize,
    pub capture_stack_start: usize,
    /// Unique ID for identity-based handler matching (replaces reference equality from TypeScript).
    pub id: u64,
}

/// A captured one-shot continuation for effect handling.
#[derive(Debug, Clone)]
pub struct VMContinuation {
    pub frames: Vec<CallFrame>,
    pub stack: Vec<Value>,
    pub handler_stack: Vec<VMHandlerFrame>,
    /// Unique ID used to enforce one-shot semantics via VM::resumed_continuations.
    pub id: u64,
    pub frame_base: usize,
    pub stack_base: usize,
}
