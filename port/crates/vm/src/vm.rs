use crate::bytecode::{Chunk, ConstantValue, FunctionPrototype, ModuleId, Op, ROOT_MODULE};
use crate::errors::{StackTraceEntry, VMError};
use crate::object::{
    CallFrame, Closure, HandlerDispatch, HandlerEntry, VMContinuation, VMHandlerFrame, VMObject,
};
use crate::value::{Value, format_value};
use rustc_hash::FxHashMap;
use std::collections::HashSet;
use std::sync::Arc;

// --- Parse state ---

// --- VMSignal: unified error/effect propagation ---

pub(crate) enum VMSignal {
    Error(VMError),
    Effect(EffectSignal),
}

impl From<VMError> for VMSignal {
    fn from(e: VMError) -> Self {
        VMSignal::Error(e)
    }
}

pub(crate) struct EffectSignal {
    pub op_name: String,
    pub args: Vec<Value>,
    pub continuation: Value, // wraps VMContinuation
    #[allow(dead_code)] // Useful metadata, not read by scheduler but may be useful for debugging
    pub target_handler_id: u64,
}

/// A fully captured effect that must be CPS-dispatched by an outer `run()` loop.
///
/// When a `Perform` inside a nested `run()` call (e.g. from `interpret_closure`)
/// finds a handler whose unwind target is below that `run()`'s base frame, the
/// effect cannot be dispatched CPS-inline in the inner `run()` — doing so would
/// unwind `frame_count` below `base`, causing the inner `run()` to exit with
/// `Ok(handler_result)` while leaving the VM's frame state corrupted for the
/// outer interpreter.
///
/// Instead, the inner `run()` captures the continuation and unwinds VM state,
/// then stores this struct in `vm.pending_effect` and returns via `VMSignal::Effect`.
/// The signal propagates up through `interpret_closure` → `call_closure_from_jit`
/// → `marlang_jit_call` → JIT bail → interpreter, where the outer `run()` loop
/// picks it up and performs the CPS dispatch in the correct scope.
#[cfg(feature = "jit")]
pub(crate) struct PendingEffect {
    pub args: Vec<Value>,
    pub continuation: Value,
    pub performed_effect: String,
    pub performed_op: String,
}

// --- VM ---

static HANDLER_ID_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
static CONTINUATION_ID_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub(crate) fn next_handler_id() -> u64 {
    HANDLER_ID_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

fn next_continuation_id() -> u64 {
    CONTINUATION_ID_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

pub struct VM {
    pub stack: Vec<Value>,
    pub stack_top: usize,
    pub frames: Vec<CallFrame>,
    pub frame_count: usize,
    /// Per-module globals tables. Index 0 is the root module (builtins + prelude + main).
    /// Each module gets its own table. PushGlobal resolves against the closure's module
    /// first, then falls back to the root module.
    pub module_globals: Vec<FxHashMap<String, Value>>,
    /// The currently active module (for SetGlobal writes).
    pub current_module: ModuleId,
    pub handler_stack: Vec<VMHandlerFrame>,
    /// Handler dispatch markers — tracks active handler clause invocations for CPS dispatch.
    pub(crate) handler_dispatch_stack: Vec<HandlerDispatch>,
    /// Tracks which continuation IDs have been resumed (one-shot enforcement).
    pub(crate) resumed_continuations: HashSet<u64>,
    /// Captured output for testing (when Some, print/println write here instead of stdout)
    pub captured_output: Option<String>,
    /// When Some, records the names of globals written via SetGlobal (for module export tracking).
    pub dirty_globals: Option<HashSet<String>>,
    /// Current fiber PID when running under the scheduler (for fast-path fiber-self).
    pub(crate) current_pid: Option<u64>,

    /// Cache for parsed handler descriptors, keyed by constant pool string pointer.
    /// Avoids re-parsing the JSON descriptor string on every PushHandler.
    pub(crate) handler_descriptor_cache: FxHashMap<usize, HandlerDescriptor>,

    // --- JIT ---
    /// JIT compilation engine (lazily initialized on first hot function).
    #[cfg(feature = "jit")]
    pub(crate) jit_engine: Option<crate::jit::JitEngine>,
    /// Map from FunctionPrototype Arc pointer address to compiled native function.
    #[cfg(feature = "jit")]
    pub(crate) jit_code: FxHashMap<usize, crate::jit::JitFn>,
    /// Call counters for JIT profiling, keyed by prototype pointer address.
    #[cfg(feature = "jit")]
    pub(crate) jit_call_counts: FxHashMap<usize, u32>,
    /// Pending effect that crossed a `run()` boundary and needs CPS dispatch
    /// in the outer interpreter loop. See `PendingEffect` for details.
    #[cfg(feature = "jit")]
    pub(crate) pending_effect: Option<PendingEffect>,
}

impl VM {
    pub fn new() -> Self {
        VM {
            stack: Vec::with_capacity(256),
            stack_top: 0,
            frames: Vec::with_capacity(64),
            frame_count: 0,
            module_globals: vec![FxHashMap::default()], // index 0 = root module
            current_module: ROOT_MODULE,
            handler_stack: Vec::new(),
            handler_dispatch_stack: Vec::new(),
            resumed_continuations: HashSet::new(),
            captured_output: None,
            dirty_globals: None,
            current_pid: None,
            handler_descriptor_cache: FxHashMap::default(),
            #[cfg(feature = "jit")]
            jit_engine: None,
            #[cfg(feature = "jit")]
            jit_code: FxHashMap::default(),
            #[cfg(feature = "jit")]
            jit_call_counts: FxHashMap::default(),
            #[cfg(feature = "jit")]
            pending_effect: None,
        }
    }

    /// Convenience accessor for the root module globals table.
    /// Most code that used `vm.globals` should use this instead.
    #[inline]
    pub fn globals(&self) -> &FxHashMap<String, Value> {
        &self.module_globals[ROOT_MODULE as usize]
    }

    /// Mutable accessor for the root module globals table.
    #[inline]
    pub fn globals_mut(&mut self) -> &mut FxHashMap<String, Value> {
        &mut self.module_globals[ROOT_MODULE as usize]
    }

    /// Get the globals table for a given module.
    #[inline]
    pub fn module_globals_for(&self, module_id: ModuleId) -> &FxHashMap<String, Value> {
        &self.module_globals[module_id as usize]
    }

    /// Get a mutable reference to the globals table for a given module.
    #[inline]
    pub fn module_globals_for_mut(&mut self, module_id: ModuleId) -> &mut FxHashMap<String, Value> {
        &mut self.module_globals[module_id as usize]
    }

    /// Allocate a new module and return its ID.
    pub fn allocate_module(&mut self) -> ModuleId {
        let id = self.module_globals.len() as ModuleId;
        self.module_globals.push(FxHashMap::default());
        id
    }

    /// Look up a global by name, checking the given module first, then falling back to root.
    #[inline]
    pub fn resolve_global(&self, name: &str, module_id: ModuleId) -> Option<&Value> {
        if module_id != ROOT_MODULE
            && let Some(v) = self.module_globals[module_id as usize].get(name)
        {
            return Some(v);
        }
        self.module_globals[ROOT_MODULE as usize].get(name)
    }

    #[inline]
    pub fn push(&mut self, value: Value) {
        if self.stack_top >= self.stack.len() {
            self.stack.push(value);
        } else {
            self.stack[self.stack_top] = value;
        }
        self.stack_top += 1;
    }

    #[inline]
    pub fn pop(&mut self) -> Value {
        debug_assert!(self.stack_top > 0, "stack underflow");
        self.stack_top -= 1;
        if self.stack_top < self.stack.len() {
            std::mem::replace(&mut self.stack[self.stack_top], Value::nil())
        } else {
            Value::nil()
        }
    }

    /// Clear stack slots in range [new_top..old_top) to release Arc references.
    pub(crate) fn clear_stack_slots(&mut self, new_top: usize) {
        let end = self.stack_top.min(self.stack.len());
        for i in new_top..end {
            self.stack[i] = Value::nil();
        }
    }

    #[inline]
    pub fn peek(&self, distance: usize) -> &Value {
        &self.stack[self.stack_top - 1 - distance]
    }

    pub(crate) fn push_frame(&mut self, frame: CallFrame) {
        if self.frame_count >= self.frames.len() {
            self.frames.push(frame);
        } else {
            self.frames[self.frame_count] = frame;
        }
        self.frame_count += 1;
    }

    /// Take a reusable locals Vec from the frame slot that will be overwritten next.
    /// This avoids heap allocation when frames are reused (which is the common case
    /// after the call stack has grown to its peak depth once).
    #[inline]
    fn take_locals(&mut self, count: usize) -> Vec<Value> {
        if self.frame_count < self.frames.len() {
            let mut locals = std::mem::take(&mut self.frames[self.frame_count].locals);
            locals.clear();
            locals.resize(count, Value::nil());
            locals
        } else {
            vec![Value::nil(); count]
        }
    }

    pub fn capture_stack_trace(&self, current_frame: &CallFrame) -> Vec<StackTraceEntry> {
        let mut trace = Vec::new();
        for i in (0..self.frame_count).rev() {
            let f = if i == self.frame_count - 1 {
                current_frame
            } else {
                &self.frames[i]
            };
            let ip = if f.ip > 0 { f.ip - 1 } else { 0 };
            let chunk = &f.closure.proto.chunk;
            let line = chunk.code.get(ip).map(|inst| inst.line).unwrap_or(0);
            trace.push(StackTraceEntry {
                function_name: f.closure.proto.name.clone(),
                line,
            });
        }
        trace
    }

    fn vm_error(&self, msg: impl Into<String>, frame: &CallFrame) -> VMError {
        let trace = self.capture_stack_trace(frame);
        let line = trace.first().map(|e| e.line).unwrap_or(0);
        VMError::new(msg, line, trace)
    }
}

impl VM {
    /// Create a new VM pre-populated with module globals cloned from a source VM.
    /// Used by the multi-threaded scheduler to give each thread its own VM.
    pub fn new_with_module_globals(module_globals: Vec<FxHashMap<String, Value>>) -> Self {
        VM {
            module_globals,
            ..VM::new()
        }
    }
}

impl Default for VM {
    fn default() -> Self {
        Self::new()
    }
}

// --- Closure helpers ---

fn create_closure_from_proto(frame: &mut CallFrame, proto: &Arc<FunctionPrototype>) -> Closure {
    use crate::object::{Upvalue, UpvalueCell};
    let mut upvalues: Vec<Upvalue> = Vec::with_capacity(proto.upvalues.len());
    for desc in &proto.upvalues {
        if desc.is_local {
            let idx = desc.index as usize;
            if desc.is_mutable {
                // Mutable capture: create or reuse a shared cell.
                // This is the letrec path — the local's value may change after capture.
                let cell = if idx < frame.open_upvalues.len() {
                    if let Some(ref cell) = frame.open_upvalues[idx] {
                        cell.clone()
                    } else {
                        let cell: UpvalueCell =
                            Arc::new(std::sync::Mutex::new(frame.locals[idx].clone()));
                        frame.open_upvalues[idx] = Some(cell.clone());
                        cell
                    }
                } else {
                    frame.open_upvalues.resize(idx + 1, None);
                    let cell: UpvalueCell =
                        Arc::new(std::sync::Mutex::new(frame.locals[idx].clone()));
                    frame.open_upvalues[idx] = Some(cell.clone());
                    cell
                };
                upvalues.push(Upvalue::Cell(cell));
            } else {
                // Immutable capture: cheap value copy (the common case).
                upvalues.push(Upvalue::Value(frame.locals[idx].clone()));
            }
        } else {
            // Capture from enclosing closure's upvalue — clone the Upvalue variant
            upvalues.push(frame.closure.upvalues[desc.index as usize].clone());
        }
    }
    let module_id = proto.module_id;
    Closure {
        proto: proto.clone(),
        upvalues,
        module_id,
    }
}

/// Call a closure, pushing a frame and running until it returns.
pub(crate) fn call_closure(
    vm: &mut VM,
    closure: &Closure,
    args: &[Value],
) -> Result<Value, VMSignal> {
    let mut locals = vec![Value::nil(); closure.proto.chunk.local_count as usize];
    for (i, arg) in args.iter().enumerate() {
        locals[i] = arg.clone();
    }
    let new_frame = CallFrame {
        closure: closure.clone(),
        ip: 0,
        locals,
        open_upvalues: Vec::new(),
    };
    let base_frame_count = vm.frame_count;
    vm.push_frame(new_frame);
    run(vm, base_frame_count)
}

/// Call any callable Value (closure, builtin, etc) from Rust code.
/// This is the public interface for calling Marlang functions from builtins.
pub fn call_value(vm: &mut VM, callee: &Value, args: &[Value]) -> Result<Value, VMError> {
    match callee.as_object() {
        Some(VMObject::Closure(closure)) => match call_closure(vm, closure, args) {
            Ok(v) => Ok(v),
            Err(VMSignal::Error(e)) => Err(e),
            Err(VMSignal::Effect(_)) => Err(VMError::runtime("Effect escaped from call_value")),
        },
        Some(VMObject::Builtin { func, .. }) => func(vm, args),
        Some(VMObject::AdtConstructor { name, arity }) => {
            if args.len() != *arity as usize {
                return Err(VMError::runtime(format!(
                    "Constructor {} expects {} arguments, got {}",
                    name,
                    arity,
                    args.len()
                )));
            }
            Ok(Value::object(VMObject::AdtValue {
                tag: name.clone(),
                fields: args.to_vec(),
            }))
        }
        _ => Err(VMError::runtime(format!(
            "Not callable: {}",
            callee.kind_name()
        ))),
    }
}

/// Resume a continuation from Rust (used by scheduler).
/// Takes ownership of the continuation to avoid deep-cloning.
/// With CPS dispatch, user-handler effects are dispatched inline by PERFORM,
/// so only scheduler effects escape as VMSignal::Effect.
pub(crate) fn resume_continuation_owned(
    vm: &mut VM,
    cont: VMContinuation,
    value: Value,
) -> Result<Value, VMSignal> {
    // Enforce one-shot semantics via VM-level tracking (works across Arc aliases)
    if !vm.resumed_continuations.insert(cont.id) {
        return Err(VMSignal::Error(VMError::runtime(
            "Continuation already resumed (one-shot)",
        )));
    }

    let resume_base = vm.frame_count;
    let stack_len = cont.stack.len();
    let frame_base = cont.frame_base;
    let stack_base = cont.stack_base;

    // Restore frames by consuming — no clone
    for f in cont.frames {
        vm.push_frame(f);
    }

    // Restore stack by consuming — no clone
    for v in cont.stack {
        vm.push(v);
    }
    vm.push(value);

    let saved_handler_len = vm.handler_stack.len();
    let saved_dispatch_len = vm.handler_dispatch_stack.len();
    let frame_offset = resume_base as i64 - frame_base as i64;
    let stack_offset = (vm.stack_top as i64 - stack_len as i64 - 1) - stack_base as i64;

    // Restore handlers by consuming — mutate in place, no clone
    for mut h in cont.handler_stack {
        h.unwind_frame_start = (h.unwind_frame_start as i64 + frame_offset) as usize;
        h.unwind_stack_start = (h.unwind_stack_start as i64 + stack_offset) as usize;
        h.capture_frame_start = (h.capture_frame_start as i64 + frame_offset) as usize;
        h.capture_stack_start = (h.capture_stack_start as i64 + stack_offset) as usize;
        vm.handler_stack.push(h);
    }
    for mut d in cont.handler_dispatch_stack {
        d.dispatch_frame = (d.dispatch_frame as i64 + frame_offset) as usize;
        vm.handler_dispatch_stack.push(d);
    }

    // Run — user-handler effects are dispatched inline by PERFORM.
    // Only scheduler effects escape as VMSignal::Effect.
    let result = run(vm, resume_base);

    vm.frame_count = resume_base;
    vm.handler_stack.truncate(saved_handler_len);
    vm.handler_dispatch_stack.truncate(saved_dispatch_len);

    result
}

/// Resume a continuation from Rust (fallback: borrows continuation, must clone).
#[allow(dead_code)]
pub(crate) fn resume_continuation(
    vm: &mut VM,
    cont: &VMContinuation,
    value: Value,
) -> Result<Value, VMSignal> {
    // Clone and delegate to the owned version
    resume_continuation_owned(vm, cont.clone(), value)
}

#[allow(dead_code)]
fn rebase_handler(h: &VMHandlerFrame, frame_offset: i64, stack_offset: i64) -> VMHandlerFrame {
    VMHandlerFrame {
        handlers: h.handlers.clone(),
        return_handler: h.return_handler.clone(),
        unwind_frame_start: (h.unwind_frame_start as i64 + frame_offset) as usize,
        unwind_stack_start: (h.unwind_stack_start as i64 + stack_offset) as usize,
        capture_frame_start: (h.capture_frame_start as i64 + frame_offset) as usize,
        capture_stack_start: (h.capture_stack_start as i64 + stack_offset) as usize,
        id: h.id,
        is_scheduler: h.is_scheduler,
        is_tail_resumptive: h.is_tail_resumptive,
    }
}

// --- Effect handler helpers ---

#[allow(dead_code)]
fn rebase_handler_dispatch(d: &HandlerDispatch, frame_offset: i64) -> HandlerDispatch {
    HandlerDispatch {
        dispatch_frame: (d.dispatch_frame as i64 + frame_offset) as usize,
        handler_entries: d.handler_entries.clone(),
        return_handler: d.return_handler.clone(),
        handler_id: d.handler_id,
    }
}

// --- Script Closure ---

/// Create a closure from a compiled chunk, suitable for running as a top-level
/// expression via `call_closure`. Unlike `interpret`, this does NOT reset the VM's
/// frame_count and stack_top — it's designed for use with the scheduler, where
/// the scheduler handler frame must survive below the script's frames.
pub fn make_script_closure(chunk: &Chunk, module_id: ModuleId) -> Closure {
    let script_proto = FunctionPrototype {
        name: "<script>".to_string(),
        arity: 0,
        is_variadic: false,
        upvalues: Vec::new(),
        chunk: chunk.clone(),
        module_id,
    };
    Closure {
        proto: Arc::new(script_proto),
        upvalues: Vec::new(),
        module_id,
    }
}

// --- Interpret ---

pub fn interpret(vm: &mut VM, chunk: &Chunk) -> Result<Value, VMError> {
    let current_module = vm.current_module;
    let script_proto = FunctionPrototype {
        name: "<script>".to_string(),
        arity: 0,
        is_variadic: false,
        upvalues: Vec::new(),
        chunk: chunk.clone(),
        module_id: current_module,
    };

    let script_closure = Closure {
        proto: Arc::new(script_proto),
        upvalues: Vec::new(),
        module_id: current_module,
    };

    let local_count = chunk.local_count as usize;
    let locals = vec![Value::nil(); local_count];
    let frame = CallFrame {
        closure: script_closure,
        ip: 0,
        locals,
        open_upvalues: Vec::new(),
    };

    vm.frame_count = 0;
    vm.stack_top = 0;
    vm.push_frame(frame);

    match run(vm, 0) {
        Ok(value) => Ok(value),
        Err(VMSignal::Error(e)) => Err(e),
        Err(VMSignal::Effect(signal)) => Err(VMError::runtime(format!(
            "Unhandled effect: {}",
            signal.op_name
        ))),
    }
}

// --- JSON parsing for handler descriptors ---

#[derive(Clone)]
struct HandlerDescriptor {
    body_proto_idx: u32,
    handlers: Vec<HandlerClauseDescriptor>,
    return_proto_idx: i32,
    /// True when all handler clauses are tail-resumptive.
    tail_resumptive: bool,
}

#[derive(Clone)]
struct HandlerClauseDescriptor {
    effect_name_idx: u32,
    op_name_idx: u32,
    proto_idx: u32,
    arity: u8,
    /// Prototype index for the tail-resume-value closure, or -1 if not tail-resumptive.
    tail_resume_proto_idx: i32,
    /// True when the handler body doesn't reference the continuation.
    /// Aborting handlers can skip continuation capture entirely.
    is_aborting: bool,
}

fn parse_handler_descriptor(json: &str) -> Result<HandlerDescriptor, String> {
    // Simple hand-rolled JSON parser for the fixed format:
    // {"bodyProtoIdx":N,"handlers":[{"opNameIdx":N,"protoIdx":N,"arity":N}...],"returnProtoIdx":N}
    let json = json.trim();
    if !json.starts_with('{') || !json.ends_with('}') {
        return Err("expected object".to_string());
    }
    let inner = &json[1..json.len() - 1];

    let body_proto_idx = extract_number(inner, "bodyProtoIdx")? as u32;
    let return_proto_idx = extract_number(inner, "returnProtoIdx")? as i32;

    // Extract handlers array
    let handlers_start = inner.find("\"handlers\":[").ok_or("missing handlers")?;
    let arr_start = inner[handlers_start..].find('[').unwrap() + handlers_start;
    let arr_end = find_matching_bracket(inner, arr_start)?;
    let arr_content = &inner[arr_start + 1..arr_end];

    let mut handlers = Vec::new();
    if !arr_content.trim().is_empty() {
        // Split on },{ boundaries
        let mut depth = 0;
        let mut start = 0;
        let bytes = arr_content.as_bytes();
        for i in 0..bytes.len() {
            match bytes[i] {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        let obj_str = &arr_content[start..=i].trim();
                        let h = parse_handler_clause(obj_str)?;
                        handlers.push(h);
                        start = i + 1;
                        // skip comma
                        while start < bytes.len() && (bytes[start] == b',' || bytes[start] == b' ')
                        {
                            start += 1;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // Parse optional tail-resumptive flag (defaults to false)
    let tail_resumptive = extract_number(inner, "tailResumptive")
        .map(|n| n as i32 != 0)
        .unwrap_or(false);

    Ok(HandlerDescriptor {
        body_proto_idx,
        handlers,
        return_proto_idx,
        tail_resumptive,
    })
}

fn parse_handler_clause(json: &str) -> Result<HandlerClauseDescriptor, String> {
    let json = json.trim();
    if !json.starts_with('{') || !json.ends_with('}') {
        return Err("expected handler clause object".to_string());
    }
    let inner = &json[1..json.len() - 1];
    let effect_name_idx = extract_number(inner, "effectNameIdx")? as u32;
    let op_name_idx = extract_number(inner, "opNameIdx")? as u32;
    let proto_idx = extract_number(inner, "protoIdx")? as u32;
    let arity = extract_number(inner, "arity")? as u8;
    // Parse optional tail-resume prototype index (defaults to -1)
    let tail_resume_proto_idx = extract_number(inner, "tailResumeProtoIdx")
        .map(|n| n as i32)
        .unwrap_or(-1);
    // Parse optional aborting flag (defaults to false)
    let is_aborting = extract_number(inner, "aborting")
        .map(|n| n as i32 != 0)
        .unwrap_or(false);
    Ok(HandlerClauseDescriptor {
        effect_name_idx,
        op_name_idx,
        proto_idx,
        arity,
        tail_resume_proto_idx,
        is_aborting,
    })
}

fn extract_number(s: &str, key: &str) -> Result<f64, String> {
    let pattern = format!("\"{}\":", key);
    let pos = s
        .find(&pattern)
        .ok_or_else(|| format!("missing key: {}", key))?;
    let after = &s[pos + pattern.len()..];
    let end = after.find([',', '}', ']']).unwrap_or(after.len());
    let num_str = after[..end].trim();
    num_str
        .parse::<f64>()
        .map_err(|e| format!("bad number for {}: {}", key, e))
}

fn find_matching_bracket(s: &str, open: usize) -> Result<usize, String> {
    let bytes = s.as_bytes();
    let open_char = bytes[open];
    let close_char = match open_char {
        b'[' => b']',
        b'{' => b'}',
        _ => return Err("not a bracket".to_string()),
    };
    let mut depth = 1;
    for (i, &b) in bytes.iter().enumerate().skip(open + 1) {
        if b == open_char {
            depth += 1;
        }
        if b == close_char {
            depth -= 1;
            if depth == 0 {
                return Ok(i);
            }
        }
    }
    Err("unmatched bracket".to_string())
}

/// Convert a `&[Value]` to a Marlang List (Cons/Nil chain).
/// Fast-path handler for effects that can be resolved inline without
/// continuation capture. Only used for effects that don't depend on
/// scheduler-level shared state (e.g. shared captured_output in the
/// parallel scheduler). IO effects (print, println, read-file, etc.)
/// are intentionally excluded here so they propagate to the scheduler,
/// which correctly handles both single-threaded and multi-threaded contexts.
pub(crate) fn try_fast_path_handler(vm: &mut VM, op_name: &str, _args: &[Value]) -> Option<Value> {
    match op_name {
        "fiber-self" => {
            let pid = vm.current_pid?;
            Some(Value::object(VMObject::Pid(pid)))
        }
        _ => None,
    }
}

// try_default_handler removed — all effects flow through the always-on scheduler.

// --- Main Execution Loop ---

pub(crate) fn run(vm: &mut VM, base_frame_count: usize) -> Result<Value, VMSignal> {
    let mut frame_idx = vm.frame_count - 1;
    let mut ip = vm.frames[frame_idx].ip;

    // Cached pointers for hot-path frame access, avoiding repeated
    // vm.frames[frame_idx].closure.proto.chunk dereference chains.
    //
    // SAFETY: These pointers are derived from vm.frames[frame_idx] and its
    // Arc<FunctionPrototype>. They remain valid as long as:
    //   - code/constants: the frame's closure holds the Arc alive
    //   - locals: the frame's locals Vec is not reallocated (it's fixed-size after creation)
    // We re-derive ALL cached pointers after every frame-changing operation
    // (Call, Return, TailCall, PushHandler, Resume, Perform).
    let mut code: *const crate::bytecode::Instruction;
    let mut constants: *const ConstantValue;
    let mut locals: *mut Value;

    macro_rules! cache_frame {
        () => {
            let f = &mut vm.frames[frame_idx];
            code = f.closure.proto.chunk.code.as_ptr();
            constants = f.closure.proto.chunk.constants.as_ptr();
            locals = f.locals.as_mut_ptr();
        };
    }
    cache_frame!();

    loop {
        // SAFETY: ip is always in bounds (enforced by bytecode compiler)
        let inst = unsafe { *code.add(ip) };
        ip += 1;

        match inst.op {
            // --- Stack operations ---
            Op::PushConst => {
                // SAFETY: operand is a valid constant index (from compiler)
                let val = unsafe { &*constants.add(inst.operand as usize) }.as_value();
                vm.push(val);
            }

            Op::PushLocal => {
                let slot = inst.operand as usize;
                // Fast path: most frames have no open upvalues (empty vec).
                // Only check the cell if open_upvalues is non-empty and this slot has one.
                let open_upvalues = &vm.frames[frame_idx].open_upvalues;
                if !open_upvalues.is_empty()
                    && let Some(Some(cell)) = open_upvalues.get(slot)
                {
                    let val = cell.lock().unwrap().clone();
                    vm.push(val);
                    continue;
                }
                // SAFETY: operand is a valid local index (from compiler)
                let val = unsafe { (*locals.add(slot)).clone() };
                vm.push(val);
            }

            Op::SetLocal => {
                let val = vm.pop();
                let slot = inst.operand as usize;
                // Sync to upvalue cell if this local has been captured mutably.
                let open_upvalues = &vm.frames[frame_idx].open_upvalues;
                if !open_upvalues.is_empty()
                    && let Some(Some(cell)) = open_upvalues.get(slot)
                {
                    *cell.lock().unwrap() = val.clone();
                }
                // SAFETY: operand is a valid local index, pointer is valid
                unsafe { *locals.add(slot) = val };
            }

            Op::PushUpvalue => {
                let upvalue = &vm.frames[frame_idx].closure.upvalues[inst.operand as usize];
                let val = upvalue.get();
                vm.push(val);
            }

            Op::SetUpvalue => {
                let val = vm.pop();
                let upvalue = &vm.frames[frame_idx].closure.upvalues[inst.operand as usize];
                upvalue.set(val);
            }

            Op::PushGlobal => {
                // SAFETY: operand is a valid constant index
                let cval = unsafe { &*constants.add(inst.operand as usize) };
                match cval {
                    ConstantValue::String(s) => {
                        // Resolve against closure's home module first, then root
                        let closure_module = vm.frames[frame_idx].closure.module_id;
                        match vm.resolve_global(s.as_str(), closure_module) {
                            Some(v) => vm.push(v.clone()),
                            None => {
                                // Fallback: strip hygiene marks and retry
                                let stripped = crate::expand::strip_all_marks(s);
                                if stripped != s.as_str() {
                                    if let Some(v) = vm.resolve_global(stripped, closure_module) {
                                        vm.push(v.clone());
                                    } else {
                                        vm.frames[frame_idx].ip = ip;
                                        return Err(VMSignal::Error(vm.vm_error(
                                            format!("Undefined variable: {}", stripped),
                                            &vm.frames[frame_idx],
                                        )));
                                    }
                                } else {
                                    vm.frames[frame_idx].ip = ip;
                                    return Err(VMSignal::Error(vm.vm_error(
                                        format!("Undefined variable: {}", s),
                                        &vm.frames[frame_idx],
                                    )));
                                }
                            }
                        }
                    }
                    _ => {
                        vm.frames[frame_idx].ip = ip;
                        return Err(VMSignal::Error(
                            vm.vm_error("Global name must be string", &vm.frames[frame_idx]),
                        ));
                    }
                }
            }

            Op::SetGlobal => {
                // SAFETY: operand is a valid constant index
                let name = match unsafe { &*constants.add(inst.operand as usize) } {
                    ConstantValue::String(s) => s.clone(),
                    _ => {
                        vm.frames[frame_idx].ip = ip;
                        return Err(VMSignal::Error(
                            vm.vm_error("Global name must be string", &vm.frames[frame_idx]),
                        ));
                    }
                };
                let value = vm.pop();

                // Track dirty globals for module export detection
                if let Some(ref mut dirty) = vm.dirty_globals {
                    dirty.insert(name.clone());
                }

                let target_module = vm.current_module;

                // Check for N-ary constructor encoding: "\0ctor:Name:arity"
                if let Some(s) = value.as_string()
                    && let Some(rest) = s.strip_prefix("\0ctor:")
                    && let Some(colon_pos) = rest.rfind(':')
                {
                    let ctor_name = &rest[..colon_pos];
                    if let Ok(arity) = rest[colon_pos + 1..].parse::<u8>() {
                        vm.module_globals[target_module as usize].insert(
                            name,
                            Value::object(VMObject::AdtConstructor {
                                name: ctor_name.to_string(),
                                arity,
                            }),
                        );
                        continue;
                    }
                }
                vm.module_globals[target_module as usize].insert(name, value);
            }

            Op::Pop => {
                vm.pop();
            }

            Op::Dup => {
                let val = vm.peek(0).clone();
                vm.push(val);
            }

            // --- Integer Arithmetic (type checker guarantees operand types) ---
            Op::AddInt => {
                let b = vm.pop();
                let a = vm.pop();
                vm.push(Value::int(a.get_int().wrapping_add(b.get_int())));
            }

            Op::SubInt => {
                let b = vm.pop();
                let a = vm.pop();
                vm.push(Value::int(a.get_int().wrapping_sub(b.get_int())));
            }

            Op::MulInt => {
                let b = vm.pop();
                let a = vm.pop();
                vm.push(Value::int(a.get_int().wrapping_mul(b.get_int())));
            }

            Op::DivInt => {
                let b = vm.pop();
                let a = vm.pop();
                let bv = b.get_int();
                if bv == 0 {
                    vm.frames[frame_idx].ip = ip;
                    return Err(VMSignal::Error(
                        vm.vm_error("Division by zero", &vm.frames[frame_idx]),
                    ));
                }
                vm.push(Value::int(a.get_int().wrapping_div(bv)));
            }

            Op::NegInt => {
                let a = vm.pop();
                vm.push(Value::int(a.get_int().wrapping_neg()));
            }

            // --- Float Arithmetic (type checker guarantees operand types) ---
            Op::AddFloat => {
                let b = vm.pop();
                let a = vm.pop();
                vm.push(Value::float(a.get_float() + b.get_float()));
            }

            Op::SubFloat => {
                let b = vm.pop();
                let a = vm.pop();
                vm.push(Value::float(a.get_float() - b.get_float()));
            }

            Op::MulFloat => {
                let b = vm.pop();
                let a = vm.pop();
                vm.push(Value::float(a.get_float() * b.get_float()));
            }

            Op::DivFloat => {
                let b = vm.pop();
                let a = vm.pop();
                vm.push(Value::float(a.get_float() / b.get_float()));
            }

            Op::NegFloat => {
                let a = vm.pop();
                vm.push(Value::float(-a.get_float()));
            }

            // --- Comparison ---
            Op::Eq => {
                let b = vm.pop();
                let a = vm.pop();
                vm.push(Value::boolean(values_equal(&a, &b)));
            }

            // --- Integer Comparison (type checker guarantees operand types) ---
            Op::LtInt => {
                let b = vm.pop();
                let a = vm.pop();
                vm.push(Value::boolean(a.get_int() < b.get_int()));
            }

            Op::GtInt => {
                let b = vm.pop();
                let a = vm.pop();
                vm.push(Value::boolean(a.get_int() > b.get_int()));
            }

            Op::LeInt => {
                let b = vm.pop();
                let a = vm.pop();
                vm.push(Value::boolean(a.get_int() <= b.get_int()));
            }

            Op::GeInt => {
                let b = vm.pop();
                let a = vm.pop();
                vm.push(Value::boolean(a.get_int() >= b.get_int()));
            }

            // --- Float Comparison (type checker guarantees operand types) ---
            Op::LtFloat => {
                let b = vm.pop();
                let a = vm.pop();
                vm.push(Value::boolean(a.get_float() < b.get_float()));
            }

            Op::GtFloat => {
                let b = vm.pop();
                let a = vm.pop();
                vm.push(Value::boolean(a.get_float() > b.get_float()));
            }

            Op::LeFloat => {
                let b = vm.pop();
                let a = vm.pop();
                vm.push(Value::boolean(a.get_float() <= b.get_float()));
            }

            Op::GeFloat => {
                let b = vm.pop();
                let a = vm.pop();
                vm.push(Value::boolean(a.get_float() >= b.get_float()));
            }

            Op::Not => {
                let a = vm.pop();
                vm.push(Value::boolean(!a.is_truthy()));
            }

            // --- Control flow ---
            Op::Jump => {
                ip = inst.operand as usize;
            }

            Op::JumpIfFalse => {
                let cond = vm.pop();
                if !cond.is_truthy() {
                    ip = inst.operand as usize;
                }
            }

            Op::Return => {
                let result = vm.pop();
                vm.frame_count -= 1;

                // Check if we're returning from a handler dispatch (body or clause)
                if let Some(dispatch) = vm.handler_dispatch_stack.last()
                    && vm.frame_count <= dispatch.dispatch_frame
                {
                    let dispatch = vm.handler_dispatch_stack.pop().unwrap();

                    // Remove the handler frame if still present
                    if let Some(pos) = vm
                        .handler_stack
                        .iter()
                        .position(|h| h.id == dispatch.handler_id)
                    {
                        vm.handler_stack.remove(pos);
                    }

                    if let Some(ret_closure) = dispatch.return_handler {
                        // Apply return handler: push its frame and let the run loop execute it.
                        // When it returns, it will fall through to the normal Return path.
                        let local_count = ret_closure.proto.chunk.local_count as usize;
                        let mut ret_locals = vm.take_locals(local_count);
                        ret_locals[0] = result;
                        let new_frame = CallFrame {
                            closure: ret_closure,
                            ip: 0,
                            locals: ret_locals,
                            open_upvalues: Vec::new(),
                        };
                        vm.push_frame(new_frame);
                        frame_idx = vm.frame_count - 1;
                        ip = 0;
                        cache_frame!();
                        continue;
                    }

                    // No return handler — result is the final value
                    if vm.frame_count <= base_frame_count {
                        return Ok(result);
                    }
                    frame_idx = vm.frame_count - 1;
                    ip = vm.frames[frame_idx].ip;
                    cache_frame!();
                    vm.push(result);
                    continue;
                }

                // Normal return path
                if vm.frame_count <= base_frame_count {
                    return Ok(result);
                }

                frame_idx = vm.frame_count - 1;
                ip = vm.frames[frame_idx].ip;
                cache_frame!();
                vm.push(result);
            }

            // --- Functions ---
            Op::Call => {
                let arity = inst.operand as usize;
                let callee = vm.peek(arity).clone();

                if let Some(obj) = callee.as_object() {
                    match obj {
                        VMObject::Closure(closure) => {
                            let required_arity = closure.proto.arity as usize;

                            if closure.proto.is_variadic {
                                if arity < required_arity {
                                    vm.frames[frame_idx].ip = ip;
                                    return Err(VMSignal::Error(vm.vm_error(
                                        format!(
                                            "Expected at least {} arguments, got {}",
                                            required_arity, arity
                                        ),
                                        &vm.frames[frame_idx],
                                    )));
                                }
                            } else if arity != required_arity {
                                vm.frames[frame_idx].ip = ip;
                                return Err(VMSignal::Error(vm.vm_error(
                                    format!("Expected {} arguments, got {}", required_arity, arity),
                                    &vm.frames[frame_idx],
                                )));
                            }

                            // --- JIT dispatch ---
                            #[cfg(feature = "jit")]
                            {
                                let proto_key = Arc::as_ptr(&closure.proto) as usize;
                                if let Some(&jit_fn) = vm.jit_code.get(&proto_key) {
                                    // We have JIT code — set up locals and call native
                                    let local_count = closure.proto.chunk.local_count as usize;
                                    let mut jit_locals = vm.take_locals(local_count);
                                    for i in (0..arity).rev() {
                                        jit_locals[i] = vm.pop();
                                    }
                                    vm.pop(); // pop callee

                                    let saved_stack_top = vm.stack_top;
                                    vm.frames[frame_idx].ip = ip;
                                    let saved_fc_call = vm.frame_count;
                                    let result = unsafe {
                                        let vm_ptr = vm as *mut VM as *mut u8;
                                        let closure_ptr =
                                            closure.as_ref() as *const Closure as *mut u8;
                                        let locals_ptr = jit_locals.as_mut_ptr() as *mut u8;
                                        jit_fn(vm_ptr, closure_ptr, locals_ptr)
                                    };
                                    // JIT functions don't push frames — restore frame_count
                                    // unless a cross-boundary effect unwound frames.
                                    if vm.pending_effect.is_none() {
                                        vm.frame_count = saved_fc_call;
                                    }

                                    // Check for bail sentinel — JIT function encountered
                                    // a non-tail-resumptive Perform and needs the interpreter
                                    if result.tag == crate::jit::runtime::JIT_BAIL_TAG {
                                        // Restore stack — the JIT function may have
                                        // modified stack_top via internal calls
                                        vm.stack_top = saved_stack_top;

                                        // Check for cross-boundary effect: the perform found
                                        // a handler in this run() scope but the JIT couldn't
                                        // dispatch it. The effect data is in vm.pending_effect.
                                        if let Some(pending) = vm.pending_effect.take() {
                                            drop(jit_locals);

                                            // Propagate as VMSignal::Effect so the
                                            // scheduler / outer run() loop can handle it.
                                            let op_name = if pending.performed_effect.is_empty() {
                                                pending.performed_op.clone()
                                            } else {
                                                format!(
                                                    "{}/{}",
                                                    pending.performed_effect, pending.performed_op
                                                )
                                            };
                                            return Err(VMSignal::Effect(EffectSignal {
                                                op_name,
                                                args: pending.args,
                                                continuation: pending.continuation,
                                                target_handler_id: 0,
                                            }));
                                        }

                                        // Normal bail: fall back to interpreter —
                                        // push a frame with the already-populated
                                        // locals and run from the start
                                        let new_frame = CallFrame {
                                            closure: (**closure).clone(),
                                            ip: 0,
                                            locals: jit_locals,
                                            open_upvalues: Vec::new(),
                                        };
                                        vm.push_frame(new_frame);
                                        frame_idx = vm.frame_count - 1;
                                        ip = 0;
                                        cache_frame!();
                                        continue;
                                    }

                                    // Drop the locals to free any Arc references
                                    drop(jit_locals);
                                    cache_frame!();
                                    let val =
                                        unsafe { Value::from_raw(result.tag, result.payload) };
                                    vm.push(val);
                                    continue;
                                }

                                // Profile and maybe compile
                                let count = vm.jit_call_counts.entry(proto_key).or_insert(0);
                                *count += 1;
                                if *count == crate::jit::JIT_THRESHOLD {
                                    let proto_clone = closure.proto.clone();
                                    if vm.jit_engine.is_none() {
                                        vm.jit_engine = crate::jit::JitEngine::new().ok();
                                    }
                                    if let Some(ref mut engine) = vm.jit_engine
                                        && let Some(jit_fn) = engine.compile(&proto_clone)
                                    {
                                        vm.jit_code.insert(proto_key, jit_fn);
                                    }
                                }
                            }

                            let cloned_closure = (**closure).clone();
                            let local_count = cloned_closure.proto.chunk.local_count as usize;

                            // Reuse locals Vec from dead frame slot when possible
                            let mut locals = vm.take_locals(local_count);

                            if cloned_closure.proto.is_variadic {
                                // Pop all args from stack
                                let mut all_args = vec![Value::nil(); arity];
                                for i in (0..arity).rev() {
                                    all_args[i] = vm.pop();
                                }
                                vm.pop(); // pop callee

                                // Required params go in locals[0..required_arity]
                                locals[..required_arity]
                                    .clone_from_slice(&all_args[..required_arity]);
                                // Excess args packed into a Cons/Nil list in locals[required_arity]
                                let rest_args: Vec<Value> = all_args[required_arity..].to_vec();
                                locals[required_arity] = crate::object::vec_to_cons_list(rest_args);
                            } else {
                                for i in (0..arity).rev() {
                                    locals[i] = vm.pop();
                                }
                                vm.pop(); // pop callee
                            }

                            let new_frame = CallFrame {
                                closure: cloned_closure,
                                ip: 0,
                                open_upvalues: Vec::new(),
                                locals,
                            };

                            // Save current frame back
                            vm.frames[frame_idx].ip = ip;

                            vm.push_frame(new_frame);
                            frame_idx = vm.frame_count - 1;
                            ip = 0;
                            cache_frame!();
                        }

                        VMObject::Builtin { func, .. } => {
                            let func = *func;
                            // Avoid heap allocation for small arities (≤4 args).
                            let result = if arity <= 4 {
                                let mut buf =
                                    [Value::nil(), Value::nil(), Value::nil(), Value::nil()];
                                for i in (0..arity).rev() {
                                    buf[i] = vm.pop();
                                }
                                vm.pop(); // pop callee
                                vm.frames[frame_idx].ip = ip;
                                func(vm, &buf[..arity])
                            } else {
                                let mut args = vec![Value::nil(); arity];
                                for i in (0..arity).rev() {
                                    args[i] = vm.pop();
                                }
                                vm.pop(); // pop callee
                                vm.frames[frame_idx].ip = ip;
                                func(vm, &args)
                            };
                            let result = result.map_err(|mut e| {
                                // Enrich builtin errors with stack trace and source location
                                if e.stack_trace.is_empty() {
                                    e.stack_trace = vm.capture_stack_trace(&vm.frames[frame_idx]);
                                    if e.line == 0 {
                                        e.line = e.stack_trace.first().map(|s| s.line).unwrap_or(0);
                                    }
                                }
                                VMSignal::Error(e)
                            })?;
                            cache_frame!(); // builtins may modify VM state
                            vm.push(result);
                        }

                        VMObject::AdtConstructor {
                            name,
                            arity: expected_arity,
                        } => {
                            let name = name.clone();
                            let expected_arity = *expected_arity;
                            if arity != expected_arity as usize {
                                vm.frames[frame_idx].ip = ip;
                                return Err(VMSignal::Error(vm.vm_error(
                                    format!(
                                        "Constructor {} expects {} arguments, got {}",
                                        name, expected_arity, arity
                                    ),
                                    &vm.frames[frame_idx],
                                )));
                            }
                            let mut args = vec![Value::nil(); arity];
                            for i in (0..arity).rev() {
                                args[i] = vm.pop();
                            }
                            vm.pop(); // pop callee
                            vm.push(Value::object(VMObject::AdtValue {
                                tag: name,
                                fields: args,
                            }));
                        }

                        #[cfg(feature = "ffi")]
                        VMObject::FfiBound {
                            lib_global_name,
                            fn_name,
                        } => {
                            let lib_global = lib_global_name.clone();
                            let fn_name = fn_name.clone();
                            let mut args = vec![Value::nil(); arity];
                            for i in (0..arity).rev() {
                                args[i] = vm.pop();
                            }
                            vm.pop(); // pop callee
                            vm.frames[frame_idx].ip = ip;

                            let closure_mod = vm.frames[frame_idx].closure.module_id;
                            let lib_val = vm
                                .resolve_global(lib_global.as_ref(), closure_mod)
                                .cloned()
                                .ok_or_else(|| {
                                    VMSignal::Error(vm.vm_error(
                                        format!(
                                            "FFI library '{}' not found in globals",
                                            lib_global
                                        ),
                                        &vm.frames[frame_idx],
                                    ))
                                })?;
                            let lib = match lib_val.as_object() {
                                Some(VMObject::FfiLibrary(l)) => l.clone(),
                                _ => {
                                    return Err(VMSignal::Error(vm.vm_error(
                                        format!("'{}' is not an FFI library", lib_global),
                                        &vm.frames[frame_idx],
                                    )));
                                }
                            };
                            let registry = crate::ffi::STRUCT_REGISTRY.lock().unwrap();
                            let result = crate::ffi::ffi_call(&lib, &fn_name, &args, &registry)
                                .map_err(|mut e| {
                                    if e.stack_trace.is_empty() {
                                        e.stack_trace =
                                            vm.capture_stack_trace(&vm.frames[frame_idx]);
                                        if e.line == 0 {
                                            e.line =
                                                e.stack_trace.first().map(|s| s.line).unwrap_or(0);
                                        }
                                    }
                                    VMSignal::Error(e)
                                })?;
                            cache_frame!();
                            vm.push(result);
                        }

                        _ => {
                            let kind = obj.kind_name();
                            vm.frames[frame_idx].ip = ip;
                            return Err(VMSignal::Error(vm.vm_error(
                                format!("Cannot call {}", kind),
                                &vm.frames[frame_idx],
                            )));
                        }
                    }
                } else {
                    vm.frames[frame_idx].ip = ip;
                    return Err(VMSignal::Error(
                        vm.vm_error("Cannot call number", &vm.frames[frame_idx]),
                    ));
                }
            }

            Op::TailCall => {
                let arity = inst.operand as usize;
                let callee = vm.peek(arity).clone();

                if let Some(obj) = callee.as_object() {
                    match obj {
                        VMObject::Closure(closure) => {
                            let required_arity = closure.proto.arity as usize;

                            if closure.proto.is_variadic {
                                if arity < required_arity {
                                    vm.frames[frame_idx].ip = ip;
                                    return Err(VMSignal::Error(vm.vm_error(
                                        format!(
                                            "Expected at least {} arguments, got {}",
                                            required_arity, arity
                                        ),
                                        &vm.frames[frame_idx],
                                    )));
                                }
                            } else if arity != required_arity {
                                vm.frames[frame_idx].ip = ip;
                                return Err(VMSignal::Error(vm.vm_error(
                                    format!("Expected {} arguments, got {}", required_arity, arity),
                                    &vm.frames[frame_idx],
                                )));
                            }

                            // --- JIT dispatch for tail calls ---
                            #[cfg(feature = "jit")]
                            {
                                let proto_key = Arc::as_ptr(&closure.proto) as usize;
                                if !vm.jit_code.contains_key(&proto_key) {
                                    // Profile: increment call counter for JIT compilation
                                    let count = vm.jit_call_counts.entry(proto_key).or_insert(0);
                                    *count += 1;
                                    if *count == crate::jit::JIT_THRESHOLD {
                                        let proto_clone = closure.proto.clone();
                                        if vm.jit_engine.is_none() {
                                            vm.jit_engine = crate::jit::JitEngine::new().ok();
                                        }
                                        if let Some(ref mut engine) = vm.jit_engine
                                            && let Some(jit_fn) = engine.compile(&proto_clone)
                                        {
                                            vm.jit_code.insert(proto_key, jit_fn);
                                        }
                                    }
                                }
                                if let Some(&jit_fn) = vm.jit_code.get(&proto_key) {
                                    let local_count = closure.proto.chunk.local_count as usize;
                                    let mut jit_locals =
                                        std::mem::take(&mut vm.frames[frame_idx].locals);
                                    jit_locals.clear();
                                    jit_locals.resize(local_count, Value::nil());
                                    for i in (0..arity).rev() {
                                        jit_locals[i] = vm.pop();
                                    }
                                    vm.pop(); // pop callee

                                    let saved_stack_top_tc = vm.stack_top;
                                    vm.frames[frame_idx].ip = ip;
                                    let saved_fc_tc = vm.frame_count;
                                    let result = unsafe {
                                        let vm_ptr = vm as *mut VM as *mut u8;
                                        let closure_ptr =
                                            closure.as_ref() as *const Closure as *mut u8;
                                        let locals_ptr = jit_locals.as_mut_ptr() as *mut u8;
                                        jit_fn(vm_ptr, closure_ptr, locals_ptr)
                                    };
                                    // JIT functions don't push frames — restore frame_count
                                    // unless a cross-boundary effect unwound frames.
                                    if vm.pending_effect.is_none() {
                                        vm.frame_count = saved_fc_tc;
                                    }

                                    // Check for bail sentinel
                                    if result.tag == crate::jit::runtime::JIT_BAIL_TAG {
                                        // Restore stack — JIT may have leaked values
                                        vm.stack_top = saved_stack_top_tc;

                                        // Check for cross-boundary effect
                                        if let Some(pending) = vm.pending_effect.take() {
                                            drop(jit_locals);

                                            // Propagate as VMSignal::Effect so the
                                            // scheduler / outer run() loop can handle it.
                                            // The continuation in the pending effect
                                            // already captured the full frame/handler state.
                                            let op_name = if pending.performed_effect.is_empty() {
                                                pending.performed_op.clone()
                                            } else {
                                                format!(
                                                    "{}/{}",
                                                    pending.performed_effect, pending.performed_op
                                                )
                                            };
                                            return Err(VMSignal::Effect(EffectSignal {
                                                op_name,
                                                args: pending.args,
                                                continuation: pending.continuation,
                                                target_handler_id: 0,
                                            }));
                                        }

                                        // Normal bail: fall back to interpreter —
                                        // replace current frame with the callee
                                        vm.frames[frame_idx].closure = (**closure).clone();
                                        vm.frames[frame_idx].ip = 0;
                                        vm.frames[frame_idx].locals = jit_locals;
                                        vm.frames[frame_idx].open_upvalues.clear();
                                        ip = 0;
                                        cache_frame!();
                                        continue;
                                    }

                                    // Restore locals to frame for reuse
                                    vm.frames[frame_idx].locals = jit_locals;

                                    let val =
                                        unsafe { Value::from_raw(result.tag, result.payload) };
                                    // Tail call: return the result (pop current frame)
                                    vm.frame_count -= 1;
                                    if vm.frame_count <= base_frame_count {
                                        return Ok(val);
                                    }
                                    frame_idx = vm.frame_count - 1;
                                    ip = vm.frames[frame_idx].ip;
                                    cache_frame!();
                                    vm.push(val);
                                    continue;
                                }
                            }

                            // Reuse current frame's locals Vec (avoids allocation)
                            let mut locals = std::mem::take(&mut vm.frames[frame_idx].locals);
                            let local_count = closure.proto.chunk.local_count as usize;
                            locals.clear();
                            locals.resize(local_count, Value::nil());

                            if closure.proto.is_variadic {
                                let mut all_args = vec![Value::nil(); arity];
                                for i in (0..arity).rev() {
                                    all_args[i] = vm.pop();
                                }
                                vm.pop(); // pop callee
                                locals[..required_arity]
                                    .clone_from_slice(&all_args[..required_arity]);
                                let rest_args: Vec<Value> = all_args[required_arity..].to_vec();
                                locals[required_arity] = crate::object::vec_to_cons_list(rest_args);
                            } else {
                                for i in (0..arity).rev() {
                                    locals[i] = vm.pop();
                                }
                                vm.pop(); // pop callee
                            }

                            // Reuse current frame
                            vm.frames[frame_idx].closure = (**closure).clone();
                            ip = 0;
                            vm.frames[frame_idx].open_upvalues.clear();
                            vm.frames[frame_idx].locals = locals;
                            cache_frame!();
                        }

                        VMObject::Builtin { func, .. } => {
                            let func = *func;
                            // Avoid heap allocation for small arities (≤4 args).
                            let result = if arity <= 4 {
                                let mut buf =
                                    [Value::nil(), Value::nil(), Value::nil(), Value::nil()];
                                for i in (0..arity).rev() {
                                    buf[i] = vm.pop();
                                }
                                vm.pop(); // pop callee
                                vm.frames[frame_idx].ip = ip;
                                func(vm, &buf[..arity])
                            } else {
                                let mut args = vec![Value::nil(); arity];
                                for i in (0..arity).rev() {
                                    args[i] = vm.pop();
                                }
                                vm.pop(); // pop callee
                                vm.frames[frame_idx].ip = ip;
                                func(vm, &args)
                            };
                            let result = result.map_err(|mut e| {
                                if e.stack_trace.is_empty() {
                                    e.stack_trace = vm.capture_stack_trace(&vm.frames[frame_idx]);
                                    if e.line == 0 {
                                        e.line = e.stack_trace.first().map(|s| s.line).unwrap_or(0);
                                    }
                                }
                                VMSignal::Error(e)
                            })?;

                            vm.frame_count -= 1;
                            if vm.frame_count <= base_frame_count {
                                return Ok(result);
                            }
                            frame_idx = vm.frame_count - 1;
                            ip = vm.frames[frame_idx].ip;
                            cache_frame!();
                            vm.push(result);
                        }

                        VMObject::AdtConstructor {
                            name,
                            arity: expected_arity,
                        } => {
                            let name = name.clone();
                            let expected_arity = *expected_arity;
                            if arity != expected_arity as usize {
                                vm.frames[frame_idx].ip = ip;
                                return Err(VMSignal::Error(vm.vm_error(
                                    format!(
                                        "Constructor {} expects {} arguments, got {}",
                                        name, expected_arity, arity
                                    ),
                                    &vm.frames[frame_idx],
                                )));
                            }
                            let mut args = vec![Value::nil(); arity];
                            for i in (0..arity).rev() {
                                args[i] = vm.pop();
                            }
                            vm.pop();
                            let result = Value::object(VMObject::AdtValue {
                                tag: name,
                                fields: args,
                            });

                            vm.frame_count -= 1;
                            if vm.frame_count <= base_frame_count {
                                return Ok(result);
                            }
                            frame_idx = vm.frame_count - 1;
                            ip = vm.frames[frame_idx].ip;
                            cache_frame!();
                            vm.push(result);
                        }

                        #[cfg(feature = "ffi")]
                        VMObject::FfiBound {
                            lib_global_name,
                            fn_name,
                        } => {
                            let lib_global = lib_global_name.clone();
                            let fn_name = fn_name.clone();
                            let mut args = vec![Value::nil(); arity];
                            for i in (0..arity).rev() {
                                args[i] = vm.pop();
                            }
                            vm.pop(); // pop callee
                            vm.frames[frame_idx].ip = ip;

                            let closure_mod = vm.frames[frame_idx].closure.module_id;
                            let lib_val = vm
                                .resolve_global(lib_global.as_ref(), closure_mod)
                                .cloned()
                                .ok_or_else(|| {
                                    VMSignal::Error(vm.vm_error(
                                        format!(
                                            "FFI library '{}' not found in globals",
                                            lib_global
                                        ),
                                        &vm.frames[frame_idx],
                                    ))
                                })?;
                            let lib = match lib_val.as_object() {
                                Some(VMObject::FfiLibrary(l)) => l.clone(),
                                _ => {
                                    return Err(VMSignal::Error(vm.vm_error(
                                        format!("'{}' is not an FFI library", lib_global),
                                        &vm.frames[frame_idx],
                                    )));
                                }
                            };
                            let registry = crate::ffi::STRUCT_REGISTRY.lock().unwrap();
                            let result = crate::ffi::ffi_call(&lib, &fn_name, &args, &registry)
                                .map_err(|mut e| {
                                    if e.stack_trace.is_empty() {
                                        e.stack_trace =
                                            vm.capture_stack_trace(&vm.frames[frame_idx]);
                                        if e.line == 0 {
                                            e.line =
                                                e.stack_trace.first().map(|s| s.line).unwrap_or(0);
                                        }
                                    }
                                    VMSignal::Error(e)
                                })?;

                            vm.frame_count -= 1;
                            if vm.frame_count <= base_frame_count {
                                return Ok(result);
                            }
                            frame_idx = vm.frame_count - 1;
                            ip = vm.frames[frame_idx].ip;
                            cache_frame!();
                            vm.push(result);
                        }

                        _ => {
                            let kind = obj.kind_name();
                            vm.frames[frame_idx].ip = ip;
                            return Err(VMSignal::Error(vm.vm_error(
                                format!("Cannot call {}", kind),
                                &vm.frames[frame_idx],
                            )));
                        }
                    }
                } else {
                    vm.frames[frame_idx].ip = ip;
                    return Err(VMSignal::Error(
                        vm.vm_error("Cannot call number", &vm.frames[frame_idx]),
                    ));
                }
            }

            Op::Closure => {
                // SAFETY: operand is a valid constant index
                let proto = match unsafe { &*constants.add(inst.operand as usize) } {
                    ConstantValue::Prototype(p) => p,
                    _ => {
                        vm.frames[frame_idx].ip = ip;
                        return Err(VMSignal::Error(vm.vm_error(
                            "CLOSURE expects a prototype constant",
                            &vm.frames[frame_idx],
                        )));
                    }
                };

                let closure = create_closure_from_proto(&mut vm.frames[frame_idx], proto);
                vm.push(Value::object(VMObject::Closure(Box::new(closure))));
            }

            // --- ADTs ---
            Op::Construct => {
                // SAFETY: operand is a valid constant index
                let name = match unsafe { &*constants.add(inst.operand as usize) } {
                    ConstantValue::String(s) => s.clone(),
                    _ => {
                        vm.frames[frame_idx].ip = ip;
                        return Err(VMSignal::Error(
                            vm.vm_error("CONSTRUCT name must be string", &vm.frames[frame_idx]),
                        ));
                    }
                };
                let arity = inst.operand2 as usize;
                let mut args = vec![Value::nil(); arity];
                for i in (0..arity).rev() {
                    args[i] = vm.pop();
                }
                vm.push(Value::object(VMObject::AdtValue {
                    tag: name,
                    fields: args,
                }));
            }

            Op::GetField => {
                let field_idx = inst.operand as usize;
                let value = vm.peek(0).clone();

                if let Some(obj) = value.as_object() {
                    match obj {
                        VMObject::AdtValue { fields, tag, .. } => {
                            if field_idx >= fields.len() {
                                vm.frames[frame_idx].ip = ip;
                                return Err(VMSignal::Error(vm.vm_error(
                                    format!("Field index {} out of bounds for {}", field_idx, tag),
                                    &vm.frames[frame_idx],
                                )));
                            }
                            let field = fields[field_idx].clone();
                            vm.push(field);
                        }
                        _ => {
                            vm.frames[frame_idx].ip = ip;
                            return Err(VMSignal::Error(vm.vm_error(
                                format!("GET_FIELD on non-ADT value: {}", obj.kind_name()),
                                &vm.frames[frame_idx],
                            )));
                        }
                    }
                } else {
                    vm.frames[frame_idx].ip = ip;
                    return Err(VMSignal::Error(
                        vm.vm_error("GET_FIELD on number", &vm.frames[frame_idx]),
                    ));
                }
            }

            Op::MatchTag => {
                // SAFETY: operand is a valid constant index
                let tag_name = match unsafe { &*constants.add(inst.operand as usize) } {
                    ConstantValue::String(s) => s.as_str(),
                    _ => {
                        vm.frames[frame_idx].ip = ip;
                        return Err(VMSignal::Error(
                            vm.vm_error("MATCH_TAG name must be string", &vm.frames[frame_idx]),
                        ));
                    }
                };
                let target = inst.operand2 as usize;
                let value = vm.peek(0);

                if value.is_int() || value.is_float() {
                    ip = target;
                } else if value.is_nil() {
                    if tag_name == "Nil" {
                        // match
                    } else {
                        ip = target;
                    }
                } else if let Some(obj) = value.as_object() {
                    let mut matches = false;
                    match obj {
                        VMObject::AdtValue { tag, .. } if tag == tag_name => {
                            matches = true;
                        }
                        _ => {}
                    }
                    if !matches {
                        ip = target;
                    }
                } else {
                    ip = target;
                }
            }

            // --- Effects ---
            Op::PushHandler => {
                // Use cached constants for handler descriptor lookup
                // SAFETY: operand is a valid constant index
                let descriptor_str = match unsafe { &*constants.add(inst.operand as usize) } {
                    ConstantValue::String(s) if s.starts_with("\0handler:") => s.clone(),
                    _ => {
                        vm.frames[frame_idx].ip = ip;
                        return Err(VMSignal::Error(
                            vm.vm_error("Invalid handler descriptor", &vm.frames[frame_idx]),
                        ));
                    }
                };

                // Cache the parsed descriptor to avoid re-parsing JSON on every PushHandler.
                // Use the constant pool address + operand index as a stable key.
                let cache_key = (constants as usize) + inst.operand as usize;
                let descriptor = if let Some(cached) = vm.handler_descriptor_cache.get(&cache_key) {
                    cached.clone()
                } else {
                    let json = &descriptor_str["\0handler:".len()..];
                    let parsed = match parse_handler_descriptor(json) {
                        Ok(d) => d,
                        Err(e) => {
                            vm.frames[frame_idx].ip = ip;
                            return Err(VMSignal::Error(vm.vm_error(
                                format!("Failed to parse handler descriptor: {}", e),
                                &vm.frames[frame_idx],
                            )));
                        }
                    };
                    vm.handler_descriptor_cache
                        .insert(cache_key, parsed.clone());
                    parsed
                };

                // Create body closure from prototype
                // SAFETY: descriptor indices are valid constant indices
                let body_proto =
                    match unsafe { &*constants.add(descriptor.body_proto_idx as usize) } {
                        ConstantValue::Prototype(p) => p,
                        _ => {
                            vm.frames[frame_idx].ip = ip;
                            return Err(VMSignal::Error(
                                vm.vm_error("Expected body prototype", &vm.frames[frame_idx]),
                            ));
                        }
                    };
                let body_closure = create_closure_from_proto(&mut vm.frames[frame_idx], body_proto);

                // Create handler clause closures
                let mut handler_entries = Vec::new();
                for h in &descriptor.handlers {
                    let effect_name = match unsafe { &*constants.add(h.effect_name_idx as usize) } {
                        ConstantValue::String(s) => s.clone(),
                        _ => {
                            vm.frames[frame_idx].ip = ip;
                            return Err(VMSignal::Error(
                                vm.vm_error("Invalid handler clause", &vm.frames[frame_idx]),
                            ));
                        }
                    };
                    let op_name = match unsafe { &*constants.add(h.op_name_idx as usize) } {
                        ConstantValue::String(s) => s.clone(),
                        _ => {
                            vm.frames[frame_idx].ip = ip;
                            return Err(VMSignal::Error(
                                vm.vm_error("Invalid handler clause", &vm.frames[frame_idx]),
                            ));
                        }
                    };
                    let proto = match unsafe { &*constants.add(h.proto_idx as usize) } {
                        ConstantValue::Prototype(p) => p,
                        _ => {
                            vm.frames[frame_idx].ip = ip;
                            return Err(VMSignal::Error(
                                vm.vm_error("Invalid handler clause", &vm.frames[frame_idx]),
                            ));
                        }
                    };
                    let tail_resume_closure = if h.tail_resume_proto_idx >= 0 {
                        match unsafe { &*constants.add(h.tail_resume_proto_idx as usize) } {
                            ConstantValue::Prototype(p) => {
                                Some(create_closure_from_proto(&mut vm.frames[frame_idx], p))
                            }
                            _ => None,
                        }
                    } else {
                        None
                    };
                    handler_entries.push(HandlerEntry {
                        effect_name,
                        op_name,
                        closure: create_closure_from_proto(&mut vm.frames[frame_idx], proto),
                        arity: h.arity,
                        tail_resume_closure,
                        is_aborting: h.is_aborting,
                    });
                }

                // Create return handler closure
                let return_closure = if descriptor.return_proto_idx >= 0 {
                    match unsafe { &*constants.add(descriptor.return_proto_idx as usize) } {
                        ConstantValue::Prototype(p) => {
                            Some(create_closure_from_proto(&mut vm.frames[frame_idx], p))
                        }
                        _ => {
                            vm.frames[frame_idx].ip = ip;
                            return Err(VMSignal::Error(vm.vm_error(
                                "Expected return handler prototype",
                                &vm.frames[frame_idx],
                            )));
                        }
                    }
                } else {
                    None
                };

                // Save current frame
                vm.frames[frame_idx].ip = ip;

                // CPS dispatch: install handler frame and call body thunk inline.
                // No Rust-level recursion — the handler dispatch is entirely in VM state.
                let handler_id = next_handler_id();
                let is_tail_resumptive = descriptor.tail_resumptive
                    && handler_entries
                        .iter()
                        .all(|e| e.tail_resume_closure.is_some());
                let handler_frame = VMHandlerFrame {
                    handlers: handler_entries.clone(),
                    return_handler: return_closure.clone(),
                    unwind_frame_start: vm.frame_count,
                    unwind_stack_start: vm.stack_top,
                    capture_frame_start: vm.frame_count,
                    capture_stack_start: vm.stack_top,
                    id: handler_id,
                    is_scheduler: false,
                    is_tail_resumptive,
                };
                vm.handler_stack.push(handler_frame);

                // Push handler dispatch marker — tells Op::Return when the body/clause returns
                vm.handler_dispatch_stack.push(HandlerDispatch {
                    dispatch_frame: vm.frame_count,
                    handler_entries,
                    return_handler: return_closure,
                    handler_id,
                });

                // Call body thunk inline (same as Op::Call for a 0-arity closure)
                let local_count = body_closure.proto.chunk.local_count as usize;
                let locals = vm.take_locals(local_count);
                let new_frame = CallFrame {
                    closure: body_closure,
                    ip: 0,
                    locals,
                    open_upvalues: Vec::new(),
                };
                vm.push_frame(new_frame);
                frame_idx = vm.frame_count - 1;
                ip = 0;
                cache_frame!();
            }

            Op::PopHandler => {
                // Remove the topmost user handler frame from the handler stack.
                // This is emitted after the body thunk of a `handle` expression completes
                // without performing any handled effects.
                if let Some(pos) = vm.handler_stack.iter().rposition(|h| !h.is_scheduler) {
                    vm.handler_stack.remove(pos);
                }
            }

            Op::Perform => {
                // SAFETY: operand is a valid constant index
                // Borrow effect name from the constant pool — avoids a String clone per perform.
                // Only allocate an owned String when building EffectSignal (scheduler path) or
                // error messages.
                let op_name_ref: &str = match unsafe { &*constants.add(inst.operand as usize) } {
                    ConstantValue::String(s) => s.as_str(),
                    _ => {
                        vm.frames[frame_idx].ip = ip;
                        return Err(VMSignal::Error(
                            vm.vm_error("PERFORM name must be string", &vm.frames[frame_idx]),
                        ));
                    }
                };
                let arity = inst.operand2 as usize;

                // Parse the performed operation name (could be "effectName/opName" or just "opName")
                // Use rsplit_once to handle effect names with slashes (e.g., "io/IO")
                // Use borrowed &str to avoid String allocation on every Perform.
                let (performed_effect, performed_op) =
                    if let Some((eff, op)) = op_name_ref.rsplit_once('/') {
                        (eff, op)
                    } else {
                        ("", op_name_ref)
                    };

                // Walk handler stack to find a matching handler AND the entry index.
                // Combined search avoids scanning entries twice (once for handler, once for entry).
                let mut found: Option<(usize, usize)> = None; // (handler_idx, entry_idx)
                'handler_search: for i in (0..vm.handler_stack.len()).rev() {
                    for (j, e) in vm.handler_stack[i].handlers.iter().enumerate() {
                        let matches = if e.effect_name.is_empty() {
                            e.op_name == performed_op
                        } else {
                            e.effect_name == performed_effect && e.op_name == performed_op
                        };
                        if matches {
                            found = Some((i, j));
                            break 'handler_search;
                        }
                    }
                }

                if let Some((handler_idx, entry_idx)) = found {
                    let is_scheduler = vm.handler_stack[handler_idx].is_scheduler;

                    // ── Tail-resumptive fast path ──
                    // When the handler is tail-resumptive, the handler clause is of the
                    // form `((op args k) -> (resume k <expr>))`. Skip continuation capture
                    // entirely: call the resume-value closure inline, push the result.
                    // The handler stays installed (deep handler semantics).
                    if !is_scheduler
                        && vm.handler_stack[handler_idx].is_tail_resumptive
                        && let Some(resume_closure) = vm.handler_stack[handler_idx].handlers
                            [entry_idx]
                            .tail_resume_closure
                            .clone()
                    {
                        // Save current frame before calling the resume-value closure
                        vm.frames[frame_idx].ip = ip;

                        // Set up locals for the tail-resume closure (effect args only, no k)
                        let local_count = resume_closure.proto.chunk.local_count as usize;
                        let mut resume_locals = vm.take_locals(local_count);
                        // Pop args from stack directly into locals (no intermediate Vec)
                        let copy_count = arity.min(local_count);
                        for k in (0..copy_count).rev() {
                            resume_locals[k] = vm.pop();
                        }
                        // Pop remaining args if arity > local_count
                        for _ in 0..(arity.saturating_sub(local_count)) {
                            vm.pop();
                        }

                        // Call the resume-value closure inline
                        let new_frame = CallFrame {
                            closure: resume_closure,
                            ip: 0,
                            open_upvalues: Vec::new(),
                            locals: resume_locals,
                        };
                        vm.push_frame(new_frame);
                        frame_idx = vm.frame_count - 1;
                        ip = 0;
                        cache_frame!();
                        continue;
                    }

                    // Pop args into a Vec for the non-fast-path cases.
                    // (The tail-resumptive fast path above already handled args directly.)
                    let mut args = vec![Value::nil(); arity];
                    for i in (0..arity).rev() {
                        args[i] = vm.pop();
                    }

                    // Fast-path: synchronous effects under the scheduler.
                    if is_scheduler
                        && let Some(result) = try_fast_path_handler(vm, op_name_ref, &args)
                    {
                        vm.push(result);
                        continue;
                    }

                    // ── Aborting handler fast path ──
                    // When the matching handler clause doesn't reference k (is_aborting),
                    // the continuation will be discarded. Skip the expensive capture
                    // (cloning frames, stack, handlers) and just unwind + dispatch.
                    if !is_scheduler
                        && vm.handler_stack[handler_idx].handlers[entry_idx].is_aborting
                    {
                        let clause = vm.handler_stack[handler_idx].handlers[entry_idx]
                            .closure
                            .clone();
                        let clause_arity = vm.handler_stack[handler_idx].handlers[entry_idx].arity;
                        let handler_entries = vm.handler_stack[handler_idx].handlers.clone();
                        let return_handler = vm.handler_stack[handler_idx].return_handler.clone();
                        let _handler_id = vm.handler_stack[handler_idx].id;
                        let capture_frame_start = vm.handler_stack[handler_idx].capture_frame_start;
                        let unwind_fc = vm.handler_stack[handler_idx].unwind_frame_start;
                        let unwind_st = vm.handler_stack[handler_idx].unwind_stack_start;

                        // Save current frame
                        vm.frames[frame_idx].ip = ip;

                        // Unwind VM state (no capture!)
                        vm.clear_stack_slots(unwind_st);
                        vm.frame_count = unwind_fc;
                        vm.stack_top = unwind_st;
                        vm.handler_stack.truncate(handler_idx);

                        // Remove handler dispatch markers for captured frames
                        vm.handler_dispatch_stack
                            .retain(|d| d.dispatch_frame < capture_frame_start);

                        // Deep handler semantics: re-install handler with new ID
                        let new_handler_id = next_handler_id();
                        let new_handler_frame = VMHandlerFrame {
                            handlers: handler_entries.clone(),
                            return_handler: return_handler.clone(),
                            unwind_frame_start: vm.frame_count,
                            unwind_stack_start: vm.stack_top,
                            capture_frame_start: vm.frame_count,
                            capture_stack_start: vm.stack_top,
                            id: new_handler_id,
                            is_scheduler: false,
                            is_tail_resumptive: false,
                        };
                        vm.handler_stack.push(new_handler_frame);

                        // Push handler dispatch marker
                        vm.handler_dispatch_stack.push(HandlerDispatch {
                            dispatch_frame: vm.frame_count,
                            handler_entries,
                            return_handler,
                            handler_id: new_handler_id,
                        });

                        // Build clause arguments: [effect_args..., nil_continuation]
                        let expected_arg_count = (clause_arity as usize).saturating_sub(1);
                        let local_count = clause.proto.chunk.local_count as usize;
                        let mut clause_locals = vm.take_locals(local_count);
                        let copy_count = expected_arg_count.min(args.len());
                        clause_locals[..copy_count].clone_from_slice(&args[..copy_count]);
                        // Pass nil as the continuation (aborting handler won't use it)
                        clause_locals[expected_arg_count] = Value::nil();

                        // Call handler clause inline
                        let new_frame = CallFrame {
                            closure: clause,
                            ip: 0,
                            open_upvalues: Vec::new(),
                            locals: clause_locals,
                        };
                        vm.push_frame(new_frame);
                        frame_idx = vm.frame_count - 1;
                        ip = 0;
                        cache_frame!();
                        continue;
                    }

                    let handler_id = vm.handler_stack[handler_idx].id;
                    let capture_frame_start = vm.handler_stack[handler_idx].capture_frame_start;
                    let capture_stack_start = vm.handler_stack[handler_idx].capture_stack_start;
                    let unwind_fc = vm.handler_stack[handler_idx].unwind_frame_start;
                    let unwind_st = vm.handler_stack[handler_idx].unwind_stack_start;

                    // Extract handler data BEFORE unwinding — only needed for user handler
                    // CPS dispatch. The scheduler path never uses these, so skip the
                    // expensive clone (25 HandlerEntry with String + Closure each).
                    let (handler_entries, return_handler) = if is_scheduler {
                        (Vec::new(), None)
                    } else {
                        (
                            vm.handler_stack[handler_idx].handlers.clone(),
                            vm.handler_stack[handler_idx].return_handler.clone(),
                        )
                    };

                    // Save current frame
                    vm.frames[frame_idx].ip = ip;

                    // Capture continuation
                    let mut captured_frames = Vec::new();
                    for f_idx in capture_frame_start..vm.frame_count {
                        let f = &vm.frames[f_idx];
                        captured_frames.push(CallFrame {
                            closure: f.closure.clone(),
                            ip: f.ip,
                            locals: f.locals.clone(),
                            open_upvalues: f.open_upvalues.clone(),
                        });
                    }
                    let captured_stack: Vec<Value> =
                        vm.stack[capture_stack_start..vm.stack_top].to_vec();

                    // Capture nested handler frames (above the matched handler)
                    let captured_handlers: Vec<VMHandlerFrame> =
                        vm.handler_stack[handler_idx + 1..].to_vec();

                    // Capture handler dispatch markers that belong to the captured frames,
                    // but exclude the dispatch marker for the handler being dispatched itself
                    // (it belongs to this handler's lifecycle, not the continuation's).
                    let captured_dispatches: Vec<HandlerDispatch> = vm
                        .handler_dispatch_stack
                        .iter()
                        .filter(|d| {
                            d.dispatch_frame >= capture_frame_start && d.handler_id != handler_id
                        })
                        .cloned()
                        .collect();

                    let continuation = VMContinuation {
                        frames: captured_frames,
                        stack: captured_stack,
                        handler_stack: captured_handlers,
                        handler_dispatch_stack: captured_dispatches,
                        id: next_continuation_id(),
                        frame_base: capture_frame_start,
                        stack_base: capture_stack_start,
                    };

                    let cont_value = Value::object(VMObject::Continuation(Box::new(continuation)));

                    // Unwind VM state
                    vm.clear_stack_slots(unwind_st);
                    vm.frame_count = unwind_fc;
                    vm.stack_top = unwind_st;
                    vm.handler_stack.truncate(handler_idx); // remove this handler and everything above

                    // Remove handler dispatch markers that belonged to captured frames
                    vm.handler_dispatch_stack
                        .retain(|d| d.dispatch_frame < capture_frame_start);

                    // ── Cross-boundary guard (JIT bail path) ──
                    // When the handler's unwind target is below this `run()`'s base
                    // frame count, the CPS dispatch cannot happen here — it would
                    // set `frame_count` below `base`, causing this `run()` to exit
                    // with the handler's result while the outer interpreter still
                    // expects normal control flow. Instead, store the fully-captured
                    // effect in `vm.pending_effect` and propagate via VMSignal::Effect
                    // so the signal can reach the outer `run()` loop for dispatch.
                    // This applies to BOTH scheduler and user handlers.
                    #[cfg(feature = "jit")]
                    if unwind_fc < base_frame_count {
                        vm.pending_effect = Some(PendingEffect {
                            args,
                            continuation: cont_value,
                            performed_effect: performed_effect.to_string(),
                            performed_op: performed_op.to_string(),
                        });
                        return Err(VMSignal::Effect(EffectSignal {
                            op_name: op_name_ref.to_string(),
                            args: Vec::new(),           // args are in pending_effect
                            continuation: Value::nil(), // continuation is in pending_effect
                            target_handler_id: handler_id,
                        }));
                    }

                    if is_scheduler {
                        // Scheduler handler: propagate via EffectSignal (existing path).
                        // Only allocate the owned String here — the hot path above
                        // used a borrowed &str from the constant pool.
                        return Err(VMSignal::Effect(EffectSignal {
                            op_name: op_name_ref.to_string(),
                            args,
                            continuation: cont_value,
                            target_handler_id: handler_id,
                        }));
                    }

                    // ── User handler: CPS inline dispatch ──
                    // Find the matching handler clause
                    let clause = handler_entries
                        .iter()
                        .find(|e| {
                            if e.effect_name.is_empty() {
                                e.op_name == performed_op
                            } else {
                                e.effect_name == performed_effect && e.op_name == performed_op
                            }
                        })
                        .cloned();
                    let clause = match clause {
                        Some(c) => c,
                        None => {
                            // Should not happen — we found the handler by name
                            return Err(VMSignal::Error(VMError::runtime(format!(
                                "Handler clause not found for effect: {}",
                                op_name_ref
                            ))));
                        }
                    };

                    // Deep handler semantics: re-install the handler frame with a NEW id
                    let new_handler_id = next_handler_id();
                    let new_handler_frame = VMHandlerFrame {
                        handlers: handler_entries.clone(),
                        return_handler: return_handler.clone(),
                        unwind_frame_start: vm.frame_count,
                        unwind_stack_start: vm.stack_top,
                        capture_frame_start: vm.frame_count,
                        capture_stack_start: vm.stack_top,
                        id: new_handler_id,
                        is_scheduler: false,
                        is_tail_resumptive: false, // normal (non-fast-path) dispatch
                    };
                    vm.handler_stack.push(new_handler_frame);

                    // Push handler dispatch marker for the clause invocation
                    vm.handler_dispatch_stack.push(HandlerDispatch {
                        dispatch_frame: vm.frame_count,
                        handler_entries,
                        return_handler,
                        handler_id: new_handler_id,
                    });

                    // Build handler clause arguments: [effect_args..., continuation]
                    let expected_arg_count = (clause.arity as usize).saturating_sub(1);
                    let local_count = clause.closure.proto.chunk.local_count as usize;
                    let mut clause_locals = vm.take_locals(local_count);
                    let copy_count = expected_arg_count.min(args.len());
                    clause_locals[..copy_count].clone_from_slice(&args[..copy_count]);
                    clause_locals[expected_arg_count] = cont_value;

                    // Call handler clause inline
                    let new_frame = CallFrame {
                        closure: clause.closure,
                        ip: 0,
                        open_upvalues: Vec::new(),
                        locals: clause_locals,
                    };
                    vm.push_frame(new_frame);
                    frame_idx = vm.frame_count - 1;
                    ip = 0;
                    cache_frame!();
                    continue;
                }

                // No handler found at all — error
                // Pop remaining args from stack
                let mut err_args = Vec::new();
                for _ in 0..arity {
                    err_args.push(vm.pop());
                }
                err_args.reverse();
                if op_name_ref == "throw" || performed_op == "throw" {
                    // Better error message for unhandled throw
                    let msg = err_args
                        .first()
                        .map(format_value)
                        .unwrap_or_else(|| "(no message)".to_string());
                    vm.frames[frame_idx].ip = ip;
                    return Err(VMSignal::Error(vm.vm_error(
                        format!("Uncaught exception: {}", msg),
                        &vm.frames[frame_idx],
                    )));
                } else {
                    vm.frames[frame_idx].ip = ip;
                    return Err(VMSignal::Error(vm.vm_error(
                        format!("Unhandled effect: {}", op_name_ref),
                        &vm.frames[frame_idx],
                    )));
                }
            }

            Op::Resume => {
                let value = vm.pop();
                let cont_val = vm.pop();

                // Check type and get continuation ID for one-shot enforcement
                let cont_id = match cont_val.as_object() {
                    Some(VMObject::Continuation(c)) => c.id,
                    _ => {
                        vm.frames[frame_idx].ip = ip;
                        return Err(VMSignal::Error(
                            vm.vm_error("resume expects a continuation", &vm.frames[frame_idx]),
                        ));
                    }
                };

                // Enforce one-shot semantics via VM-level tracking (works across Arc aliases)
                if !vm.resumed_continuations.insert(cont_id) {
                    vm.frames[frame_idx].ip = ip;
                    return Err(VMSignal::Error(vm.vm_error(
                        "Continuation already resumed (one-shot)",
                        &vm.frames[frame_idx],
                    )));
                }

                // Try to take ownership of the continuation (avoids deep clone
                // when this is the sole reference — the common case for one-shot
                // continuations).
                let cont_data = match cont_val.try_take_object() {
                    Ok(VMObject::Continuation(boxed)) => *boxed,
                    Ok(_) => unreachable!("checked Continuation above"),
                    Err(val) => {
                        // Fallback: refcount > 1, must clone
                        match val.as_object() {
                            Some(VMObject::Continuation(c)) => (**c).clone(),
                            _ => unreachable!("checked Continuation above"),
                        }
                    }
                };

                // Update the current (top) handler frame's capture/unwind points
                // so future PERFORMs capture from the right place.
                vm.frames[frame_idx].ip = ip;
                if let Some(top_handler) = vm.handler_stack.last_mut() {
                    top_handler.capture_frame_start = vm.frame_count;
                    top_handler.capture_stack_start = vm.stack_top;
                    top_handler.unwind_frame_start = vm.frame_count;
                    top_handler.unwind_stack_start = vm.stack_top;
                }

                // Save metadata before consuming
                let cont_stack_len = cont_data.stack.len();
                let cont_frame_base = cont_data.frame_base;
                let cont_stack_base = cont_data.stack_base;

                // Restore continuation frames on top of current frames (consuming — no clone)
                let resume_base = vm.frame_count;
                for f in cont_data.frames {
                    vm.push_frame(f);
                }

                // Restore continuation stack + push resume value (consuming — no clone)
                for v in cont_data.stack {
                    vm.push(v);
                }
                vm.push(value);

                // Restore nested handlers, rebasing indices
                let frame_offset = resume_base as i64 - cont_frame_base as i64;
                let stack_offset =
                    (vm.stack_top as i64 - cont_stack_len as i64 - 1) - cont_stack_base as i64;

                for mut h in cont_data.handler_stack {
                    h.unwind_frame_start = (h.unwind_frame_start as i64 + frame_offset) as usize;
                    h.unwind_stack_start = (h.unwind_stack_start as i64 + stack_offset) as usize;
                    h.capture_frame_start = (h.capture_frame_start as i64 + frame_offset) as usize;
                    h.capture_stack_start = (h.capture_stack_start as i64 + stack_offset) as usize;
                    vm.handler_stack.push(h);
                }

                // Restore handler dispatch markers, rebasing frame offsets
                for mut d in cont_data.handler_dispatch_stack {
                    d.dispatch_frame = (d.dispatch_frame as i64 + frame_offset) as usize;
                    vm.handler_dispatch_stack.push(d);
                }

                // CPS dispatch: the resumed continuation runs inline.
                // When the deepest continuation frame returns via Op::Return,
                // it falls through back to the caller of (resume k v).
                // Any re-performed effects are caught by the re-installed handler
                // (which is on the VM handler stack via the continuation's captured handlers).
                // No need for run() or run_restored_continuation() — just set up the
                // top continuation frame and continue the run loop.
                frame_idx = vm.frame_count - 1;
                ip = vm.frames[frame_idx].ip;
                cache_frame!();
            }

            Op::Halt => {
                let msg = vm.pop();
                if let Some(s) = msg.as_string() {
                    vm.frames[frame_idx].ip = ip;
                    return Err(VMSignal::Error(vm.vm_error(s, &vm.frames[frame_idx])));
                }
                vm.frames[frame_idx].ip = ip;
                return Err(VMSignal::Error(vm.vm_error("HALT", &vm.frames[frame_idx])));
            }
        }
    }
}

// --- Value equality ---

pub fn values_equal(a: &Value, b: &Value) -> bool {
    // Int == Int
    if let (Some(na), Some(nb)) = (a.as_int(), b.as_int()) {
        return na == nb;
    }

    // Float == Float
    if let (Some(na), Some(nb)) = (a.as_float(), b.as_float()) {
        return na == nb;
    }

    // Int != Float (different types are never equal)

    // Both nil
    if a.is_nil() && b.is_nil() {
        return true;
    }

    // Both booleans
    if let (Some(ba), Some(bb)) = (a.as_bool(), b.as_bool()) {
        return ba == bb;
    }

    // Object comparison
    if let (Some(oa), Some(ob)) = (a.as_object(), b.as_object()) {
        return objects_equal(oa, ob);
    }

    // nil == Nil ADT
    if a.is_nil()
        && let Some(obj) = b.as_object()
    {
        return matches!(obj, VMObject::AdtValue { tag, fields } if tag == "Nil" && fields.is_empty());
    }
    if b.is_nil()
        && let Some(obj) = a.as_object()
    {
        return matches!(obj, VMObject::AdtValue { tag, fields } if tag == "Nil" && fields.is_empty());
    }

    if let (Some(ba), Some(bb)) = (a.as_char(), b.as_char()) {
        return ba == bb;
    }

    false
}

fn objects_equal(a: &VMObject, b: &VMObject) -> bool {
    match (a, b) {
        (VMObject::String(sa), VMObject::String(sb)) => sa == sb,
        (
            VMObject::AdtValue {
                tag: t1,
                fields: f1,
            },
            VMObject::AdtValue {
                tag: t2,
                fields: f2,
            },
        ) => {
            if t1 != t2 || f1.len() != f2.len() {
                return false;
            }
            f1.iter().zip(f2.iter()).all(|(a, b)| values_equal(a, b))
        }
        (VMObject::Pid(a), VMObject::Pid(b)) => a == b,
        (VMObject::Port(a), VMObject::Port(b)) => a == b,
        (VMObject::ByteArray(a), VMObject::ByteArray(b)) => a == b,
        _ => false,
    }
}
