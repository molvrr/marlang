use std::collections::HashSet;
use std::sync::Arc;

use rustc_hash::FxHashMap;

use crate::bytecode::{Chunk, ConstantValue, FunctionPrototype, ModuleId, Op, ROOT_MODULE};
use crate::errors::{StackTraceEntry, VMError};
use crate::object::{
    CallFrame, Closure, HandlerDispatch, HandlerEntry, VMContinuation, VMHandlerFrame, VMObject,
};
use crate::value::{Value, format_value_unquoted};

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
        if module_id != ROOT_MODULE {
            if let Some(v) = self.module_globals[module_id as usize].get(name) {
                return Some(v);
            }
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

    fn push_frame(&mut self, frame: CallFrame) {
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

fn create_closure_from_proto(frame: &CallFrame, proto: &FunctionPrototype) -> Closure {
    let mut upvalues = Vec::with_capacity(proto.upvalues.len());
    for desc in &proto.upvalues {
        if desc.is_local {
            upvalues.push(frame.locals[desc.index as usize].clone());
        } else {
            upvalues.push(frame.closure.upvalues[desc.index as usize].clone());
        }
    }
    let module_id = proto.module_id;
    Closure {
        proto: Arc::new(proto.clone()),
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
/// With CPS dispatch, user-handler effects are dispatched inline by PERFORM,
/// so only scheduler effects escape as VMSignal::Effect.
#[allow(dead_code)]
pub(crate) fn resume_continuation(
    vm: &mut VM,
    cont: &VMContinuation,
    value: Value,
) -> Result<Value, VMSignal> {
    // Enforce one-shot semantics via VM-level tracking (works across Arc aliases)
    if !vm.resumed_continuations.insert(cont.id) {
        return Err(VMSignal::Error(VMError::runtime(
            "Continuation already resumed (one-shot)",
        )));
    }

    let resume_base = vm.frame_count;
    for f in &cont.frames {
        let frame = CallFrame {
            closure: f.closure.clone(),
            ip: f.ip,
            locals: f.locals.clone(),
        };
        vm.push_frame(frame);
    }

    for v in &cont.stack {
        vm.push(v.clone());
    }
    vm.push(value);

    let saved_handler_len = vm.handler_stack.len();
    let saved_dispatch_len = vm.handler_dispatch_stack.len();
    let frame_offset = resume_base as i64 - cont.frame_base as i64;
    let stack_offset = (vm.stack_top as i64 - cont.stack.len() as i64 - 1) - cont.stack_base as i64;
    for h in &cont.handler_stack {
        let rebased = rebase_handler(h, frame_offset, stack_offset);
        vm.handler_stack.push(rebased);
    }
    for d in &cont.handler_dispatch_stack {
        let rebased = rebase_handler_dispatch(d, frame_offset);
        vm.handler_dispatch_stack.push(rebased);
    }

    // Run — user-handler effects are dispatched inline by PERFORM.
    // Only scheduler effects escape as VMSignal::Effect.
    let result = run(vm, resume_base);

    vm.frame_count = resume_base;
    vm.handler_stack.truncate(saved_handler_len);
    vm.handler_dispatch_stack.truncate(saved_dispatch_len);

    result
}

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
    }
}

// --- Effect handler helpers ---

fn rebase_handler_dispatch(d: &HandlerDispatch, frame_offset: i64) -> HandlerDispatch {
    HandlerDispatch {
        dispatch_frame: (d.dispatch_frame as i64 + frame_offset) as usize,
        handler_entries: d.handler_entries.clone(),
        return_handler: d.return_handler.clone(),
        handler_id: d.handler_id,
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

    let locals = vec![Value::nil(); chunk.local_count as usize];
    let frame = CallFrame {
        closure: script_closure,
        ip: 0,
        locals,
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

struct HandlerDescriptor {
    body_proto_idx: u32,
    handlers: Vec<HandlerClauseDescriptor>,
    return_proto_idx: i32,
}

struct HandlerClauseDescriptor {
    op_name_idx: u32,
    proto_idx: u32,
    arity: u8,
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

    Ok(HandlerDescriptor {
        body_proto_idx,
        handlers,
        return_proto_idx,
    })
}

fn parse_handler_clause(json: &str) -> Result<HandlerClauseDescriptor, String> {
    let json = json.trim();
    if !json.starts_with('{') || !json.ends_with('}') {
        return Err("expected handler clause object".to_string());
    }
    let inner = &json[1..json.len() - 1];
    let op_name_idx = extract_number(inner, "opNameIdx")? as u32;
    let proto_idx = extract_number(inner, "protoIdx")? as u32;
    let arity = extract_number(inner, "arity")? as u8;
    Ok(HandlerClauseDescriptor {
        op_name_idx,
        proto_idx,
        arity,
    })
}

fn extract_number(s: &str, key: &str) -> Result<f64, String> {
    let pattern = format!("\"{}\":", key);
    let pos = s
        .find(&pattern)
        .ok_or_else(|| format!("missing key: {}", key))?;
    let after = &s[pos + pattern.len()..];
    let end = after
        .find(|c: char| c == ',' || c == '}' || c == ']')
        .unwrap_or(after.len());
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
    for i in (open + 1)..bytes.len() {
        if bytes[i] == open_char {
            depth += 1;
        }
        if bytes[i] == close_char {
            depth -= 1;
            if depth == 0 {
                return Ok(i);
            }
        }
    }
    Err("unmatched bracket".to_string())
}

// --- Default IO handlers ---

fn default_make_ok(value: Value) -> Value {
    Value::object(VMObject::AdtValue {
        tag: "Ok".to_string(),
        fields: vec![value],
    })
}

fn default_make_err(msg: &str) -> Value {
    Value::object(VMObject::AdtValue {
        tag: "Err".to_string(),
        fields: vec![Value::string(msg)],
    })
}

fn try_default_handler(vm: &mut VM, op_name: &str, args: &[Value]) -> Option<Value> {
    match op_name {
        "print" => {
            let msg = args.first().cloned().unwrap_or_else(Value::nil);
            let text = format_value_unquoted(&msg);
            if let Some(ref mut buf) = vm.captured_output {
                buf.push_str(&text);
            } else {
                print!("{}", text);
            }
            Some(Value::nil())
        }
        "println" => {
            let msg = args.first().cloned().unwrap_or_else(Value::nil);
            let text = format_value_unquoted(&msg);
            if let Some(ref mut buf) = vm.captured_output {
                buf.push_str(&text);
                buf.push('\n');
            } else {
                println!("{}", text);
            }
            Some(Value::nil())
        }
        "read-line" => {
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).ok();
            if line.ends_with('\n') {
                line.pop();
                if line.ends_with('\r') {
                    line.pop();
                }
            }
            Some(Value::string(line.as_str()))
        }
        "read-file" => {
            let path = args.first()?;
            let path_str = path.as_string()?;
            match std::fs::read_to_string(path_str) {
                Ok(content) => Some(default_make_ok(Value::string(content.as_str()))),
                Err(e) => Some(default_make_err(&format!("read-file: {}", e))),
            }
        }
        "write-file" => {
            let path = args.first()?.as_string()?;
            let content = args.get(1)?.as_string()?;
            match std::fs::write(path, content) {
                Ok(_) => Some(default_make_ok(Value::nil())),
                Err(e) => Some(default_make_err(&format!("write-file: {}", e))),
            }
        }
        _ => None,
    }
}

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
                // SAFETY: operand is a valid local index (from compiler)
                let val = unsafe { (*locals.add(inst.operand as usize)).clone() };
                vm.push(val);
            }

            Op::SetLocal => {
                let val = vm.pop();
                // SAFETY: operand is a valid local index, pointer is valid
                unsafe { *locals.add(inst.operand as usize) = val };
            }

            Op::PushUpvalue => {
                let val = vm.frames[frame_idx].closure.upvalues[inst.operand as usize].clone();
                vm.push(val);
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
                if let Some(s) = value.as_string() {
                    if s.starts_with("\0ctor:") {
                        let rest = &s[6..];
                        if let Some(colon_pos) = rest.rfind(':') {
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

            // --- Arithmetic ---
            Op::Add => {
                let b = vm.pop();
                let a = vm.pop();
                match (a.as_number(), b.as_number()) {
                    (Some(na), Some(nb)) => vm.push(Value::number(na + nb)),
                    _ => {
                        // String concatenation
                        match (a.as_string(), b.as_string()) {
                            (Some(sa), Some(sb)) => {
                                let result = format!("{}{}", sa, sb);
                                vm.push(Value::string(result.as_str()));
                            }
                            _ => {
                                vm.frames[frame_idx].ip = ip;
                                return Err(VMSignal::Error(vm.vm_error(
                                    format!("Cannot add {} and {}", a.kind_name(), b.kind_name()),
                                    &vm.frames[frame_idx],
                                )));
                            }
                        }
                    }
                }
            }

            Op::Sub => {
                let b = vm.pop();
                let a = vm.pop();
                match (a.as_number(), b.as_number()) {
                    (Some(na), Some(nb)) => vm.push(Value::number(na - nb)),
                    _ => {
                        vm.frames[frame_idx].ip = ip;
                        return Err(VMSignal::Error(
                            vm.vm_error("Subtraction requires numbers", &vm.frames[frame_idx]),
                        ));
                    }
                }
            }

            Op::Mul => {
                let b = vm.pop();
                let a = vm.pop();
                match (a.as_number(), b.as_number()) {
                    (Some(na), Some(nb)) => vm.push(Value::number(na * nb)),
                    _ => {
                        vm.frames[frame_idx].ip = ip;
                        return Err(VMSignal::Error(vm.vm_error(
                            "Multiplication requires numbers",
                            &vm.frames[frame_idx],
                        )));
                    }
                }
            }

            Op::Div => {
                let b = vm.pop();
                let a = vm.pop();
                match (a.as_number(), b.as_number()) {
                    (Some(na), Some(nb)) => vm.push(Value::number(na / nb)),
                    _ => {
                        vm.frames[frame_idx].ip = ip;
                        return Err(VMSignal::Error(
                            vm.vm_error("Division requires numbers", &vm.frames[frame_idx]),
                        ));
                    }
                }
            }

            Op::Neg => {
                let a = vm.pop();
                match a.as_number() {
                    Some(n) => vm.push(Value::number(-n)),
                    None => {
                        vm.frames[frame_idx].ip = ip;
                        return Err(VMSignal::Error(
                            vm.vm_error("Negate requires number", &vm.frames[frame_idx]),
                        ));
                    }
                }
            }

            // --- Comparison ---
            Op::Eq => {
                let b = vm.pop();
                let a = vm.pop();
                vm.push(Value::boolean(values_equal(&a, &b)));
            }

            Op::Lt => {
                let b = vm.pop();
                let a = vm.pop();
                match (a.as_number(), b.as_number()) {
                    (Some(na), Some(nb)) => vm.push(Value::boolean(na < nb)),
                    _ => {
                        vm.frames[frame_idx].ip = ip;
                        return Err(VMSignal::Error(
                            vm.vm_error("< requires numbers", &vm.frames[frame_idx]),
                        ));
                    }
                }
            }

            Op::Gt => {
                let b = vm.pop();
                let a = vm.pop();
                match (a.as_number(), b.as_number()) {
                    (Some(na), Some(nb)) => vm.push(Value::boolean(na > nb)),
                    _ => {
                        vm.frames[frame_idx].ip = ip;
                        return Err(VMSignal::Error(
                            vm.vm_error("> requires numbers", &vm.frames[frame_idx]),
                        ));
                    }
                }
            }

            Op::Le => {
                let b = vm.pop();
                let a = vm.pop();
                match (a.as_number(), b.as_number()) {
                    (Some(na), Some(nb)) => vm.push(Value::boolean(na <= nb)),
                    _ => {
                        vm.frames[frame_idx].ip = ip;
                        return Err(VMSignal::Error(
                            vm.vm_error("<= requires numbers", &vm.frames[frame_idx]),
                        ));
                    }
                }
            }

            Op::Ge => {
                let b = vm.pop();
                let a = vm.pop();
                match (a.as_number(), b.as_number()) {
                    (Some(na), Some(nb)) => vm.push(Value::boolean(na >= nb)),
                    _ => {
                        vm.frames[frame_idx].ip = ip;
                        return Err(VMSignal::Error(
                            vm.vm_error(">= requires numbers", &vm.frames[frame_idx]),
                        ));
                    }
                }
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
                if let Some(dispatch) = vm.handler_dispatch_stack.last() {
                    if vm.frame_count <= dispatch.dispatch_frame {
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
                                for i in 0..required_arity {
                                    locals[i] = all_args[i].clone();
                                }
                                // Excess args packed into a List in locals[required_arity]
                                let rest_args: Vec<Value> = all_args[required_arity..].to_vec();
                                locals[required_arity] = Value::object(VMObject::List(rest_args));
                            } else {
                                for i in (0..arity).rev() {
                                    locals[i] = vm.pop();
                                }
                                vm.pop(); // pop callee
                            }

                            let new_frame = CallFrame {
                                closure: cloned_closure,
                                ip: 0,
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
                            let mut args = vec![Value::nil(); arity];
                            for i in (0..arity).rev() {
                                args[i] = vm.pop();
                            }
                            vm.pop(); // pop callee

                            vm.frames[frame_idx].ip = ip;
                            let result = func(vm, &args).map_err(VMSignal::Error)?;
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
                                .map_err(VMSignal::Error)?;
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
                                for i in 0..required_arity {
                                    locals[i] = all_args[i].clone();
                                }
                                let rest_args: Vec<Value> = all_args[required_arity..].to_vec();
                                locals[required_arity] = Value::object(VMObject::List(rest_args));
                            } else {
                                for i in (0..arity).rev() {
                                    locals[i] = vm.pop();
                                }
                                vm.pop(); // pop callee
                            }

                            // Reuse current frame
                            vm.frames[frame_idx].closure = (**closure).clone();
                            ip = 0;
                            vm.frames[frame_idx].locals = locals;
                            cache_frame!();
                        }

                        VMObject::Builtin { func, .. } => {
                            let func = *func;
                            let mut args = vec![Value::nil(); arity];
                            for i in (0..arity).rev() {
                                args[i] = vm.pop();
                            }
                            vm.pop(); // pop callee

                            vm.frames[frame_idx].ip = ip;
                            let result = func(vm, &args).map_err(VMSignal::Error)?;

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
                                .map_err(VMSignal::Error)?;

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

                let closure = create_closure_from_proto(&vm.frames[frame_idx], proto);
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
                        VMObject::List(elements) => {
                            if field_idx == 0 {
                                if elements.is_empty() {
                                    vm.frames[frame_idx].ip = ip;
                                    return Err(VMSignal::Error(vm.vm_error(
                                        "Cannot get head of empty list",
                                        &vm.frames[frame_idx],
                                    )));
                                }
                                vm.push(elements[0].clone());
                            } else if field_idx == 1 {
                                vm.push(Value::object(VMObject::List(elements[1..].to_vec())));
                            } else {
                                vm.frames[frame_idx].ip = ip;
                                return Err(VMSignal::Error(vm.vm_error(
                                    format!("Invalid field index {} for list", field_idx),
                                    &vm.frames[frame_idx],
                                )));
                            }
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

                if value.is_number() {
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
                        VMObject::List(elements) => {
                            if tag_name == "Cons" && !elements.is_empty() {
                                matches = true;
                            }
                            if tag_name == "Nil" && elements.is_empty() {
                                matches = true;
                            }
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

                let json = &descriptor_str["\0handler:".len()..];
                let descriptor = match parse_handler_descriptor(json) {
                    Ok(d) => d,
                    Err(e) => {
                        vm.frames[frame_idx].ip = ip;
                        return Err(VMSignal::Error(vm.vm_error(
                            format!("Failed to parse handler descriptor: {}", e),
                            &vm.frames[frame_idx],
                        )));
                    }
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
                let body_closure = create_closure_from_proto(&vm.frames[frame_idx], body_proto);

                // Create handler clause closures
                let mut handler_entries = Vec::new();
                for h in &descriptor.handlers {
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
                    handler_entries.push(HandlerEntry {
                        op_name,
                        closure: create_closure_from_proto(&vm.frames[frame_idx], proto),
                        arity: h.arity,
                    });
                }

                // Create return handler closure
                let return_closure = if descriptor.return_proto_idx >= 0 {
                    match unsafe { &*constants.add(descriptor.return_proto_idx as usize) } {
                        ConstantValue::Prototype(p) => {
                            Some(create_closure_from_proto(&vm.frames[frame_idx], p))
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
                let handler_frame = VMHandlerFrame {
                    handlers: handler_entries.clone(),
                    return_handler: return_closure.clone(),
                    unwind_frame_start: vm.frame_count,
                    unwind_stack_start: vm.stack_top,
                    capture_frame_start: vm.frame_count,
                    capture_stack_start: vm.stack_top,
                    id: handler_id,
                    is_scheduler: false,
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
                let op_name = match unsafe { &*constants.add(inst.operand as usize) } {
                    ConstantValue::String(s) => s.clone(),
                    _ => {
                        vm.frames[frame_idx].ip = ip;
                        return Err(VMSignal::Error(
                            vm.vm_error("PERFORM name must be string", &vm.frames[frame_idx]),
                        ));
                    }
                };
                let arity = inst.operand2 as usize;

                let mut args = vec![Value::nil(); arity];
                for i in (0..arity).rev() {
                    args[i] = vm.pop();
                }

                // Walk handler stack to find a matching handler
                let mut found = None;
                for i in (0..vm.handler_stack.len()).rev() {
                    if vm.handler_stack[i]
                        .handlers
                        .iter()
                        .any(|e| e.op_name == op_name)
                    {
                        found = Some(i);
                        break;
                    }
                }

                if let Some(handler_idx) = found {
                    let is_scheduler = vm.handler_stack[handler_idx].is_scheduler;
                    let handler_id = vm.handler_stack[handler_idx].id;
                    let capture_frame_start = vm.handler_stack[handler_idx].capture_frame_start;
                    let capture_stack_start = vm.handler_stack[handler_idx].capture_stack_start;
                    let unwind_fc = vm.handler_stack[handler_idx].unwind_frame_start;
                    let unwind_st = vm.handler_stack[handler_idx].unwind_stack_start;

                    // Extract handler data BEFORE unwinding (needed for user handler dispatch)
                    let handler_entries = vm.handler_stack[handler_idx].handlers.clone();
                    let return_handler = vm.handler_stack[handler_idx].return_handler.clone();

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
                        });
                    }
                    let captured_stack: Vec<Value> = vm.stack[capture_stack_start..vm.stack_top]
                        .iter()
                        .map(|v| v.clone())
                        .collect();

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

                    if is_scheduler {
                        // Scheduler handler: propagate via EffectSignal (existing path)
                        return Err(VMSignal::Effect(EffectSignal {
                            op_name,
                            args,
                            continuation: cont_value,
                            target_handler_id: handler_id,
                        }));
                    }

                    // ── User handler: CPS inline dispatch ──
                    // Find the matching handler clause
                    let clause = handler_entries
                        .iter()
                        .find(|e| e.op_name == op_name)
                        .cloned();
                    let clause = match clause {
                        Some(c) => c,
                        None => {
                            // Should not happen — we found the handler by name
                            return Err(VMSignal::Error(VMError::runtime(format!(
                                "Handler clause not found for effect: {}",
                                op_name
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
                    for i in 0..expected_arg_count.min(args.len()) {
                        clause_locals[i] = args[i].clone();
                    }
                    clause_locals[expected_arg_count] = cont_value;

                    // Call handler clause inline
                    let new_frame = CallFrame {
                        closure: clause.closure,
                        ip: 0,
                        locals: clause_locals,
                    };
                    vm.push_frame(new_frame);
                    frame_idx = vm.frame_count - 1;
                    ip = 0;
                    cache_frame!();
                    continue;
                }

                // No user handler found — try default handlers
                if let Some(result) = try_default_handler(vm, &op_name, &args) {
                    vm.push(result);
                } else if op_name == "throw" {
                    // Better error message for unhandled throw
                    let msg = args
                        .first()
                        .and_then(|v| v.as_string().map(|s| s.to_string()))
                        .unwrap_or_else(|| "(no message)".to_string());
                    vm.frames[frame_idx].ip = ip;
                    return Err(VMSignal::Error(vm.vm_error(
                        format!("Uncaught exception: {}", msg),
                        &vm.frames[frame_idx],
                    )));
                } else {
                    vm.frames[frame_idx].ip = ip;
                    return Err(VMSignal::Error(vm.vm_error(
                        format!("Unhandled effect: {}", op_name),
                        &vm.frames[frame_idx],
                    )));
                }
            }

            Op::Resume => {
                let value = vm.pop();
                let cont_val = vm.pop();

                let cont = match cont_val.as_object() {
                    Some(VMObject::Continuation(c)) => c,
                    _ => {
                        vm.frames[frame_idx].ip = ip;
                        return Err(VMSignal::Error(
                            vm.vm_error("resume expects a continuation", &vm.frames[frame_idx]),
                        ));
                    }
                };

                // Enforce one-shot semantics via VM-level tracking (works across Arc aliases)
                if !vm.resumed_continuations.insert(cont.id) {
                    vm.frames[frame_idx].ip = ip;
                    return Err(VMSignal::Error(vm.vm_error(
                        "Continuation already resumed (one-shot)",
                        &vm.frames[frame_idx],
                    )));
                }

                let cont_data = (**cont).clone();

                // Update the current (top) handler frame's capture/unwind points
                // so future PERFORMs capture from the right place.
                vm.frames[frame_idx].ip = ip;
                if let Some(top_handler) = vm.handler_stack.last_mut() {
                    top_handler.capture_frame_start = vm.frame_count;
                    top_handler.capture_stack_start = vm.stack_top;
                    top_handler.unwind_frame_start = vm.frame_count;
                    top_handler.unwind_stack_start = vm.stack_top;
                }

                // Restore continuation frames on top of current frames
                let resume_base = vm.frame_count;
                for f in &cont_data.frames {
                    vm.push_frame(CallFrame {
                        closure: f.closure.clone(),
                        ip: f.ip,
                        locals: f.locals.clone(),
                    });
                }

                // Restore continuation stack + push resume value
                for v in &cont_data.stack {
                    vm.push(v.clone());
                }
                vm.push(value);

                // Restore nested handlers, rebasing indices
                let frame_offset = resume_base as i64 - cont_data.frame_base as i64;
                let stack_offset = (vm.stack_top as i64 - cont_data.stack.len() as i64 - 1)
                    - cont_data.stack_base as i64;

                for h in &cont_data.handler_stack {
                    let rebased = rebase_handler(h, frame_offset, stack_offset);
                    vm.handler_stack.push(rebased);
                }

                // Restore handler dispatch markers, rebasing frame offsets
                for d in &cont_data.handler_dispatch_stack {
                    let rebased = rebase_handler_dispatch(d, frame_offset);
                    vm.handler_dispatch_stack.push(rebased);
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
    // Fast path: both numbers
    if let (Some(na), Some(nb)) = (a.as_number(), b.as_number()) {
        return na == nb;
    }

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

    // nil == empty list, nil == Nil ADT
    if a.is_nil() {
        if let Some(obj) = b.as_object() {
            return matches!(obj,
                VMObject::List(elements) if elements.is_empty())
                || matches!(obj, VMObject::AdtValue { tag, fields } if tag == "Nil" && fields.is_empty());
        }
    }
    if b.is_nil() {
        if let Some(obj) = a.as_object() {
            return matches!(obj,
                VMObject::List(elements) if elements.is_empty())
                || matches!(obj, VMObject::AdtValue { tag, fields } if tag == "Nil" && fields.is_empty());
        }
    }

    false
}

fn objects_equal(a: &VMObject, b: &VMObject) -> bool {
    match (a, b) {
        (VMObject::String(sa), VMObject::String(sb)) => sa == sb,
        (VMObject::List(la), VMObject::List(lb)) => {
            if la.len() != lb.len() {
                return false;
            }
            la.iter().zip(lb.iter()).all(|(a, b)| values_equal(a, b))
        }
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
        // nil-equivalent comparisons
        (VMObject::List(elements), VMObject::AdtValue { tag, fields })
        | (VMObject::AdtValue { tag, fields }, VMObject::List(elements)) => {
            elements.is_empty() && tag == "Nil" && fields.is_empty()
        }
        (VMObject::Pid(a), VMObject::Pid(b)) => a == b,
        (VMObject::Port(a), VMObject::Port(b)) => a == b,
        _ => false,
    }
}
