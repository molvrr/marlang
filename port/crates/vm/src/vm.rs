use std::collections::{HashMap, HashSet};

use crate::bytecode::{Chunk, ConstantValue, FunctionPrototype, Op};
use crate::errors::{StackTraceEntry, VMError};
use crate::object::{
    CallFrame, Closure, HandlerEntry, VMContinuation, VMHandlerFrame, VMObject,
};
use crate::value::{format_value_unquoted, Value};

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
    pub target_handler_id: u64,
}

// --- VM ---

static HANDLER_ID_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
static CONTINUATION_ID_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_handler_id() -> u64 {
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
    pub globals: HashMap<String, Value>,
    pub handler_stack: Vec<VMHandlerFrame>,
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
            globals: HashMap::new(),
            handler_stack: Vec::new(),
            resumed_continuations: HashSet::new(),
            captured_output: None,
            dirty_globals: None,
        }
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
    fn clear_stack_slots(&mut self, new_top: usize) {
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

impl Default for VM {
    fn default() -> Self {
        Self::new()
    }
}

/// Pre-convert constants, recursively handling prototypes by converting their
/// nested chunks' constants as well. Prototypes become closures (with no upvalues).
fn preconvert_constants_recursive(constants: &[ConstantValue]) -> Vec<Value> {
    constants
        .iter()
        .map(|c| match c {
            ConstantValue::Number(n) => Value::number(*n),
            ConstantValue::String(s) => Value::string(s.as_str()),
            ConstantValue::Boolean(b) => Value::boolean(*b),
            ConstantValue::Nil => Value::nil(),
            ConstantValue::Prototype(_) => Value::nil(), // prototypes accessed via compile-time constants
        })
        .collect()
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
    Closure {
        proto: proto.clone(),
        upvalues,
    }
}

/// Call a closure, pushing a frame and running until it returns.
pub(crate) fn call_closure(vm: &mut VM, closure: &Closure, args: &[Value]) -> Result<Value, VMSignal> {
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

/// Resume a continuation from Rust (used by scheduler in future).
/// Unlike the RESUME opcode, this does NOT catch EffectSignal —
/// effects propagate to the caller.
#[allow(dead_code)] // Used by future scheduler (Phase 5)
pub(crate) fn resume_continuation(vm: &mut VM, cont: &VMContinuation, value: Value) -> Result<Value, VMSignal> {
    // Enforce one-shot semantics via VM-level tracking (works across Arc aliases)
    if !vm.resumed_continuations.insert(cont.id) {
        return Err(VMSignal::Error(VMError::runtime("Continuation already resumed (one-shot)")));
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
    let frame_offset = resume_base as i64 - cont.frame_base as i64;
    let stack_offset = (vm.stack_top as i64 - cont.stack.len() as i64 - 1) - cont.stack_base as i64;
    for h in &cont.handler_stack {
        vm.handler_stack.push(rebase_handler(h, frame_offset, stack_offset));
    }

    let result = run(vm, resume_base);

    vm.frame_count = resume_base;
    vm.handler_stack.truncate(saved_handler_len);

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
    }
}

// --- Effect handler execution ---

fn run_with_handler(
    vm: &mut VM,
    handler_entries: &[HandlerEntry],
    return_handler: &Option<Closure>,
    closure: &Closure,
    args: &[Value],
) -> Result<Value, VMSignal> {
    let handler_id = next_handler_id();
    let handler_frame = VMHandlerFrame {
        handlers: handler_entries.to_vec(),
        return_handler: return_handler.clone(),
        unwind_frame_start: vm.frame_count,
        unwind_stack_start: vm.stack_top,
        capture_frame_start: vm.frame_count,
        capture_stack_start: vm.stack_top,
        id: handler_id,
    };
    vm.handler_stack.push(handler_frame);

    match call_closure(vm, closure, args) {
        Ok(result) => {
            // Remove our handler if still present
            if let Some(pos) = vm.handler_stack.iter().position(|h| h.id == handler_id) {
                vm.handler_stack.remove(pos);
            }
            Ok(result)
        }
        Err(VMSignal::Effect(signal)) if signal.target_handler_id == handler_id => {
            // Handler was already removed from stack by PERFORM's unwind.
            let clause = handler_entries.iter().find(|e| e.op_name == signal.op_name);
            let clause = match clause {
                Some(c) => c,
                None => return Err(VMSignal::Effect(signal)),
            };
            let expected_arg_count = (clause.arity as usize).saturating_sub(1);
            let mut handler_args: Vec<Value> = Vec::new();
            for i in 0..expected_arg_count.min(signal.args.len()) {
                handler_args.push(signal.args[i].clone());
            }
            handler_args.push(signal.continuation);
            // Deep handler semantics: recursively run with same handler def
            run_with_handler(vm, handler_entries, return_handler, &clause.closure, &handler_args)
        }
        Err(e) => {
            // Not for us — clean up if our handler is still on the stack
            if let Some(pos) = vm.handler_stack.iter().position(|h| h.id == handler_id) {
                vm.handler_stack.remove(pos);
            }
            Err(e)
        }
    }
}

/// Run a restored continuation, catching effects that target restored handlers.
fn run_restored_continuation(
    vm: &mut VM,
    resume_base: usize,
    restored_handlers: &[VMHandlerFrame],
    saved_handler_len: usize,
) -> Result<Value, VMSignal> {
    match run(vm, resume_base) {
        Ok(result) => Ok(result),
        Err(VMSignal::Effect(signal)) => {
            for hframe in restored_handlers {
                if signal.target_handler_id == hframe.id {
                    let clause = hframe.handlers.iter().find(|e| e.op_name == signal.op_name);
                    let clause = match clause {
                        Some(c) => c,
                        None => return Err(VMSignal::Effect(signal)),
                    };
                    let expected_arg_count = (clause.arity as usize).saturating_sub(1);
                    let mut handler_args = Vec::new();
                    for i in 0..expected_arg_count.min(signal.args.len()) {
                        handler_args.push(signal.args[i].clone());
                    }
                    handler_args.push(signal.continuation);

                    vm.handler_stack.truncate(saved_handler_len);
                    return run_with_handler(
                        vm,
                        &hframe.handlers,
                        &hframe.return_handler,
                        &clause.closure,
                        &handler_args,
                    );
                }
            }
            Err(VMSignal::Effect(signal))
        }
        Err(e) => Err(e),
    }
}

// --- Interpret ---

pub fn interpret(vm: &mut VM, chunk: &Chunk) -> Result<Value, VMError> {
    let script_proto = FunctionPrototype {
        name: "<script>".to_string(),
        arity: 0,
        upvalues: Vec::new(),
        chunk: chunk.clone(),
    };

    let script_closure = Closure {
        proto: script_proto,
        upvalues: Vec::new(),
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
    let inner = &json[1..json.len()-1];

    let body_proto_idx = extract_number(inner, "bodyProtoIdx")? as u32;
    let return_proto_idx = extract_number(inner, "returnProtoIdx")? as i32;

    // Extract handlers array
    let handlers_start = inner.find("\"handlers\":[").ok_or("missing handlers")?;
    let arr_start = inner[handlers_start..].find('[').unwrap() + handlers_start;
    let arr_end = find_matching_bracket(inner, arr_start)?;
    let arr_content = &inner[arr_start+1..arr_end];

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
                        while start < bytes.len() && (bytes[start] == b',' || bytes[start] == b' ') {
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
    let inner = &json[1..json.len()-1];
    let op_name_idx = extract_number(inner, "opNameIdx")? as u32;
    let proto_idx = extract_number(inner, "protoIdx")? as u32;
    let arity = extract_number(inner, "arity")? as u8;
    Ok(HandlerClauseDescriptor { op_name_idx, proto_idx, arity })
}

fn extract_number(s: &str, key: &str) -> Result<f64, String> {
    let pattern = format!("\"{}\":", key);
    let pos = s.find(&pattern).ok_or_else(|| format!("missing key: {}", key))?;
    let after = &s[pos + pattern.len()..];
    let end = after.find(|c: char| c == ',' || c == '}' || c == ']').unwrap_or(after.len());
    let num_str = after[..end].trim();
    num_str.parse::<f64>().map_err(|e| format!("bad number for {}: {}", key, e))
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
    for i in (open+1)..bytes.len() {
        if bytes[i] == open_char { depth += 1; }
        if bytes[i] == close_char { depth -= 1; if depth == 0 { return Ok(i); } }
    }
    Err("unmatched bracket".to_string())
}

// --- Default IO handlers ---

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
                Ok(content) => Some(Value::string(content.as_str())),
                Err(_) => None,
            }
        }
        "write-file" => {
            let path = args.first()?.as_string()?;
            let content = args.get(1)?.as_string()?;
            std::fs::write(path, content).ok()?;
            Some(Value::nil())
        }
        _ => None,
    }
}

// --- Main Execution Loop ---

pub(crate) fn run(vm: &mut VM, base_frame_count: usize) -> Result<Value, VMSignal> {
    // Cache current frame data
    let mut frame_idx = vm.frame_count - 1;
    let mut frame = vm.frames[frame_idx].clone();

    // Pre-convert constants for the initial frame's chunk
    let mut vm_constants = preconvert_constants_recursive(&frame.closure.proto.chunk.constants);

    loop {
        let inst = &frame.closure.proto.chunk.code[frame.ip];
        frame.ip += 1;

        match inst.op {
            // --- Stack operations ---
            Op::PushConst => {
                let val = vm_constants[inst.operand as usize].clone();
                vm.push(val);
            }

            Op::PushLocal => {
                let val = frame.locals[inst.operand as usize].clone();
                vm.push(val);
            }

            Op::SetLocal => {
                let val = vm.pop();
                frame.locals[inst.operand as usize] = val;
            }

            Op::PushUpvalue => {
                let val = frame.closure.upvalues[inst.operand as usize].clone();
                vm.push(val);
            }

            Op::PushGlobal => {
                let name = match &frame.closure.proto.chunk.constants[inst.operand as usize] {
                    ConstantValue::String(s) => s,
                    _ => {
                        save_frame(vm, frame_idx, &frame);
                        return Err(VMSignal::Error(vm.vm_error("Global name must be string", &frame)));
                    }
                };
                let value = match vm.globals.get(name) {
                    Some(v) => v.clone(),
                    None => {
                        save_frame(vm, frame_idx, &frame);
                        return Err(VMSignal::Error(vm.vm_error(
                            format!("Undefined variable: {}", name),
                            &frame,
                        )));
                    }
                };
                vm.push(value);
            }

            Op::SetGlobal => {
                let name = match &frame.closure.proto.chunk.constants[inst.operand as usize] {
                    ConstantValue::String(s) => s.clone(),
                    _ => {
                        save_frame(vm, frame_idx, &frame);
                        return Err(VMSignal::Error(vm.vm_error("Global name must be string", &frame)));
                    }
                };
                let value = vm.pop();

                // Track dirty globals for module export detection
                if let Some(ref mut dirty) = vm.dirty_globals {
                    dirty.insert(name.clone());
                }

                // Check for N-ary constructor encoding: "\0ctor:Name:arity"
                if let Some(s) = value.as_string() {
                    if s.starts_with("\0ctor:") {
                        let rest = &s[6..];
                        if let Some(colon_pos) = rest.rfind(':') {
                            let ctor_name = &rest[..colon_pos];
                            if let Ok(arity) = rest[colon_pos+1..].parse::<u8>() {
                                vm.globals.insert(
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
                vm.globals.insert(name, value);
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
                                save_frame(vm, frame_idx, &frame);
                                return Err(VMSignal::Error(vm.vm_error(
                                    format!("Cannot add {} and {}", a.kind_name(), b.kind_name()),
                                    &frame,
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
                        save_frame(vm, frame_idx, &frame);
                        return Err(VMSignal::Error(vm.vm_error("Subtraction requires numbers", &frame)));
                    }
                }
            }

            Op::Mul => {
                let b = vm.pop();
                let a = vm.pop();
                match (a.as_number(), b.as_number()) {
                    (Some(na), Some(nb)) => vm.push(Value::number(na * nb)),
                    _ => {
                        save_frame(vm, frame_idx, &frame);
                        return Err(VMSignal::Error(vm.vm_error("Multiplication requires numbers", &frame)));
                    }
                }
            }

            Op::Div => {
                let b = vm.pop();
                let a = vm.pop();
                match (a.as_number(), b.as_number()) {
                    (Some(na), Some(nb)) => vm.push(Value::number(na / nb)),
                    _ => {
                        save_frame(vm, frame_idx, &frame);
                        return Err(VMSignal::Error(vm.vm_error("Division requires numbers", &frame)));
                    }
                }
            }

            Op::Neg => {
                let a = vm.pop();
                match a.as_number() {
                    Some(n) => vm.push(Value::number(-n)),
                    None => {
                        save_frame(vm, frame_idx, &frame);
                        return Err(VMSignal::Error(vm.vm_error("Negate requires number", &frame)));
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
                        save_frame(vm, frame_idx, &frame);
                        return Err(VMSignal::Error(vm.vm_error("< requires numbers", &frame)));
                    }
                }
            }

            Op::Gt => {
                let b = vm.pop();
                let a = vm.pop();
                match (a.as_number(), b.as_number()) {
                    (Some(na), Some(nb)) => vm.push(Value::boolean(na > nb)),
                    _ => {
                        save_frame(vm, frame_idx, &frame);
                        return Err(VMSignal::Error(vm.vm_error("> requires numbers", &frame)));
                    }
                }
            }

            Op::Le => {
                let b = vm.pop();
                let a = vm.pop();
                match (a.as_number(), b.as_number()) {
                    (Some(na), Some(nb)) => vm.push(Value::boolean(na <= nb)),
                    _ => {
                        save_frame(vm, frame_idx, &frame);
                        return Err(VMSignal::Error(vm.vm_error("<= requires numbers", &frame)));
                    }
                }
            }

            Op::Ge => {
                let b = vm.pop();
                let a = vm.pop();
                match (a.as_number(), b.as_number()) {
                    (Some(na), Some(nb)) => vm.push(Value::boolean(na >= nb)),
                    _ => {
                        save_frame(vm, frame_idx, &frame);
                        return Err(VMSignal::Error(vm.vm_error(">= requires numbers", &frame)));
                    }
                }
            }

            Op::Not => {
                let a = vm.pop();
                vm.push(Value::boolean(!a.is_truthy()));
            }

            // --- Control flow ---
            Op::Jump => {
                frame.ip = inst.operand as usize;
            }

            Op::JumpIfFalse => {
                let cond = vm.pop();
                if !cond.is_truthy() {
                    frame.ip = inst.operand as usize;
                }
            }

            Op::Return => {
                let result = vm.pop();
                vm.frame_count -= 1;

                if vm.frame_count <= base_frame_count {
                    // Save frame back before returning (for stack trace accuracy)
                    return Ok(result);
                }

                frame_idx = vm.frame_count - 1;
                frame = vm.frames[frame_idx].clone();
                vm_constants = preconvert_constants_recursive(&frame.closure.proto.chunk.constants);
                vm.push(result);
            }

            // --- Functions ---
            Op::Call => {
                let arity = inst.operand as usize;
                let callee = vm.peek(arity).clone();

                if let Some(obj) = callee.as_object() {
                    match obj {
                        VMObject::Closure(closure) => {
                            if arity != closure.proto.arity as usize {
                                save_frame(vm, frame_idx, &frame);
                                return Err(VMSignal::Error(vm.vm_error(
                                    format!("Expected {} arguments, got {}", closure.proto.arity, arity),
                                    &frame,
                                )));
                            }

                            let mut locals = vec![Value::nil(); closure.proto.chunk.local_count as usize];
                            for i in (0..arity).rev() {
                                locals[i] = vm.pop();
                            }
                            vm.pop(); // pop callee

                            let new_frame = CallFrame {
                                closure: (**closure).clone(),
                                ip: 0,
                                locals,
                            };

                            // Save current frame back
                            save_frame(vm, frame_idx, &frame);

                            vm.push_frame(new_frame);
                            frame_idx = vm.frame_count - 1;
                            frame = vm.frames[frame_idx].clone();
                            vm_constants = preconvert_constants_recursive(&frame.closure.proto.chunk.constants);
                        }

                        VMObject::Builtin { func, .. } => {
                            let func = *func;
                            let mut args = vec![Value::nil(); arity];
                            for i in (0..arity).rev() {
                                args[i] = vm.pop();
                            }
                            vm.pop(); // pop callee

                            save_frame(vm, frame_idx, &frame);
                            let result = func(vm, &args).map_err(VMSignal::Error)?;
                            vm.push(result);
                        }

                        VMObject::AdtConstructor { name, arity: expected_arity } => {
                            let name = name.clone();
                            let expected_arity = *expected_arity;
                            if arity != expected_arity as usize {
                                save_frame(vm, frame_idx, &frame);
                                return Err(VMSignal::Error(vm.vm_error(
                                    format!("Constructor {} expects {} arguments, got {}", name, expected_arity, arity),
                                    &frame,
                                )));
                            }
                            let mut args = vec![Value::nil(); arity];
                            for i in (0..arity).rev() {
                                args[i] = vm.pop();
                            }
                            vm.pop(); // pop callee
                            vm.push(Value::object(VMObject::AdtValue { tag: name, fields: args }));
                        }

                        _ => {
                            let kind = obj.kind_name();
                            save_frame(vm, frame_idx, &frame);
                            return Err(VMSignal::Error(vm.vm_error(
                                format!("Cannot call {}", kind),
                                &frame,
                            )));
                        }
                    }
                } else {
                    save_frame(vm, frame_idx, &frame);
                    return Err(VMSignal::Error(vm.vm_error("Cannot call number", &frame)));
                }
            }

            Op::TailCall => {
                let arity = inst.operand as usize;
                let callee = vm.peek(arity).clone();

                if let Some(obj) = callee.as_object() {
                    match obj {
                        VMObject::Closure(closure) => {
                            if arity != closure.proto.arity as usize {
                                save_frame(vm, frame_idx, &frame);
                                return Err(VMSignal::Error(vm.vm_error(
                                    format!("Expected {} arguments, got {}", closure.proto.arity, arity),
                                    &frame,
                                )));
                            }

                            let mut locals = vec![Value::nil(); closure.proto.chunk.local_count as usize];
                            for i in (0..arity).rev() {
                                locals[i] = vm.pop();
                            }
                            vm.pop(); // pop callee

                            // Reuse current frame
                            frame.closure = (**closure).clone();
                            frame.ip = 0;
                            frame.locals = locals;
                            vm_constants = preconvert_constants_recursive(&frame.closure.proto.chunk.constants);
                        }

                        VMObject::Builtin { func, .. } => {
                            let func = *func;
                            let mut args = vec![Value::nil(); arity];
                            for i in (0..arity).rev() {
                                args[i] = vm.pop();
                            }
                            vm.pop(); // pop callee

                            save_frame(vm, frame_idx, &frame);
                            let result = func(vm, &args).map_err(VMSignal::Error)?;

                            vm.frame_count -= 1;
                            if vm.frame_count <= base_frame_count {
                                return Ok(result);
                            }
                            frame_idx = vm.frame_count - 1;
                            frame = vm.frames[frame_idx].clone();
                            vm_constants = preconvert_constants_recursive(&frame.closure.proto.chunk.constants);
                            vm.push(result);
                        }

                        VMObject::AdtConstructor { name, arity: expected_arity } => {
                            let name = name.clone();
                            let expected_arity = *expected_arity;
                            if arity != expected_arity as usize {
                                save_frame(vm, frame_idx, &frame);
                                return Err(VMSignal::Error(vm.vm_error(
                                    format!("Constructor {} expects {} arguments, got {}", name, expected_arity, arity),
                                    &frame,
                                )));
                            }
                            let mut args = vec![Value::nil(); arity];
                            for i in (0..arity).rev() {
                                args[i] = vm.pop();
                            }
                            vm.pop();
                            let result = Value::object(VMObject::AdtValue { tag: name, fields: args });

                            vm.frame_count -= 1;
                            if vm.frame_count <= base_frame_count {
                                return Ok(result);
                            }
                            frame_idx = vm.frame_count - 1;
                            frame = vm.frames[frame_idx].clone();
                            vm_constants = preconvert_constants_recursive(&frame.closure.proto.chunk.constants);
                            vm.push(result);
                        }

                        _ => {
                            let kind = obj.kind_name();
                            save_frame(vm, frame_idx, &frame);
                            return Err(VMSignal::Error(vm.vm_error(
                                format!("Cannot call {}", kind),
                                &frame,
                            )));
                        }
                    }
                } else {
                    save_frame(vm, frame_idx, &frame);
                    return Err(VMSignal::Error(vm.vm_error("Cannot call number", &frame)));
                }
            }

            Op::Closure => {
                let proto = match &frame.closure.proto.chunk.constants[inst.operand as usize] {
                    ConstantValue::Prototype(p) => p,
                    _ => {
                        save_frame(vm, frame_idx, &frame);
                        return Err(VMSignal::Error(vm.vm_error("CLOSURE expects a prototype constant", &frame)));
                    }
                };

                let closure = create_closure_from_proto(&frame, proto);
                vm.push(Value::object(VMObject::Closure(Box::new(closure))));
            }

            // --- ADTs ---
            Op::Construct => {
                let name = match &frame.closure.proto.chunk.constants[inst.operand as usize] {
                    ConstantValue::String(s) => s.clone(),
                    _ => {
                        save_frame(vm, frame_idx, &frame);
                        return Err(VMSignal::Error(vm.vm_error("CONSTRUCT name must be string", &frame)));
                    }
                };
                let arity = inst.operand2 as usize;
                let mut args = vec![Value::nil(); arity];
                for i in (0..arity).rev() {
                    args[i] = vm.pop();
                }
                vm.push(Value::object(VMObject::AdtValue { tag: name, fields: args }));
            }

            Op::GetField => {
                let field_idx = inst.operand as usize;
                let value = vm.peek(0).clone();

                if let Some(obj) = value.as_object() {
                    match obj {
                        VMObject::AdtValue { fields, tag, .. } => {
                            if field_idx >= fields.len() {
                                save_frame(vm, frame_idx, &frame);
                                return Err(VMSignal::Error(vm.vm_error(
                                    format!("Field index {} out of bounds for {}", field_idx, tag),
                                    &frame,
                                )));
                            }
                            let field = fields[field_idx].clone();
                            vm.push(field);
                        }
                        VMObject::List(elements) => {
                            if field_idx == 0 {
                                if elements.is_empty() {
                                    save_frame(vm, frame_idx, &frame);
                                    return Err(VMSignal::Error(vm.vm_error(
                                        "Cannot get head of empty list",
                                        &frame,
                                    )));
                                }
                                vm.push(elements[0].clone());
                            } else if field_idx == 1 {
                                vm.push(Value::object(VMObject::List(elements[1..].to_vec())));
                            } else {
                                save_frame(vm, frame_idx, &frame);
                                return Err(VMSignal::Error(vm.vm_error(
                                    format!("Invalid field index {} for list", field_idx),
                                    &frame,
                                )));
                            }
                        }
                        _ => {
                            save_frame(vm, frame_idx, &frame);
                            return Err(VMSignal::Error(vm.vm_error(
                                format!("GET_FIELD on non-ADT value: {}", obj.kind_name()),
                                &frame,
                            )));
                        }
                    }
                } else {
                    save_frame(vm, frame_idx, &frame);
                    return Err(VMSignal::Error(vm.vm_error("GET_FIELD on number", &frame)));
                }
            }

            Op::MatchTag => {
                let tag_name = match &frame.closure.proto.chunk.constants[inst.operand as usize] {
                    ConstantValue::String(s) => s.as_str(),
                    _ => {
                        save_frame(vm, frame_idx, &frame);
                        return Err(VMSignal::Error(vm.vm_error("MATCH_TAG name must be string", &frame)));
                    }
                };
                let target = inst.operand2 as usize;
                let value = vm.peek(0);

                if value.is_number() {
                    frame.ip = target;
                } else if value.is_nil() {
                    if tag_name == "Nil" {
                        // match
                    } else {
                        frame.ip = target;
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
                        frame.ip = target;
                    }
                } else {
                    frame.ip = target;
                }
            }

            // --- Effects ---
            Op::PushHandler => {
                let descriptor_str = match &frame.closure.proto.chunk.constants[inst.operand as usize] {
                    ConstantValue::String(s) if s.starts_with("\0handler:") => s.clone(),
                    _ => {
                        save_frame(vm, frame_idx, &frame);
                        return Err(VMSignal::Error(vm.vm_error("Invalid handler descriptor", &frame)));
                    }
                };

                let json = &descriptor_str["\0handler:".len()..];
                let descriptor = match parse_handler_descriptor(json) {
                    Ok(d) => d,
                    Err(e) => {
                        save_frame(vm, frame_idx, &frame);
                        return Err(VMSignal::Error(vm.vm_error(
                            format!("Failed to parse handler descriptor: {}", e),
                            &frame,
                        )));
                    }
                };

                // Create body closure from prototype
                let body_proto = match &frame.closure.proto.chunk.constants[descriptor.body_proto_idx as usize] {
                    ConstantValue::Prototype(p) => p,
                    _ => {
                        save_frame(vm, frame_idx, &frame);
                        return Err(VMSignal::Error(vm.vm_error("Expected body prototype", &frame)));
                    }
                };
                let body_closure = create_closure_from_proto(&frame, body_proto);

                // Create handler clause closures
                let mut handler_entries = Vec::new();
                for h in &descriptor.handlers {
                    let op_name = match &frame.closure.proto.chunk.constants[h.op_name_idx as usize] {
                        ConstantValue::String(s) => s.clone(),
                        _ => {
                            save_frame(vm, frame_idx, &frame);
                            return Err(VMSignal::Error(vm.vm_error("Invalid handler clause", &frame)));
                        }
                    };
                    let proto = match &frame.closure.proto.chunk.constants[h.proto_idx as usize] {
                        ConstantValue::Prototype(p) => p,
                        _ => {
                            save_frame(vm, frame_idx, &frame);
                            return Err(VMSignal::Error(vm.vm_error("Invalid handler clause", &frame)));
                        }
                    };
                    handler_entries.push(HandlerEntry {
                        op_name,
                        closure: create_closure_from_proto(&frame, proto),
                        arity: h.arity,
                    });
                }

                // Create return handler closure
                let return_closure = if descriptor.return_proto_idx >= 0 {
                    match &frame.closure.proto.chunk.constants[descriptor.return_proto_idx as usize] {
                        ConstantValue::Prototype(p) => Some(create_closure_from_proto(&frame, p)),
                        _ => {
                            save_frame(vm, frame_idx, &frame);
                            return Err(VMSignal::Error(vm.vm_error("Expected return handler prototype", &frame)));
                        }
                    }
                } else {
                    None
                };

                // Save current frame before calling handler
                save_frame(vm, frame_idx, &frame);

                // Run body with handler installed
                let mut result = run_with_handler(
                    vm,
                    &handler_entries,
                    &return_closure,
                    &body_closure,
                    &[],
                )?;

                // Apply return handler to the final result
                if let Some(ref ret_closure) = return_closure {
                    result = call_closure(vm, ret_closure, &[result])?;
                }

                // Restore frame (it may have been clobbered by handler execution)
                frame = vm.frames[frame_idx].clone();
                vm_constants = preconvert_constants_recursive(&frame.closure.proto.chunk.constants);
                vm.push(result);
            }

            Op::PopHandler => {
                // Currently unused — handler cleanup managed by run_with_handler
            }

            Op::Perform => {
                let op_name = match &frame.closure.proto.chunk.constants[inst.operand as usize] {
                    ConstantValue::String(s) => s.clone(),
                    _ => {
                        save_frame(vm, frame_idx, &frame);
                        return Err(VMSignal::Error(vm.vm_error("PERFORM name must be string", &frame)));
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
                    if vm.handler_stack[i].handlers.iter().any(|e| e.op_name == op_name) {
                        found = Some(i);
                        break;
                    }
                }

                if let Some(handler_idx) = found {
                    // Extract handler data before borrowing vm mutably
                    let handler_id = vm.handler_stack[handler_idx].id;
                    let capture_frame_start = vm.handler_stack[handler_idx].capture_frame_start;
                    let capture_stack_start = vm.handler_stack[handler_idx].capture_stack_start;
                    let unwind_fc = vm.handler_stack[handler_idx].unwind_frame_start;
                    let unwind_st = vm.handler_stack[handler_idx].unwind_stack_start;

                    // Capture continuation
                    // Save current frame first
                    save_frame(vm, frame_idx, &frame);
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

                    // Capture nested handler frames
                    let captured_handlers: Vec<VMHandlerFrame> = vm.handler_stack[handler_idx + 1..].to_vec();

                    let continuation = VMContinuation {
                        frames: captured_frames,
                        stack: captured_stack,
                        handler_stack: captured_handlers,
                        id: next_continuation_id(),
                        frame_base: capture_frame_start,
                        stack_base: capture_stack_start,
                    };

                    // Unwind VM state — clear stack slots to release Arc references
                    vm.clear_stack_slots(unwind_st);
                    vm.frame_count = unwind_fc;
                    vm.stack_top = unwind_st;
                    vm.handler_stack.truncate(handler_idx); // remove this handler and everything above

                    return Err(VMSignal::Effect(EffectSignal {
                        op_name,
                        args,
                        continuation: Value::object(VMObject::Continuation(Box::new(continuation))),
                        target_handler_id: handler_id,
                    }));
                }

                // No user handler found — try default handlers
                if let Some(result) = try_default_handler(vm, &op_name, &args) {
                    vm.push(result);
                } else {
                    save_frame(vm, frame_idx, &frame);
                    return Err(VMSignal::Error(vm.vm_error(
                        format!("Unhandled effect: {}", op_name),
                        &frame,
                    )));
                }
            }

            Op::Resume => {
                let value = vm.pop();
                let cont_val = vm.pop();

                let cont = match cont_val.as_object() {
                    Some(VMObject::Continuation(c)) => c,
                    _ => {
                        save_frame(vm, frame_idx, &frame);
                        return Err(VMSignal::Error(vm.vm_error("resume expects a continuation", &frame)));
                    }
                };

                // Enforce one-shot semantics via VM-level tracking (works across Arc aliases)
                if !vm.resumed_continuations.insert(cont.id) {
                    save_frame(vm, frame_idx, &frame);
                    return Err(VMSignal::Error(vm.vm_error("Continuation already resumed (one-shot)", &frame)));
                }

                let cont_data = (**cont).clone();

                // Update the current (top) handler frame's capture/unwind points
                save_frame(vm, frame_idx, &frame);
                if let Some(top_handler) = vm.handler_stack.last_mut() {
                    top_handler.capture_frame_start = vm.frame_count;
                    top_handler.capture_stack_start = vm.stack_top;
                    top_handler.unwind_frame_start = vm.frame_count;
                    top_handler.unwind_stack_start = vm.stack_top;
                }

                // Restore continuation frames
                let resume_base = vm.frame_count;
                for f in &cont_data.frames {
                    vm.push_frame(CallFrame {
                        closure: f.closure.clone(),
                        ip: f.ip,
                        locals: f.locals.clone(),
                    });
                }

                // Restore continuation stack
                for v in &cont_data.stack {
                    vm.push(v.clone());
                }

                // Push resume value
                vm.push(value);

                // Restore nested handlers, rebasing indices
                let saved_handler_len = vm.handler_stack.len();
                let frame_offset = resume_base as i64 - cont_data.frame_base as i64;
                let stack_offset = (vm.stack_top as i64 - cont_data.stack.len() as i64 - 1) - cont_data.stack_base as i64;

                let mut restored_handlers = Vec::new();
                for h in &cont_data.handler_stack {
                    let rebased = rebase_handler(h, frame_offset, stack_offset);
                    restored_handlers.push(rebased.clone());
                    vm.handler_stack.push(rebased);
                }

                // Save top handler data before running (PERFORM may unwind the stack)
                let top_handler_data = if saved_handler_len > 0 {
                    let th = &vm.handler_stack[saved_handler_len - 1];
                    Some((th.id, th.handlers.clone(), th.return_handler.clone()))
                } else {
                    None
                };

                // Run restored continuation
                let result = if restored_handlers.is_empty() {
                    run(vm, resume_base)
                } else {
                    run_restored_continuation(vm, resume_base, &restored_handlers, saved_handler_len)
                };

                // Handle the result
                let result = match result {
                    Ok(v) => v,
                    Err(VMSignal::Effect(signal)) => {
                        // Check if body re-performed an effect handled by the enclosing handler
                        if let Some((top_id, ref entries, ref ret_handler)) = top_handler_data {
                            if signal.target_handler_id == top_id {
                                let clause = entries.iter().find(|e| e.op_name == signal.op_name).cloned();

                                if let Some(clause) = clause {
                                    let expected_arg_count = (clause.arity as usize).saturating_sub(1);
                                    let mut handler_args = Vec::new();
                                    for i in 0..expected_arg_count.min(signal.args.len()) {
                                        handler_args.push(signal.args[i].clone());
                                    }
                                    handler_args.push(signal.continuation);

                                    run_with_handler(
                                        vm,
                                        entries,
                                        ret_handler,
                                        &clause.closure,
                                        &handler_args,
                                    )?
                                } else {
                                    return Err(VMSignal::Effect(signal));
                                }
                            } else {
                                return Err(VMSignal::Effect(signal));
                            }
                        } else {
                            return Err(VMSignal::Effect(signal));
                        }
                    }
                    Err(e) => return Err(e),
                };

                // Restore VM state
                vm.frame_count = resume_base;
                vm.handler_stack.truncate(saved_handler_len);

                // Restore frame
                frame_idx = vm.frame_count - 1;
                frame = vm.frames[frame_idx].clone();
                vm_constants = preconvert_constants_recursive(&frame.closure.proto.chunk.constants);
                vm.push(result);
            }

            Op::Halt => {
                let msg = vm.pop();
                if let Some(s) = msg.as_string() {
                    save_frame(vm, frame_idx, &frame);
                    return Err(VMSignal::Error(vm.vm_error(s, &frame)));
                }
                save_frame(vm, frame_idx, &frame);
                return Err(VMSignal::Error(vm.vm_error("HALT", &frame)));
            }
        }
    }
}

/// Save a local frame copy back to the VM's frame array.
#[inline]
fn save_frame(vm: &mut VM, idx: usize, frame: &CallFrame) {
    if idx < vm.frames.len() {
        vm.frames[idx] = frame.clone();
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
            if la.len() != lb.len() { return false; }
            la.iter().zip(lb.iter()).all(|(a, b)| values_equal(a, b))
        }
        (VMObject::AdtValue { tag: t1, fields: f1 }, VMObject::AdtValue { tag: t2, fields: f2 }) => {
            if t1 != t2 || f1.len() != f2.len() { return false; }
            f1.iter().zip(f2.iter()).all(|(a, b)| values_equal(a, b))
        }
        // nil-equivalent comparisons
        (VMObject::List(elements), VMObject::AdtValue { tag, fields }) |
        (VMObject::AdtValue { tag, fields }, VMObject::List(elements)) => {
            elements.is_empty() && tag == "Nil" && fields.is_empty()
        }
        (VMObject::Pid(a), VMObject::Pid(b)) => a == b,
        (VMObject::Channel(a), VMObject::Channel(b)) => a == b,
        _ => false,
    }
}
