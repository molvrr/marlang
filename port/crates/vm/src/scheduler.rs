//! Single-threaded synchronous scheduler for Marlang concurrency.
//!
//! Implements fibers (green threads), channels (CSP), actors (mailboxes,
//! selective receive, supervision), and synchronous IO — all powered by
//! the existing effect/continuation machinery.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use crate::bytecode::{Chunk, FunctionPrototype};
use crate::errors::VMError;
use crate::object::{Closure, HandlerEntry, VMHandlerFrame, VMObject};
use crate::value::{format_value_unquoted, Value};
use crate::vm::{
    call_closure, call_value, next_handler_id, resume_continuation, EffectSignal, VMSignal, VM,
};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

type FiberId = u64;

enum Fiber {
    New {
        closure: Closure,
        pid: FiberId,
    },
    Suspended {
        continuation: Value,
        resume_value: Value,
        pid: FiberId,
    },
}

impl Fiber {
    fn pid(&self) -> FiberId {
        match self {
            Fiber::New { pid, .. } | Fiber::Suspended { pid, .. } => *pid,
        }
    }
}

struct ChannelState {
    buffer: VecDeque<Value>,
    capacity: usize,
    closed: bool,
    waiting_senders: Vec<WaitingSender>,
    waiting_receivers: Vec<WaitingReceiver>,
}

struct WaitingSender {
    pid: FiberId,
    value: Value,
    continuation: Value,
}

struct WaitingReceiver {
    pid: FiberId,
    continuation: Value,
}

struct BlockedReceiver {
    continuation: Value,
    predicate: Option<Closure>,
}

enum ExitReason {
    Normal,
    Crash(String),
}

struct SchedulerState {
    ready_queue: VecDeque<Fiber>,
    next_pid: FiberId,
    current_pid: FiberId,
    live_pids: HashSet<FiberId>,

    // Channels
    channels: HashMap<u64, ChannelState>,
    next_channel_id: u64,

    // Actors
    mailboxes: HashMap<FiberId, VecDeque<Value>>,
    blocked_on_receive: HashMap<FiberId, BlockedReceiver>,

    // Supervision
    monitors: HashMap<FiberId, HashSet<FiberId>>,
    links: HashMap<FiberId, HashSet<FiberId>>,
    trap_exit: HashSet<FiberId>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn enqueue_resumed(
    state: &mut SchedulerState,
    pid: FiberId,
    continuation: Value,
    resume_value: Value,
) {
    state.ready_queue.push_back(Fiber::Suspended {
        continuation,
        resume_value,
        pid,
    });
}

fn as_pid(val: &Value) -> Option<u64> {
    match val.as_object() {
        Some(VMObject::Pid(id)) => Some(*id),
        _ => None,
    }
}

fn as_channel(val: &Value) -> Option<u64> {
    match val.as_object() {
        Some(VMObject::Port(id)) => Some(*id),
        _ => None,
    }
}

/// Call a predicate closure synchronously. Returns true if truthy, false on error.
fn call_predicate_safe(vm: &mut VM, predicate: &Closure, value: &Value) -> bool {
    let pred_val = Value::object(VMObject::Closure(Box::new(predicate.clone())));
    match call_value(vm, &pred_val, &[value.clone()]) {
        Ok(result) => result.is_truthy(),
        Err(_) => false,
    }
}

fn deliver_message(
    state: &mut SchedulerState,
    vm: &mut VM,
    target_pid: FiberId,
    msg: Value,
) {
    let mailbox = match state.mailboxes.get_mut(&target_pid) {
        Some(m) => m,
        None => return,
    };
    mailbox.push_back(msg.clone());

    // Check if target is blocked on receive
    if let Some(blocked) = state.blocked_on_receive.remove(&target_pid) {
        if let Some(ref predicate) = blocked.predicate {
            // Selective receive: test predicate against new message
            if call_predicate_safe(vm, predicate, &msg) {
                // Remove the message we just pushed (it's the last one)
                let mailbox = state.mailboxes.get_mut(&target_pid).unwrap();
                mailbox.pop_back();
                enqueue_resumed(state, target_pid, blocked.continuation, msg);
            } else {
                // Predicate didn't match — re-block
                state.blocked_on_receive.insert(target_pid, blocked);
            }
        } else {
            // Non-selective: dequeue first message, wake receiver
            let mailbox = state.mailboxes.get_mut(&target_pid).unwrap();
            let first = mailbox.pop_front().unwrap();
            enqueue_resumed(state, target_pid, blocked.continuation, first);
        }
    }
}

fn kill_fiber(state: &mut SchedulerState, vm: &mut VM, pid: FiberId, reason: &str) {
    // Remove from ready queue
    state.ready_queue.retain(|f| f.pid() != pid);
    // Remove from blocked
    state.blocked_on_receive.remove(&pid);
    // Process exit
    handle_fiber_exit(state, vm, pid, ExitReason::Crash(reason.to_string()));
}

fn handle_fiber_exit(
    state: &mut SchedulerState,
    vm: &mut VM,
    pid: FiberId,
    reason: ExitReason,
) {
    state.live_pids.remove(&pid);
    let reason_str = match &reason {
        ExitReason::Normal => "normal".to_string(),
        ExitReason::Crash(msg) => msg.clone(),
    };

    // Notify monitors: deliver (Down pid reason)
    if let Some(monitor_set) = state.monitors.remove(&pid) {
        for monitor_pid in monitor_set {
            if state.live_pids.contains(&monitor_pid) {
                let down_msg = Value::object(VMObject::AdtValue {
                    tag: "Down".to_string(),
                    fields: vec![
                        Value::object(VMObject::Pid(pid)),
                        Value::string(reason_str.as_str()),
                    ],
                });
                deliver_message(state, vm, monitor_pid, down_msg);
            }
        }
    }

    // Process links
    if let Some(linked_set) = state.links.remove(&pid) {
        for linked_pid in linked_set {
            // Remove reverse link
            if let Some(reverse_links) = state.links.get_mut(&linked_pid) {
                reverse_links.remove(&pid);
            }
            if !state.live_pids.contains(&linked_pid) {
                continue;
            }
            if matches!(reason, ExitReason::Normal) {
                continue;
            }

            if state.trap_exit.contains(&linked_pid) {
                let exit_msg = Value::object(VMObject::AdtValue {
                    tag: "Exit".to_string(),
                    fields: vec![
                        Value::object(VMObject::Pid(pid)),
                        Value::string(reason_str.as_str()),
                    ],
                });
                deliver_message(state, vm, linked_pid, exit_msg);
            } else {
                kill_fiber(state, vm, linked_pid, &reason_str);
            }
        }
    }

    // Clean up
    state.mailboxes.remove(&pid);
    state.blocked_on_receive.remove(&pid);
}

// ---------------------------------------------------------------------------
// Handler frame installation
// ---------------------------------------------------------------------------

const HANDLED_EFFECTS: &[&str] = &[
    // Layer 1: Fibers + Channels
    "fiber-spawn",
    "fiber-yield",
    "fiber-self",
    "chan-new",
    "chan-send",
    "chan-recv",
    "chan-close",
    // Layer 2: Actors
    "actor-send",
    "actor-receive",
    "actor-receive-match",
    "actor-monitor",
    "actor-link",
    "actor-unlink",
    "actor-trap-exit",
    // Layer 3: IO
    "print",
    "println",
    "read-line",
    "read-file",
    "write-file",
];

fn make_dummy_closure() -> Closure {
    Closure {
        proto: Arc::new(FunctionPrototype {
            name: "<scheduler>".to_string(),
            arity: 0,
            upvalues: Vec::new(),
            chunk: Chunk::new(),
        }),
        upvalues: Vec::new(),
    }
}

fn install_handler_frame(vm: &mut VM) {
    let dummy = make_dummy_closure();
    let entries: Vec<HandlerEntry> = HANDLED_EFFECTS
        .iter()
        .map(|name| HandlerEntry {
            op_name: name.to_string(),
            closure: dummy.clone(),
            arity: 1,
        })
        .collect();

    let handler_frame = VMHandlerFrame {
        handlers: entries,
        return_handler: None,
        unwind_frame_start: vm.frame_count,
        unwind_stack_start: vm.stack_top,
        capture_frame_start: vm.frame_count,
        capture_stack_start: vm.stack_top,
        id: next_handler_id(),
    };
    vm.handler_stack.push(handler_frame);
}

fn remove_handler_frame(vm: &mut VM) {
    vm.handler_stack.pop();
}

// ---------------------------------------------------------------------------
// Effect Dispatch
// ---------------------------------------------------------------------------

fn handle_effect(
    state: &mut SchedulerState,
    vm: &mut VM,
    signal: EffectSignal,
) -> Result<(), VMError> {
    let cont = signal.continuation;
    let args = signal.args;

    match signal.op_name.as_str() {
        // --- Layer 1: Fibers ---

        "fiber-spawn" => {
            let thunk = match args.first().and_then(|v| v.as_object()) {
                Some(VMObject::Closure(c)) => (**c).clone(),
                _ => return Err(VMError::runtime("fiber-spawn: argument must be a closure")),
            };
            let pid = state.next_pid;
            state.next_pid += 1;
            state.live_pids.insert(pid);
            state.mailboxes.insert(pid, VecDeque::new());
            // Resume parent first (so it can set up monitors/links before child runs)
            enqueue_resumed(
                state,
                state.current_pid,
                cont,
                Value::object(VMObject::Pid(pid)),
            );
            state
                .ready_queue
                .push_back(Fiber::New { closure: thunk, pid });
        }

        "fiber-yield" => {
            enqueue_resumed(state, state.current_pid, cont, Value::nil());
        }

        "fiber-self" => {
            enqueue_resumed(
                state,
                state.current_pid,
                cont,
                Value::object(VMObject::Pid(state.current_pid)),
            );
        }

        // --- Layer 1: Channels ---

        "chan-new" => {
            let capacity = args
                .first()
                .and_then(|v| v.as_number())
                .unwrap_or(0.0) as usize;
            let id = state.next_channel_id;
            state.next_channel_id += 1;
            state.channels.insert(
                id,
                ChannelState {
                    buffer: VecDeque::new(),
                    capacity,
                    closed: false,
                    waiting_senders: Vec::new(),
                    waiting_receivers: Vec::new(),
                },
            );
            enqueue_resumed(
                state,
                state.current_pid,
                cont,
                Value::object(VMObject::Port(id)),
            );
        }

        "chan-send" => {
            let ch_id = as_channel(args.first().unwrap_or(&Value::nil()))
                .ok_or_else(|| VMError::runtime("chan-send: first argument must be a channel"))?;
            let value = args.get(1).cloned().unwrap_or_else(Value::nil);
            let chan = state
                .channels
                .get_mut(&ch_id)
                .ok_or_else(|| VMError::runtime("chan-send: invalid channel"))?;

            if chan.closed {
                return Err(VMError::runtime("Cannot send on closed channel"));
            }

            if !chan.waiting_receivers.is_empty() {
                // Hand off to waiting receiver
                let receiver = chan.waiting_receivers.remove(0);
                enqueue_resumed(state, receiver.pid, receiver.continuation, value);
                enqueue_resumed(state, state.current_pid, cont, Value::nil());
            } else if chan.buffer.len() < chan.capacity {
                // Buffer the value
                chan.buffer.push_back(value);
                enqueue_resumed(state, state.current_pid, cont, Value::nil());
            } else {
                // Block sender
                chan.waiting_senders.push(WaitingSender {
                    pid: state.current_pid,
                    value,
                    continuation: cont,
                });
            }
        }

        "chan-recv" => {
            let ch_id = as_channel(args.first().unwrap_or(&Value::nil()))
                .ok_or_else(|| VMError::runtime("chan-recv: first argument must be a channel"))?;
            let chan = state
                .channels
                .get_mut(&ch_id)
                .ok_or_else(|| VMError::runtime("chan-recv: invalid channel"))?;

            if !chan.buffer.is_empty() {
                let value = chan.buffer.pop_front().unwrap();
                // If there's a waiting sender, move their value into the buffer
                if !chan.waiting_senders.is_empty() {
                    let sender = chan.waiting_senders.remove(0);
                    chan.buffer.push_back(sender.value);
                    enqueue_resumed(state, sender.pid, sender.continuation, Value::nil());
                }
                enqueue_resumed(state, state.current_pid, cont, value);
            } else if !chan.waiting_senders.is_empty() {
                // Unbuffered: hand off directly from sender
                let sender = chan.waiting_senders.remove(0);
                enqueue_resumed(state, sender.pid, sender.continuation, Value::nil());
                enqueue_resumed(state, state.current_pid, cont, sender.value);
            } else if chan.closed {
                enqueue_resumed(state, state.current_pid, cont, Value::nil());
            } else {
                // Block receiver
                chan.waiting_receivers.push(WaitingReceiver {
                    pid: state.current_pid,
                    continuation: cont,
                });
            }
        }

        "chan-close" => {
            let ch_id = as_channel(args.first().unwrap_or(&Value::nil()))
                .ok_or_else(|| VMError::runtime("chan-close: first argument must be a channel"))?;
            let chan = state
                .channels
                .get_mut(&ch_id)
                .ok_or_else(|| VMError::runtime("chan-close: invalid channel"))?;

            chan.closed = true;
            // Wake all blocked receivers with nil
            let receivers: Vec<WaitingReceiver> = chan.waiting_receivers.drain(..).collect();
            for receiver in receivers {
                enqueue_resumed(state, receiver.pid, receiver.continuation, Value::nil());
            }
            enqueue_resumed(state, state.current_pid, cont, Value::nil());
        }

        // --- Layer 2: Actors ---

        "actor-send" => {
            let target_pid = as_pid(args.first().unwrap_or(&Value::nil()))
                .ok_or_else(|| VMError::runtime("actor-send: first argument must be a pid"))?;
            let msg = args.get(1).cloned().unwrap_or_else(Value::nil);
            deliver_message(state, vm, target_pid, msg);
            enqueue_resumed(state, state.current_pid, cont, Value::nil());
        }

        "actor-receive" => {
            let mailbox = state.mailboxes.get_mut(&state.current_pid);
            if let Some(mailbox) = mailbox {
                if !mailbox.is_empty() {
                    let msg = mailbox.pop_front().unwrap();
                    enqueue_resumed(state, state.current_pid, cont, msg);
                    return Ok(());
                }
            }
            // Block until message arrives
            state.blocked_on_receive.insert(
                state.current_pid,
                BlockedReceiver {
                    continuation: cont,
                    predicate: None,
                },
            );
        }

        "actor-receive-match" => {
            let predicate = match args.first().and_then(|v| v.as_object()) {
                Some(VMObject::Closure(c)) => (**c).clone(),
                _ => {
                    return Err(VMError::runtime(
                        "actor-receive-match: argument must be a closure",
                    ))
                }
            };

            // Scan mailbox for matching message
            let current_pid = state.current_pid;
            if let Some(mailbox) = state.mailboxes.get_mut(&current_pid) {
                for i in 0..mailbox.len() {
                    if call_predicate_safe(vm, &predicate, &mailbox[i]) {
                        let msg = mailbox.remove(i).unwrap();
                        enqueue_resumed(state, current_pid, cont, msg);
                        return Ok(());
                    }
                }
            }

            // No match — block with predicate
            state.blocked_on_receive.insert(
                current_pid,
                BlockedReceiver {
                    continuation: cont,
                    predicate: Some(predicate),
                },
            );
        }

        "actor-monitor" => {
            let target_pid = as_pid(args.first().unwrap_or(&Value::nil()))
                .ok_or_else(|| VMError::runtime("actor-monitor: argument must be a pid"))?;
            state
                .monitors
                .entry(target_pid)
                .or_default()
                .insert(state.current_pid);
            enqueue_resumed(state, state.current_pid, cont, Value::nil());
        }

        "actor-link" => {
            let target_pid = as_pid(args.first().unwrap_or(&Value::nil()))
                .ok_or_else(|| VMError::runtime("actor-link: argument must be a pid"))?;
            state
                .links
                .entry(state.current_pid)
                .or_default()
                .insert(target_pid);
            state
                .links
                .entry(target_pid)
                .or_default()
                .insert(state.current_pid);
            enqueue_resumed(state, state.current_pid, cont, Value::nil());
        }

        "actor-unlink" => {
            let target_pid = as_pid(args.first().unwrap_or(&Value::nil()))
                .ok_or_else(|| VMError::runtime("actor-unlink: argument must be a pid"))?;
            if let Some(set) = state.links.get_mut(&state.current_pid) {
                set.remove(&target_pid);
            }
            if let Some(set) = state.links.get_mut(&target_pid) {
                set.remove(&state.current_pid);
            }
            enqueue_resumed(state, state.current_pid, cont, Value::nil());
        }

        "actor-trap-exit" => {
            let enabled = args.first().cloned().unwrap_or_else(Value::nil);
            if enabled.is_truthy() {
                state.trap_exit.insert(state.current_pid);
            } else {
                state.trap_exit.remove(&state.current_pid);
            }
            enqueue_resumed(state, state.current_pid, cont, Value::nil());
        }

        // --- Layer 3: IO effects (synchronous) ---

        "print" => {
            let msg = args.first().cloned().unwrap_or_else(Value::nil);
            let text = format_value_unquoted(&msg);
            if let Some(ref mut buf) = vm.captured_output {
                buf.push_str(&text);
            } else {
                print!("{}", text);
            }
            enqueue_resumed(state, state.current_pid, cont, Value::nil());
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
            enqueue_resumed(state, state.current_pid, cont, Value::nil());
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
            enqueue_resumed(
                state,
                state.current_pid,
                cont,
                Value::string(line.as_str()),
            );
        }

        "read-file" => {
            let path = args
                .first()
                .and_then(|v| v.as_string())
                .ok_or_else(|| VMError::runtime("read-file: path must be a string"))?;
            match std::fs::read_to_string(path) {
                Ok(content) => {
                    enqueue_resumed(
                        state,
                        state.current_pid,
                        cont,
                        Value::string(content.as_str()),
                    );
                }
                Err(e) => {
                    return Err(VMError::runtime(format!("read-file: {}", e)));
                }
            }
        }

        "write-file" => {
            let path = args
                .first()
                .and_then(|v| v.as_string())
                .ok_or_else(|| VMError::runtime("write-file: path must be a string"))?;
            let content = args
                .get(1)
                .and_then(|v| v.as_string())
                .ok_or_else(|| VMError::runtime("write-file: content must be a string"))?;
            std::fs::write(path, content)
                .map_err(|e| VMError::runtime(format!("write-file: {}", e)))?;
            enqueue_resumed(state, state.current_pid, cont, Value::nil());
        }

        _ => {
            return Err(VMError::runtime(format!(
                "Scheduler: unhandled effect '{}'",
                signal.op_name
            )));
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Entry Point
// ---------------------------------------------------------------------------

/// Run a closure as the main fiber, scheduling any spawned child fibers
/// cooperatively until all are complete.
pub fn run_fibers(vm: &mut VM, body: &Closure) -> Result<Value, VMError> {
    let mut state = SchedulerState {
        ready_queue: VecDeque::new(),
        next_pid: 1, // 0 is main
        current_pid: 0,
        live_pids: HashSet::from([0]),
        channels: HashMap::new(),
        next_channel_id: 0,
        mailboxes: HashMap::from([(0, VecDeque::new())]),
        blocked_on_receive: HashMap::new(),
        monitors: HashMap::new(),
        links: HashMap::new(),
        trap_exit: HashSet::new(),
    };

    let mut main_result = Value::nil();

    // Run main fiber (PID 0)
    install_handler_frame(vm);
    match call_closure(vm, body, &[]) {
        Ok(result) => {
            remove_handler_frame(vm);
            main_result = result;
            handle_fiber_exit(&mut state, vm, 0, ExitReason::Normal);
        }
        Err(VMSignal::Effect(signal)) => {
            // PERFORM already unwound VM state and removed handler frame
            handle_effect(&mut state, vm, signal)?;
        }
        Err(VMSignal::Error(e)) => {
            // Main fiber crashed
            handle_fiber_exit(&mut state, vm, 0, ExitReason::Crash(e.message.clone()));
        }
    }

    // Synchronous scheduler loop
    while let Some(fiber) = state.ready_queue.pop_front() {
        let pid = fiber.pid();
        state.current_pid = pid;

        let saved_frame_count = vm.frame_count;
        let saved_stack_top = vm.stack_top;

        // Reinstall handler frame (PERFORM removes it when catching an effect)
        install_handler_frame(vm);

        let result = match &fiber {
            Fiber::New { closure, .. } => call_closure(vm, closure, &[]),
            Fiber::Suspended {
                continuation,
                resume_value,
                ..
            } => match continuation.as_object() {
                Some(VMObject::Continuation(c)) => {
                    resume_continuation(vm, c.as_ref(), resume_value.clone())
                }
                _ => Err(VMSignal::Error(VMError::runtime(
                    "Invalid continuation in fiber",
                ))),
            },
        };

        match result {
            Ok(result) => {
                remove_handler_frame(vm);
                if pid == 0 {
                    main_result = result;
                }
                handle_fiber_exit(&mut state, vm, pid, ExitReason::Normal);
            }
            Err(VMSignal::Effect(signal)) => {
                // PERFORM already cleaned up VM state
                handle_effect(&mut state, vm, signal)?;
            }
            Err(VMSignal::Error(e)) => {
                // Fiber crashed — restore VM state
                vm.clear_stack_slots(saved_stack_top);
                vm.frame_count = saved_frame_count;
                vm.stack_top = saved_stack_top;
                if !vm.handler_stack.is_empty() {
                    remove_handler_frame(vm);
                }
                handle_fiber_exit(
                    &mut state,
                    vm,
                    pid,
                    ExitReason::Crash(e.message.clone()),
                );
            }
        }
    }

    Ok(main_result)
}
