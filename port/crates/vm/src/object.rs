use crate::bytecode::FunctionPrototype;
use crate::value::Value;
use std::sync::{Arc, Mutex};

/// A mutable upvalue cell, shared between a closure and its enclosing scope.
/// Uses `Arc<Mutex<Value>>` for thread-safety (required by multi-threaded scheduler).
pub type UpvalueCell = Arc<Mutex<Value>>;

/// Build a Cons/Nil chain from a `Vec<Value>`.
///
/// Converts `[a, b, c]` into `Cons(a, Cons(b, Cons(c, Nil)))` using `AdtValue`.
/// This is the canonical way to construct list values at runtime.
pub fn vec_to_cons_list(items: Vec<Value>) -> Value {
    let mut result = Value::object(VMObject::AdtValue {
        tag: "Nil".to_string(),
        fields: vec![],
    });
    for item in items.into_iter().rev() {
        result = Value::object(VMObject::AdtValue {
            tag: "Cons".to_string(),
            fields: vec![item, result],
        });
    }
    result
}

/// A captured upvalue: either a plain value copy (cheap, common case) or a
/// shared mutable cell (for letrec-style recursive local defines).
#[derive(Debug, Clone)]
pub enum Upvalue {
    /// Captured by value — the common case. No heap allocation, no locking.
    Value(Value),
    /// Shared mutable cell — used when a local is captured while still being
    /// assigned to (e.g., recursive local define capturing itself).
    Cell(UpvalueCell),
}

impl Upvalue {
    /// Read the current value from this upvalue.
    #[inline]
    pub fn get(&self) -> Value {
        match self {
            Upvalue::Value(v) => v.clone(),
            Upvalue::Cell(cell) => cell.lock().unwrap().clone(),
        }
    }

    /// Write a value into this upvalue (only valid for Cell variant).
    #[inline]
    pub fn set(&self, val: Value) {
        match self {
            Upvalue::Value(_) => {
                // Value upvalues are immutable snapshots — this shouldn't be called.
                // In debug builds, panic; in release, silently ignore.
                debug_assert!(false, "cannot set a Value upvalue");
            }
            Upvalue::Cell(cell) => {
                *cell.lock().unwrap() = val;
            }
        }
    }
}

/// Heap-allocated VM objects, stored behind `Arc` in tagged `Value`s.
#[derive(Debug, Clone)]
pub enum VMObject {
    String(Arc<str>),
    Symbol(String),
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
    Port(u64),
    /// A bound FFI function: stores the library global name and function name.
    /// When called, looks up the library from globals and dispatches via ffi_call.
    #[cfg(feature = "ffi")]
    FfiBound {
        lib_global_name: Arc<str>,
        fn_name: Arc<str>,
    },
    // FFI types
    #[cfg(feature = "ffi")]
    FfiLibrary(std::sync::Arc<crate::ffi::FfiLibrary>),
    #[cfg(feature = "ffi")]
    FfiPointer(u64),
    #[cfg(feature = "ffi")]
    FfiStructDef(std::sync::Arc<crate::ffi::FfiStructDef>),
    #[cfg(feature = "ffi")]
    FfiStruct(Box<crate::ffi::FfiStruct>),
}

impl VMObject {
    pub fn kind_name(&self) -> &'static str {
        match self {
            VMObject::String(_) => "string",
            VMObject::Symbol(_) => "symbol",
            VMObject::AdtValue { .. } => "adt-value",
            VMObject::AdtConstructor { .. } => "adt-constructor",
            VMObject::Closure(_) => "closure",
            VMObject::Continuation(_) => "continuation",
            VMObject::Builtin { .. } => "builtin",
            VMObject::Pid(_) => "pid",
            VMObject::Port(_) => "port",
            #[cfg(feature = "ffi")]
            VMObject::FfiBound { .. } => "ffi-bound",
            #[cfg(feature = "ffi")]
            VMObject::FfiLibrary(_) => "ffi-library",
            #[cfg(feature = "ffi")]
            VMObject::FfiPointer(_) => "ffi-pointer",
            #[cfg(feature = "ffi")]
            VMObject::FfiStructDef(_) => "ffi-struct-def",
            #[cfg(feature = "ffi")]
            VMObject::FfiStruct(_) => "ffi-struct",
        }
    }
}

impl PartialEq for VMObject {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (VMObject::String(a), VMObject::String(b)) => a == b,
            (VMObject::Symbol(a), VMObject::Symbol(b)) => a == b,
            (
                VMObject::AdtValue {
                    tag: t1,
                    fields: f1,
                },
                VMObject::AdtValue {
                    tag: t2,
                    fields: f2,
                },
            ) => t1 == t2 && f1 == f2,
            (
                VMObject::AdtConstructor {
                    name: n1,
                    arity: a1,
                },
                VMObject::AdtConstructor {
                    name: n2,
                    arity: a2,
                },
            ) => n1 == n2 && a1 == a2,
            (VMObject::Pid(a), VMObject::Pid(b)) => a == b,
            (VMObject::Port(a), VMObject::Port(b)) => a == b,
            #[cfg(feature = "ffi")]
            (VMObject::FfiPointer(a), VMObject::FfiPointer(b)) => a == b,
            #[cfg(feature = "ffi")]
            (
                VMObject::FfiBound {
                    lib_global_name: l1,
                    fn_name: f1,
                },
                VMObject::FfiBound {
                    lib_global_name: l2,
                    fn_name: f2,
                },
            ) => l1 == l2 && f1 == f2,
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
/// Uses Arc to share the prototype, avoiding expensive clones of the chunk.
/// Most upvalues are cheap value copies; only letrec-style recursive local
/// defines use shared mutable cells (`Arc<Mutex<Value>>`).
#[derive(Debug, Clone)]
pub struct Closure {
    pub proto: Arc<FunctionPrototype>,
    pub upvalues: Vec<Upvalue>,
    /// The module this closure belongs to. Copied from `proto.module_id` at creation,
    /// stored here for fast access during PushGlobal resolution without indirecting
    /// through the Arc<FunctionPrototype>.
    pub module_id: crate::bytecode::ModuleId,
}

/// A call frame on the VM's call stack.
#[derive(Debug, Clone)]
pub struct CallFrame {
    pub closure: Closure,
    pub ip: usize,
    pub locals: Vec<Value>,
    /// Open upvalue cells for locals that have been captured by inner closures.
    /// Maps local slot index → shared upvalue cell. When a local is captured,
    /// its value is moved into an `UpvalueCell` stored here; subsequent reads/writes
    /// to that local go through the cell (shared with the capturing closure).
    pub open_upvalues: Vec<Option<UpvalueCell>>,
}

/// An entry in a handler frame mapping an effect operation to its handler closure.
#[derive(Debug, Clone)]
pub struct HandlerEntry {
    pub effect_name: String,
    pub op_name: String,
    pub closure: Closure,
    pub arity: u8,
    /// For tail-resumptive handlers: a closure that computes the resume value
    /// without needing the continuation. Takes the effect args (but not k) and
    /// returns the value to resume with. When set, `Perform` can skip
    /// continuation capture entirely.
    pub tail_resume_closure: Option<Closure>,
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
    /// True for scheduler-installed dummy handlers. These use the EffectSignal path.
    /// False for user `handle` blocks, which are dispatched inline via CPS.
    pub is_scheduler: bool,
    /// True when ALL handler clauses are tail-resumptive (body is `(k expr)` where
    /// expr is a simple expression that doesn't reference k). When true, `Perform`
    /// can skip continuation capture and call the tail-resume closure directly.
    pub is_tail_resumptive: bool,
}

/// A handler dispatch marker — tracks that we're inside a handler clause invocation.
/// When Op::Return drops frame_count to `dispatch_frame`, we know the handler clause
/// has returned and we should clean up the handler + apply the return handler.
#[derive(Debug, Clone)]
pub struct HandlerDispatch {
    /// Frame count threshold — when Op::Return drops frame_count to this, we're done
    pub dispatch_frame: usize,
    /// Handler entries for deep re-installation on resume
    pub handler_entries: Vec<HandlerEntry>,
    /// Return handler closure
    pub return_handler: Option<Closure>,
    /// Handler ID for the re-installed handler frame
    pub handler_id: u64,
}

/// A captured one-shot continuation for effect handling.
#[derive(Debug, Clone)]
pub struct VMContinuation {
    pub frames: Vec<CallFrame>,
    pub stack: Vec<Value>,
    pub handler_stack: Vec<VMHandlerFrame>,
    pub handler_dispatch_stack: Vec<HandlerDispatch>,
    /// Unique ID used to enforce one-shot semantics via VM::resumed_continuations.
    pub id: u64,
    pub frame_base: usize,
    pub stack_base: usize,
}
