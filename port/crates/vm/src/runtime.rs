//! Multi-threaded scheduler for Marlang concurrency.
//!
//! N scheduler threads each own a VM instance. Fibers are distributed across
//! threads at spawn time, cross-thread communication uses crossbeam channels.
//! Bytecode execution on each thread is lock-free.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crossbeam_channel::{Receiver, Sender};
use crossbeam_deque::{Steal, Stealer, Worker};

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

// SAFETY: Fiber contains Value (Send+Sync) and Closure (Arc-based, Send+Sync).
unsafe impl Send for Fiber {}

#[allow(dead_code)]
enum SchedulerMessage {
    RunFiber(Fiber),
    WakeWithValue { pid: FiberId, value: Value },
    MailboxNotify { pid: FiberId },
    IoComplete { pid: FiberId, result: Result<Value, String> },
    Shutdown,
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

// ---------------------------------------------------------------------------
// Shared State (Arc-wrapped, accessed by all scheduler threads)
// ---------------------------------------------------------------------------

struct SharedState {
    // PID registry
    next_pid: AtomicU64,
    alive: Mutex<HashSet<FiberId>>,

    // PID → scheduler_id mapping for wakeup routing
    pid_to_scheduler: Mutex<HashMap<FiberId, usize>>,

    // Centralized mailboxes (simplifies work stealing — mailbox doesn't move with fiber)
    mailboxes: Mutex<HashMap<FiberId, VecDeque<Value>>>,

    // Parked fibers waiting on receive (pid → scheduler_id that holds the continuation)
    parked_pids: Mutex<HashMap<FiberId, usize>>,

    // Channels
    channels: Mutex<HashMap<u64, ChannelState>>,
    next_channel_id: AtomicU64,

    // Supervision
    monitors: Mutex<HashMap<FiberId, HashSet<FiberId>>>,
    links: Mutex<HashMap<FiberId, HashSet<FiberId>>>,
    trap_exit: Mutex<HashSet<FiberId>>,

    // Termination detection
    idle_count: AtomicUsize,
    total_schedulers: usize,
    shutdown: AtomicBool,

    // Per-scheduler inboxes (for sending messages to specific schedulers)
    scheduler_senders: Vec<Sender<SchedulerMessage>>,

    // Main fiber result channel
    result_sender: Mutex<Option<Sender<Result<Value, VMError>>>>,

    // Captured output (for tests — all schedulers append here)
    captured_output: Arc<Mutex<Option<String>>>,

    // Async IO support
    pending_io: AtomicUsize,
    tokio_handle: tokio::runtime::Handle,
}

// ---------------------------------------------------------------------------
// Per-Scheduler State (thread-local, no locking on hot path)
// ---------------------------------------------------------------------------

struct PerScheduler {
    id: usize,
    ready_queue: VecDeque<Fiber>,
    inbox: Receiver<SchedulerMessage>,
    parked_fibers: HashMap<FiberId, BlockedReceiver>,
    current_pid: FiberId,
    // Work stealing
    #[allow(dead_code)]
    steal_worker: Worker<Fiber>,
    stealers: Vec<Stealer<Fiber>>,
    // Round-robin counter for spawn distribution
    spawn_counter: usize,
    // IO-parked fibers (pid → continuation), awaiting async IO completion
    io_parked: HashMap<FiberId, Value>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn as_pid(val: &Value) -> Option<u64> {
    match val.as_object() {
        Some(VMObject::Pid(id)) => Some(*id),
        _ => None,
    }
}

fn as_channel(val: &Value) -> Option<u64> {
    match val.as_object() {
        Some(VMObject::Channel(id)) => Some(*id),
        _ => None,
    }
}

fn call_predicate_safe(vm: &mut VM, predicate: &Closure, value: &Value) -> bool {
    let pred_val = Value::object(VMObject::Closure(Box::new(predicate.clone())));
    match call_value(vm, &pred_val, &[value.clone()]) {
        Ok(result) => result.is_truthy(),
        Err(_) => false,
    }
}

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

const HANDLED_EFFECTS: &[&str] = &[
    "fiber-spawn", "fiber-yield", "fiber-self",
    "chan-new", "chan-send", "chan-recv", "chan-close",
    "actor-send", "actor-receive", "actor-receive-match",
    "actor-monitor", "actor-link", "actor-unlink", "actor-trap-exit",
    "print", "println", "read-line", "read-file", "write-file",
];

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
    sched: &mut PerScheduler,
    shared: &SharedState,
    vm: &mut VM,
    signal: EffectSignal,
) -> Result<(), VMError> {
    let cont = signal.continuation;
    let args = signal.args;

    match signal.op_name.as_str() {
        // --- Fibers ---

        "fiber-spawn" => {
            let thunk = match args.first().and_then(|v| v.as_object()) {
                Some(VMObject::Closure(c)) => (**c).clone(),
                _ => return Err(VMError::runtime("fiber-spawn: argument must be a closure")),
            };
            let pid = shared.next_pid.fetch_add(1, Ordering::Relaxed);
            {
                let mut alive = shared.alive.lock().unwrap();
                alive.insert(pid);
            }
            {
                let mut mailboxes = shared.mailboxes.lock().unwrap();
                mailboxes.insert(pid, VecDeque::new());
            }

            // Resume parent first
            sched.ready_queue.push_back(Fiber::Suspended {
                continuation: cont,
                resume_value: Value::object(VMObject::Pid(pid)),
                pid: sched.current_pid,
            });

            // Decide where to run the child
            let target = pick_scheduler(sched, shared);
            let new_fiber = Fiber::New { closure: thunk, pid };

            if target == sched.id {
                sched.ready_queue.push_back(new_fiber);
                // Register pid to our scheduler
                let mut pid_map = shared.pid_to_scheduler.lock().unwrap();
                pid_map.insert(pid, sched.id);
            } else {
                // Register pid to target scheduler
                {
                    let mut pid_map = shared.pid_to_scheduler.lock().unwrap();
                    pid_map.insert(pid, target);
                }
                let _ = shared.scheduler_senders[target].send(SchedulerMessage::RunFiber(new_fiber));
            }
        }

        "fiber-yield" => {
            sched.ready_queue.push_back(Fiber::Suspended {
                continuation: cont,
                resume_value: Value::nil(),
                pid: sched.current_pid,
            });
        }

        "fiber-self" => {
            sched.ready_queue.push_back(Fiber::Suspended {
                continuation: cont,
                resume_value: Value::object(VMObject::Pid(sched.current_pid)),
                pid: sched.current_pid,
            });
        }

        // --- Channels ---

        "chan-new" => {
            let capacity = args
                .first()
                .and_then(|v| v.as_number())
                .unwrap_or(0.0) as usize;
            let id = shared.next_channel_id.fetch_add(1, Ordering::Relaxed);
            {
                let mut channels = shared.channels.lock().unwrap();
                channels.insert(id, ChannelState {
                    buffer: VecDeque::new(),
                    capacity,
                    closed: false,
                    waiting_senders: Vec::new(),
                    waiting_receivers: Vec::new(),
                });
            }
            sched.ready_queue.push_back(Fiber::Suspended {
                continuation: cont,
                resume_value: Value::object(VMObject::Channel(id)),
                pid: sched.current_pid,
            });
        }

        "chan-send" => {
            let ch_id = as_channel(args.first().unwrap_or(&Value::nil()))
                .ok_or_else(|| VMError::runtime("chan-send: first argument must be a channel"))?;
            let value = args.get(1).cloned().unwrap_or_else(Value::nil);

            let mut channels = shared.channels.lock().unwrap();
            let chan = channels
                .get_mut(&ch_id)
                .ok_or_else(|| VMError::runtime("chan-send: invalid channel"))?;

            if chan.closed {
                return Err(VMError::runtime("Cannot send on closed channel"));
            }

            if !chan.waiting_receivers.is_empty() {
                let receiver = chan.waiting_receivers.remove(0);
                drop(channels);
                // Wake the receiver on its scheduler
                wake_fiber(sched, shared, receiver.pid, Fiber::Suspended {
                    continuation: receiver.continuation,
                    resume_value: value,
                    pid: receiver.pid,
                });
                sched.ready_queue.push_back(Fiber::Suspended {
                    continuation: cont,
                    resume_value: Value::nil(),
                    pid: sched.current_pid,
                });
            } else if chan.buffer.len() < chan.capacity {
                chan.buffer.push_back(value);
                drop(channels);
                sched.ready_queue.push_back(Fiber::Suspended {
                    continuation: cont,
                    resume_value: Value::nil(),
                    pid: sched.current_pid,
                });
            } else {
                // Block sender
                chan.waiting_senders.push(WaitingSender {
                    pid: sched.current_pid,
                    value,
                    continuation: cont,
                });
            }
        }

        "chan-recv" => {
            let ch_id = as_channel(args.first().unwrap_or(&Value::nil()))
                .ok_or_else(|| VMError::runtime("chan-recv: first argument must be a channel"))?;

            let mut channels = shared.channels.lock().unwrap();
            let chan = channels
                .get_mut(&ch_id)
                .ok_or_else(|| VMError::runtime("chan-recv: invalid channel"))?;

            if !chan.buffer.is_empty() {
                let value = chan.buffer.pop_front().unwrap();
                if !chan.waiting_senders.is_empty() {
                    let sender = chan.waiting_senders.remove(0);
                    chan.buffer.push_back(sender.value);
                    drop(channels);
                    wake_fiber(sched, shared, sender.pid, Fiber::Suspended {
                        continuation: sender.continuation,
                        resume_value: Value::nil(),
                        pid: sender.pid,
                    });
                } else {
                    drop(channels);
                }
                sched.ready_queue.push_back(Fiber::Suspended {
                    continuation: cont,
                    resume_value: value,
                    pid: sched.current_pid,
                });
            } else if !chan.waiting_senders.is_empty() {
                let sender = chan.waiting_senders.remove(0);
                drop(channels);
                wake_fiber(sched, shared, sender.pid, Fiber::Suspended {
                    continuation: sender.continuation,
                    resume_value: Value::nil(),
                    pid: sender.pid,
                });
                sched.ready_queue.push_back(Fiber::Suspended {
                    continuation: cont,
                    resume_value: sender.value,
                    pid: sched.current_pid,
                });
            } else if chan.closed {
                drop(channels);
                sched.ready_queue.push_back(Fiber::Suspended {
                    continuation: cont,
                    resume_value: Value::nil(),
                    pid: sched.current_pid,
                });
            } else {
                chan.waiting_receivers.push(WaitingReceiver {
                    pid: sched.current_pid,
                    continuation: cont,
                });
            }
        }

        "chan-close" => {
            let ch_id = as_channel(args.first().unwrap_or(&Value::nil()))
                .ok_or_else(|| VMError::runtime("chan-close: first argument must be a channel"))?;

            let mut channels = shared.channels.lock().unwrap();
            let chan = channels
                .get_mut(&ch_id)
                .ok_or_else(|| VMError::runtime("chan-close: invalid channel"))?;

            chan.closed = true;
            let receivers: Vec<WaitingReceiver> = chan.waiting_receivers.drain(..).collect();
            drop(channels);

            for receiver in receivers {
                wake_fiber(sched, shared, receiver.pid, Fiber::Suspended {
                    continuation: receiver.continuation,
                    resume_value: Value::nil(),
                    pid: receiver.pid,
                });
            }
            sched.ready_queue.push_back(Fiber::Suspended {
                continuation: cont,
                resume_value: Value::nil(),
                pid: sched.current_pid,
            });
        }

        // --- Actors ---

        "actor-send" => {
            let target_pid = as_pid(args.first().unwrap_or(&Value::nil()))
                .ok_or_else(|| VMError::runtime("actor-send: first argument must be a pid"))?;
            let msg = args.get(1).cloned().unwrap_or_else(Value::nil);

            deliver_message_shared(sched, shared, vm, target_pid, msg);

            sched.ready_queue.push_back(Fiber::Suspended {
                continuation: cont,
                resume_value: Value::nil(),
                pid: sched.current_pid,
            });
        }

        "actor-receive" => {
            let current_pid = sched.current_pid;
            let mut mailboxes = shared.mailboxes.lock().unwrap();
            if let Some(mailbox) = mailboxes.get_mut(&current_pid) {
                if !mailbox.is_empty() {
                    let msg = mailbox.pop_front().unwrap();
                    drop(mailboxes);
                    sched.ready_queue.push_back(Fiber::Suspended {
                        continuation: cont,
                        resume_value: msg,
                        pid: current_pid,
                    });
                    return Ok(());
                }
            }
            drop(mailboxes);

            // Park fiber locally, register in shared parked_pids
            sched.parked_fibers.insert(current_pid, BlockedReceiver {
                continuation: cont,
                predicate: None,
            });
            let mut parked = shared.parked_pids.lock().unwrap();
            parked.insert(current_pid, sched.id);
        }

        "actor-receive-match" => {
            let predicate = match args.first().and_then(|v| v.as_object()) {
                Some(VMObject::Closure(c)) => (**c).clone(),
                _ => return Err(VMError::runtime("actor-receive-match: argument must be a closure")),
            };

            let current_pid = sched.current_pid;
            let mut mailboxes = shared.mailboxes.lock().unwrap();
            if let Some(mailbox) = mailboxes.get_mut(&current_pid) {
                for i in 0..mailbox.len() {
                    if call_predicate_safe(vm, &predicate, &mailbox[i]) {
                        let msg = mailbox.remove(i).unwrap();
                        drop(mailboxes);
                        sched.ready_queue.push_back(Fiber::Suspended {
                            continuation: cont,
                            resume_value: msg,
                            pid: current_pid,
                        });
                        return Ok(());
                    }
                }
            }
            drop(mailboxes);

            // No match — park with predicate
            sched.parked_fibers.insert(current_pid, BlockedReceiver {
                continuation: cont,
                predicate: Some(predicate),
            });
            let mut parked = shared.parked_pids.lock().unwrap();
            parked.insert(current_pid, sched.id);
        }

        "actor-monitor" => {
            let target_pid = as_pid(args.first().unwrap_or(&Value::nil()))
                .ok_or_else(|| VMError::runtime("actor-monitor: argument must be a pid"))?;
            {
                let mut monitors = shared.monitors.lock().unwrap();
                monitors.entry(target_pid).or_default().insert(sched.current_pid);
            }
            sched.ready_queue.push_back(Fiber::Suspended {
                continuation: cont,
                resume_value: Value::nil(),
                pid: sched.current_pid,
            });
        }

        "actor-link" => {
            let target_pid = as_pid(args.first().unwrap_or(&Value::nil()))
                .ok_or_else(|| VMError::runtime("actor-link: argument must be a pid"))?;
            {
                let mut links = shared.links.lock().unwrap();
                links.entry(sched.current_pid).or_default().insert(target_pid);
                links.entry(target_pid).or_default().insert(sched.current_pid);
            }
            sched.ready_queue.push_back(Fiber::Suspended {
                continuation: cont,
                resume_value: Value::nil(),
                pid: sched.current_pid,
            });
        }

        "actor-unlink" => {
            let target_pid = as_pid(args.first().unwrap_or(&Value::nil()))
                .ok_or_else(|| VMError::runtime("actor-unlink: argument must be a pid"))?;
            {
                let mut links = shared.links.lock().unwrap();
                if let Some(set) = links.get_mut(&sched.current_pid) {
                    set.remove(&target_pid);
                }
                if let Some(set) = links.get_mut(&target_pid) {
                    set.remove(&sched.current_pid);
                }
            }
            sched.ready_queue.push_back(Fiber::Suspended {
                continuation: cont,
                resume_value: Value::nil(),
                pid: sched.current_pid,
            });
        }

        "actor-trap-exit" => {
            let enabled = args.first().cloned().unwrap_or_else(Value::nil);
            {
                let mut trap_exit = shared.trap_exit.lock().unwrap();
                if enabled.is_truthy() {
                    trap_exit.insert(sched.current_pid);
                } else {
                    trap_exit.remove(&sched.current_pid);
                }
            }
            sched.ready_queue.push_back(Fiber::Suspended {
                continuation: cont,
                resume_value: Value::nil(),
                pid: sched.current_pid,
            });
        }

        // --- IO effects (async via tokio) ---

        "print" => {
            let msg = args.first().cloned().unwrap_or_else(Value::nil);
            let text = format_value_unquoted(&msg);
            let sender = shared.scheduler_senders[sched.id].clone();
            let pid = sched.current_pid;
            let captured = shared.captured_output.clone();

            sched.io_parked.insert(pid, cont);
            shared.pending_io.fetch_add(1, Ordering::AcqRel);

            shared.tokio_handle.spawn(async move {
                {
                    let mut cap = captured.lock().unwrap();
                    if let Some(ref mut buf) = *cap {
                        buf.push_str(&text);
                    } else {
                        print!("{}", text);
                    }
                }
                let _ = sender.send(SchedulerMessage::IoComplete { pid, result: Ok(Value::nil()) });
            });
        }

        "println" => {
            let msg = args.first().cloned().unwrap_or_else(Value::nil);
            let text = format_value_unquoted(&msg);
            let sender = shared.scheduler_senders[sched.id].clone();
            let pid = sched.current_pid;
            let captured = shared.captured_output.clone();

            sched.io_parked.insert(pid, cont);
            shared.pending_io.fetch_add(1, Ordering::AcqRel);

            shared.tokio_handle.spawn(async move {
                {
                    let mut cap = captured.lock().unwrap();
                    if let Some(ref mut buf) = *cap {
                        buf.push_str(&text);
                        buf.push('\n');
                    } else {
                        println!("{}", text);
                    }
                }
                let _ = sender.send(SchedulerMessage::IoComplete { pid, result: Ok(Value::nil()) });
            });
        }

        "read-line" => {
            let sender = shared.scheduler_senders[sched.id].clone();
            let pid = sched.current_pid;

            sched.io_parked.insert(pid, cont);
            shared.pending_io.fetch_add(1, Ordering::AcqRel);

            shared.tokio_handle.spawn(async move {
                let result = tokio::task::spawn_blocking(|| {
                    let mut line = String::new();
                    std::io::stdin().read_line(&mut line).ok();
                    if line.ends_with('\n') {
                        line.pop();
                        if line.ends_with('\r') {
                            line.pop();
                        }
                    }
                    line
                }).await;

                let result = match result {
                    Ok(line) => Ok(Value::string(line.as_str())),
                    Err(e) => Err(format!("read-line: {}", e)),
                };
                let _ = sender.send(SchedulerMessage::IoComplete { pid, result });
            });
        }

        "read-file" => {
            let path = args
                .first()
                .and_then(|v| v.as_string())
                .ok_or_else(|| VMError::runtime("read-file: path must be a string"))?
                .to_string();
            let sender = shared.scheduler_senders[sched.id].clone();
            let pid = sched.current_pid;

            sched.io_parked.insert(pid, cont);
            shared.pending_io.fetch_add(1, Ordering::AcqRel);

            shared.tokio_handle.spawn(async move {
                let result = tokio::fs::read_to_string(&path).await
                    .map(|s| Value::string(&*s))
                    .map_err(|e| format!("read-file: {}", e));
                let _ = sender.send(SchedulerMessage::IoComplete { pid, result });
            });
        }

        "write-file" => {
            let path = args
                .first()
                .and_then(|v| v.as_string())
                .ok_or_else(|| VMError::runtime("write-file: path must be a string"))?
                .to_string();
            let content = args
                .get(1)
                .and_then(|v| v.as_string())
                .ok_or_else(|| VMError::runtime("write-file: content must be a string"))?
                .to_string();
            let sender = shared.scheduler_senders[sched.id].clone();
            let pid = sched.current_pid;

            sched.io_parked.insert(pid, cont);
            shared.pending_io.fetch_add(1, Ordering::AcqRel);

            shared.tokio_handle.spawn(async move {
                let result = tokio::fs::write(&path, content.as_bytes()).await
                    .map(|_| Value::nil())
                    .map_err(|e| format!("write-file: {}", e));
                let _ = sender.send(SchedulerMessage::IoComplete { pid, result });
            });
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
// Message delivery & fiber wakeup
// ---------------------------------------------------------------------------

/// Wake a fiber, routing to the correct scheduler thread.
fn wake_fiber(
    sched: &mut PerScheduler,
    shared: &SharedState,
    pid: FiberId,
    fiber: Fiber,
) {
    // Check if this fiber is parked locally
    if let Some(_) = sched.parked_fibers.get(&pid) {
        // It's ours — will be handled in check_parked_wakeups
        // Just put on our ready queue directly
        sched.ready_queue.push_back(fiber);
        return;
    }

    // Check which scheduler owns this pid
    let target = {
        let pid_map = shared.pid_to_scheduler.lock().unwrap();
        pid_map.get(&pid).copied()
    };

    if let Some(target_id) = target {
        if target_id == sched.id {
            sched.ready_queue.push_back(fiber);
        } else {
            let _ = shared.scheduler_senders[target_id].send(SchedulerMessage::RunFiber(fiber));
        }
    } else {
        // Unknown pid — best effort, put on our queue
        sched.ready_queue.push_back(fiber);
    }
}

/// Deliver a message to a target pid's shared mailbox.
/// If the target is parked on receive, wake it.
fn deliver_message_shared(
    sched: &mut PerScheduler,
    shared: &SharedState,
    vm: &mut VM,
    target_pid: FiberId,
    msg: Value,
) {
    // Check if the target is alive
    {
        let alive = shared.alive.lock().unwrap();
        if !alive.contains(&target_pid) {
            return;
        }
    }

    // Push to shared mailbox
    {
        let mut mailboxes = shared.mailboxes.lock().unwrap();
        if let Some(mailbox) = mailboxes.get_mut(&target_pid) {
            mailbox.push_back(msg.clone());
        } else {
            return;
        }
    }

    // Check if the target is parked
    let parked_info = {
        let parked = shared.parked_pids.lock().unwrap();
        parked.get(&target_pid).copied()
    };

    if let Some(scheduler_id) = parked_info {
        if scheduler_id == sched.id {
            // Target is parked on our scheduler — try to wake locally
            if let Some(blocked) = sched.parked_fibers.remove(&target_pid) {
                {
                    let mut parked = shared.parked_pids.lock().unwrap();
                    parked.remove(&target_pid);
                }

                if let Some(ref predicate) = blocked.predicate {
                    if call_predicate_safe(vm, predicate, &msg) {
                        // Remove the message we just pushed
                        let mut mailboxes = shared.mailboxes.lock().unwrap();
                        if let Some(mailbox) = mailboxes.get_mut(&target_pid) {
                            mailbox.pop_back();
                        }
                        drop(mailboxes);
                        sched.ready_queue.push_back(Fiber::Suspended {
                            continuation: blocked.continuation,
                            resume_value: msg,
                            pid: target_pid,
                        });
                    } else {
                        // Re-park
                        sched.parked_fibers.insert(target_pid, blocked);
                        let mut parked = shared.parked_pids.lock().unwrap();
                        parked.insert(target_pid, sched.id);
                    }
                } else {
                    // Non-selective: pop first message
                    let mut mailboxes = shared.mailboxes.lock().unwrap();
                    let first = mailboxes
                        .get_mut(&target_pid)
                        .and_then(|mb| mb.pop_front())
                        .unwrap_or_else(Value::nil);
                    drop(mailboxes);
                    sched.ready_queue.push_back(Fiber::Suspended {
                        continuation: blocked.continuation,
                        resume_value: first,
                        pid: target_pid,
                    });
                }
            }
        } else {
            // Target parked on another scheduler — send MailboxNotify
            let _ = shared.scheduler_senders[scheduler_id]
                .send(SchedulerMessage::MailboxNotify { pid: target_pid });
        }
    }
}

/// Handle fiber exit with supervision (monitors, links).
fn handle_fiber_exit(
    sched: &mut PerScheduler,
    shared: &SharedState,
    vm: &mut VM,
    pid: FiberId,
    reason: ExitReason,
) {
    {
        let mut alive = shared.alive.lock().unwrap();
        alive.remove(&pid);
    }
    {
        let mut pid_map = shared.pid_to_scheduler.lock().unwrap();
        pid_map.remove(&pid);
    }

    let reason_str = match &reason {
        ExitReason::Normal => "normal".to_string(),
        ExitReason::Crash(msg) => msg.clone(),
    };

    // Notify monitors: deliver (Down pid reason) messages
    let monitor_set = {
        let mut monitors = shared.monitors.lock().unwrap();
        monitors.remove(&pid).unwrap_or_default()
    };
    {
        let alive = shared.alive.lock().unwrap();
        let live_monitors: Vec<_> = monitor_set.into_iter().filter(|m| alive.contains(m)).collect();
        drop(alive);
        for monitor_pid in live_monitors {
            let down_msg = Value::object(VMObject::AdtValue {
                tag: "Down".to_string(),
                fields: vec![
                    Value::object(VMObject::Pid(pid)),
                    Value::string(reason_str.as_str()),
                ],
            });
            deliver_message_shared(sched, shared, vm, monitor_pid, down_msg);
        }
    }

    // Process links
    let linked_set = {
        let mut links = shared.links.lock().unwrap();
        let linked = links.remove(&pid).unwrap_or_default();
        for linked_pid in &linked {
            if let Some(reverse) = links.get_mut(linked_pid) {
                reverse.remove(&pid);
            }
        }
        linked
    };

    if !matches!(reason, ExitReason::Normal) {
        let alive = shared.alive.lock().unwrap();
        let live_linked: Vec<_> = linked_set.into_iter().filter(|l| alive.contains(l)).collect();
        drop(alive);

        // Collect trap_exit status for all linked pids at once
        let trap_exit = shared.trap_exit.lock().unwrap();
        let trapped: Vec<_> = live_linked.iter()
            .map(|pid| (*pid, trap_exit.contains(pid)))
            .collect();
        drop(trap_exit);

        for (linked_pid, is_trapping) in trapped {
            if is_trapping {
                let exit_msg = Value::object(VMObject::AdtValue {
                    tag: "Exit".to_string(),
                    fields: vec![
                        Value::object(VMObject::Pid(pid)),
                        Value::string(reason_str.as_str()),
                    ],
                });
                deliver_message_shared(sched, shared, vm, linked_pid, exit_msg);
            } else {
                kill_fiber_shared(sched, shared, vm, linked_pid, &reason_str);
            }
        }
    }

    // Clean up
    {
        let mut mailboxes = shared.mailboxes.lock().unwrap();
        mailboxes.remove(&pid);
    }
    {
        let mut parked = shared.parked_pids.lock().unwrap();
        parked.remove(&pid);
    }
    sched.parked_fibers.remove(&pid);
}

fn kill_fiber_shared(
    sched: &mut PerScheduler,
    shared: &SharedState,
    vm: &mut VM,
    pid: FiberId,
    reason: &str,
) {
    // Remove from our ready queue if local
    sched.ready_queue.retain(|f| f.pid() != pid);
    sched.parked_fibers.remove(&pid);
    {
        let mut parked = shared.parked_pids.lock().unwrap();
        parked.remove(&pid);
    }
    handle_fiber_exit(sched, shared, vm, pid, ExitReason::Crash(reason.to_string()));
}

// ---------------------------------------------------------------------------
// Spawn distribution
// ---------------------------------------------------------------------------

fn pick_scheduler(sched: &mut PerScheduler, shared: &SharedState) -> usize {
    // Local bias: keep fiber local if queue is small
    if sched.ready_queue.len() < 4 {
        return sched.id;
    }
    // Round-robin across schedulers
    sched.spawn_counter += 1;
    sched.spawn_counter % shared.total_schedulers
}

// ---------------------------------------------------------------------------
// Inbox processing
// ---------------------------------------------------------------------------

fn drain_inbox(sched: &mut PerScheduler, shared: &SharedState, vm: &mut VM) {
    while let Ok(msg) = sched.inbox.try_recv() {
        match msg {
            SchedulerMessage::RunFiber(fiber) => {
                sched.ready_queue.push_back(fiber);
            }
            SchedulerMessage::IoComplete { pid, result } => {
                if let Some(cont) = sched.io_parked.remove(&pid) {
                    shared.pending_io.fetch_sub(1, Ordering::AcqRel);
                    match result {
                        Ok(value) => {
                            sched.ready_queue.push_back(Fiber::Suspended {
                                continuation: cont,
                                resume_value: value,
                                pid,
                            });
                        }
                        Err(msg) => {
                            handle_fiber_exit(sched, shared, vm, pid, ExitReason::Crash(msg));
                        }
                    }
                }
            }
            SchedulerMessage::WakeWithValue { pid, value } => {
                if let Some(blocked) = sched.parked_fibers.remove(&pid) {
                    {
                        let mut parked = shared.parked_pids.lock().unwrap();
                        parked.remove(&pid);
                    }
                    sched.ready_queue.push_back(Fiber::Suspended {
                        continuation: blocked.continuation,
                        resume_value: value,
                        pid,
                    });
                }
            }
            SchedulerMessage::MailboxNotify { pid } => {
                // A message was added to this pid's shared mailbox.
                // Check if the pid is parked on us and try to wake.
                if let Some(blocked) = sched.parked_fibers.remove(&pid) {
                    {
                        let mut parked = shared.parked_pids.lock().unwrap();
                        parked.remove(&pid);
                    }

                    if let Some(ref predicate) = blocked.predicate {
                        // Selective receive: scan mailbox
                        let mut mailboxes = shared.mailboxes.lock().unwrap();
                        if let Some(mailbox) = mailboxes.get_mut(&pid) {
                            let mut found = None;
                            for i in 0..mailbox.len() {
                                if call_predicate_safe(vm, predicate, &mailbox[i]) {
                                    found = Some(mailbox.remove(i).unwrap());
                                    break;
                                }
                            }
                            drop(mailboxes);
                            if let Some(msg) = found {
                                sched.ready_queue.push_back(Fiber::Suspended {
                                    continuation: blocked.continuation,
                                    resume_value: msg,
                                    pid,
                                });
                            } else {
                                // Re-park
                                sched.parked_fibers.insert(pid, blocked);
                                let mut parked = shared.parked_pids.lock().unwrap();
                                parked.insert(pid, sched.id);
                            }
                        } else {
                            drop(mailboxes);
                            // No mailbox — re-park
                            sched.parked_fibers.insert(pid, blocked);
                            let mut parked = shared.parked_pids.lock().unwrap();
                            parked.insert(pid, sched.id);
                        }
                    } else {
                        // Non-selective: pop first message
                        let mut mailboxes = shared.mailboxes.lock().unwrap();
                        let msg = mailboxes
                            .get_mut(&pid)
                            .and_then(|mb| mb.pop_front())
                            .unwrap_or_else(Value::nil);
                        drop(mailboxes);
                        sched.ready_queue.push_back(Fiber::Suspended {
                            continuation: blocked.continuation,
                            resume_value: msg,
                            pid,
                        });
                    }
                }
            }
            SchedulerMessage::Shutdown => {
                // Exit the loop
                return;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Work stealing
// ---------------------------------------------------------------------------

fn try_steal(sched: &mut PerScheduler) -> bool {
    let start = sched.spawn_counter % sched.stealers.len().max(1);
    for offset in 0..sched.stealers.len() {
        let idx = (start + offset) % sched.stealers.len();
        loop {
            match sched.stealers[idx].steal() {
                Steal::Success(fiber) => {
                    sched.ready_queue.push_back(fiber);
                    return true;
                }
                Steal::Retry => continue,
                Steal::Empty => break,
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Termination detection
// ---------------------------------------------------------------------------

fn check_termination(shared: &SharedState) -> bool {
    if shared.shutdown.load(Ordering::Acquire) {
        return true;
    }

    // Don't shut down while IO is pending
    if shared.pending_io.load(Ordering::Acquire) > 0 {
        return false;
    }

    // Check if all fibers are done
    let alive_count = {
        let alive = shared.alive.lock().unwrap();
        alive.len()
    };
    let parked_count = {
        let parked = shared.parked_pids.lock().unwrap();
        parked.len()
    };

    // If no alive fibers remain, we're done
    if alive_count == 0 {
        shared.shutdown.store(true, Ordering::Release);
        // Send shutdown to all
        for sender in &shared.scheduler_senders {
            let _ = sender.send(SchedulerMessage::Shutdown);
        }
        return true;
    }

    // If all alive fibers are parked (deadlock), we're done
    if alive_count == parked_count {
        shared.shutdown.store(true, Ordering::Release);
        for sender in &shared.scheduler_senders {
            let _ = sender.send(SchedulerMessage::Shutdown);
        }
        return true;
    }

    false
}

// ---------------------------------------------------------------------------
// Scheduler loop (per thread)
// ---------------------------------------------------------------------------

fn scheduler_loop(
    sched: &mut PerScheduler,
    shared: &SharedState,
    vm: &mut VM,
    is_main: bool,
    result_sender: Option<Sender<Result<Value, VMError>>>,
    main_body: Option<&Closure>,
) {
    let mut main_result = Value::nil();

    // If this is scheduler 0, run the main fiber first
    if is_main {
        if let Some(body) = main_body {
            let main_pid: FiberId = 0;
            sched.current_pid = main_pid;

            install_handler_frame(vm);
            match call_closure(vm, body, &[]) {
                Ok(result) => {
                    remove_handler_frame(vm);
                    main_result = result;
                    handle_fiber_exit(sched, shared, vm, main_pid, ExitReason::Normal);
                }
                Err(VMSignal::Effect(signal)) => {
                    if let Err(e) = handle_effect(sched, shared, vm, signal) {
                        handle_fiber_exit(sched, shared, vm, main_pid, ExitReason::Crash(e.message.clone()));
                    }
                }
                Err(VMSignal::Error(e)) => {
                    handle_fiber_exit(sched, shared, vm, main_pid, ExitReason::Crash(e.message.clone()));
                }
            }
        }
    }

    loop {
        // 1. Drain inbox
        drain_inbox(sched, shared, vm);

        // 2. Run all ready fibers
        while let Some(fiber) = sched.ready_queue.pop_front() {
            let pid = fiber.pid();
            sched.current_pid = pid;

            // Push to steal deque so other threads can steal
            // Actually, we run fibers from ready_queue directly.
            // Only push excess work to steal_worker for stealing.
            // For simplicity: run from ready_queue, populate steal_worker on spawn overflow.

            let saved_frame_count = vm.frame_count;
            let saved_stack_top = vm.stack_top;

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
                    handle_fiber_exit(sched, shared, vm, pid, ExitReason::Normal);
                }
                Err(VMSignal::Effect(signal)) => {
                    if let Err(e) = handle_effect(sched, shared, vm, signal) {
                        // Effect handling failed — treat as crash
                        vm.clear_stack_slots(saved_stack_top);
                        vm.frame_count = saved_frame_count;
                        vm.stack_top = saved_stack_top;
                        if !vm.handler_stack.is_empty() {
                            remove_handler_frame(vm);
                        }
                        handle_fiber_exit(sched, shared, vm, pid, ExitReason::Crash(e.message.clone()));
                    }
                }
                Err(VMSignal::Error(e)) => {
                    vm.clear_stack_slots(saved_stack_top);
                    vm.frame_count = saved_frame_count;
                    vm.stack_top = saved_stack_top;
                    if !vm.handler_stack.is_empty() {
                        remove_handler_frame(vm);
                    }
                    handle_fiber_exit(sched, shared, vm, pid, ExitReason::Crash(e.message.clone()));
                }
            }

            // Check for shutdown after each fiber
            if shared.shutdown.load(Ordering::Acquire) {
                if is_main {
                    if let Some(ref sender) = result_sender {
                        let _ = sender.send(Ok(main_result.clone()));
                    }
                }
                return;
            }
        }

        // 3. Try work stealing
        if try_steal(sched) {
            continue;
        }

        // 4. Check termination
        if check_termination(shared) {
            if is_main {
                if let Some(ref sender) = result_sender {
                    let _ = sender.send(Ok(main_result.clone()));
                }
            }
            return;
        }

        // 5. Idle — wait briefly for new work
        shared.idle_count.fetch_add(1, Ordering::AcqRel);

        match sched.inbox.recv_timeout(std::time::Duration::from_millis(1)) {
            Ok(msg) => {
                shared.idle_count.fetch_sub(1, Ordering::AcqRel);
                match msg {
                    SchedulerMessage::RunFiber(fiber) => {
                        sched.ready_queue.push_back(fiber);
                    }
                    SchedulerMessage::IoComplete { pid, result } => {
                        if let Some(cont) = sched.io_parked.remove(&pid) {
                            shared.pending_io.fetch_sub(1, Ordering::AcqRel);
                            match result {
                                Ok(value) => {
                                    sched.ready_queue.push_back(Fiber::Suspended {
                                        continuation: cont,
                                        resume_value: value,
                                        pid,
                                    });
                                }
                                Err(msg) => {
                                    handle_fiber_exit(sched, shared, vm, pid, ExitReason::Crash(msg));
                                }
                            }
                        }
                    }
                    SchedulerMessage::WakeWithValue { pid, value } => {
                        if let Some(blocked) = sched.parked_fibers.remove(&pid) {
                            {
                                let mut parked = shared.parked_pids.lock().unwrap();
                                parked.remove(&pid);
                            }
                            sched.ready_queue.push_back(Fiber::Suspended {
                                continuation: blocked.continuation,
                                resume_value: value,
                                pid,
                            });
                        }
                    }
                    SchedulerMessage::MailboxNotify { pid } => {
                        // Re-use inbox drain logic
                        if let Some(blocked) = sched.parked_fibers.remove(&pid) {
                            {
                                let mut parked = shared.parked_pids.lock().unwrap();
                                parked.remove(&pid);
                            }
                            if blocked.predicate.is_none() {
                                let mut mailboxes = shared.mailboxes.lock().unwrap();
                                let msg = mailboxes
                                    .get_mut(&pid)
                                    .and_then(|mb| mb.pop_front())
                                    .unwrap_or_else(Value::nil);
                                drop(mailboxes);
                                sched.ready_queue.push_back(Fiber::Suspended {
                                    continuation: blocked.continuation,
                                    resume_value: msg,
                                    pid,
                                });
                            } else {
                                let predicate = blocked.predicate.as_ref().unwrap();
                                let mut mailboxes = shared.mailboxes.lock().unwrap();
                                if let Some(mailbox) = mailboxes.get_mut(&pid) {
                                    let mut found = None;
                                    for i in 0..mailbox.len() {
                                        if call_predicate_safe(vm, predicate, &mailbox[i]) {
                                            found = Some(mailbox.remove(i).unwrap());
                                            break;
                                        }
                                    }
                                    drop(mailboxes);
                                    if let Some(msg) = found {
                                        sched.ready_queue.push_back(Fiber::Suspended {
                                            continuation: blocked.continuation,
                                            resume_value: msg,
                                            pid,
                                        });
                                    } else {
                                        sched.parked_fibers.insert(pid, blocked);
                                        let mut parked = shared.parked_pids.lock().unwrap();
                                        parked.insert(pid, sched.id);
                                    }
                                } else {
                                    drop(mailboxes);
                                    sched.parked_fibers.insert(pid, blocked);
                                    let mut parked = shared.parked_pids.lock().unwrap();
                                    parked.insert(pid, sched.id);
                                }
                            }
                        }
                    }
                    SchedulerMessage::Shutdown => {
                        shared.idle_count.fetch_sub(1, Ordering::AcqRel);
                        if is_main {
                            if let Some(ref sender) = result_sender {
                                let _ = sender.send(Ok(main_result.clone()));
                            }
                        }
                        return;
                    }
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                shared.idle_count.fetch_sub(1, Ordering::AcqRel);
                // Check termination again
                if check_termination(shared) {
                    if is_main {
                        if let Some(ref sender) = result_sender {
                            let _ = sender.send(Ok(main_result.clone()));
                        }
                    }
                    return;
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                shared.idle_count.fetch_sub(1, Ordering::AcqRel);
                if is_main {
                    if let Some(ref sender) = result_sender {
                        let _ = sender.send(Ok(main_result.clone()));
                    }
                }
                return;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Entry Point
// ---------------------------------------------------------------------------

/// Run a closure as the main fiber across N scheduler threads.
/// Each thread gets its own VM with cloned globals.
pub fn run_fibers_parallel(
    vm: &mut VM,
    body: &Closure,
    num_threads: usize,
) -> Result<Value, VMError> {
    let num_threads = num_threads.max(1);

    // Create per-scheduler channels
    let mut senders = Vec::with_capacity(num_threads);
    let mut receivers = Vec::with_capacity(num_threads);
    for _ in 0..num_threads {
        let (tx, rx) = crossbeam_channel::unbounded();
        senders.push(tx);
        receivers.push(rx);
    }

    // Create work-stealing deques
    let mut workers = Vec::with_capacity(num_threads);
    let mut all_stealers = Vec::new();
    for _ in 0..num_threads {
        let w = Worker::new_fifo();
        all_stealers.push(w.stealer());
        workers.push(w);
    }

    // Result channel
    let (result_tx, result_rx) = crossbeam_channel::bounded(1);

    // Create tokio runtime for async IO
    let tokio_rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("failed to create tokio runtime");

    // Transfer captured_output to shared state
    let captured = vm.captured_output.take();

    let shared = Arc::new(SharedState {
        next_pid: AtomicU64::new(1), // 0 is main
        alive: Mutex::new(HashSet::from([0])),
        pid_to_scheduler: Mutex::new(HashMap::from([(0, 0)])),
        mailboxes: Mutex::new(HashMap::from([(0, VecDeque::new())])),
        parked_pids: Mutex::new(HashMap::new()),
        channels: Mutex::new(HashMap::new()),
        next_channel_id: AtomicU64::new(0),
        monitors: Mutex::new(HashMap::new()),
        links: Mutex::new(HashMap::new()),
        trap_exit: Mutex::new(HashSet::new()),
        idle_count: AtomicUsize::new(0),
        total_schedulers: num_threads,
        shutdown: AtomicBool::new(false),
        scheduler_senders: senders,
        result_sender: Mutex::new(Some(result_tx)),
        captured_output: Arc::new(Mutex::new(captured)),
        pending_io: AtomicUsize::new(0),
        tokio_handle: tokio_rt.handle().clone(),
    });

    // Clone globals for each thread
    let globals = vm.globals.clone();
    let body_clone = body.clone();

    // Use thread::scope for safe borrowing
    std::thread::scope(|scope| {
        let mut handles = Vec::new();

        for i in 0..num_threads {
            let shared = Arc::clone(&shared);
            let globals = globals.clone();
            let body_clone = body_clone.clone();
            let receiver = receivers.remove(0);
            let worker = workers.remove(0);
            let stealers: Vec<Stealer<Fiber>> = all_stealers
                .iter()
                .enumerate()
                .filter(|(idx, _)| *idx != i)
                .map(|(_, s)| s.clone())
                .collect();
            let result_sender = if i == 0 {
                let sender = shared.result_sender.lock().unwrap().take();
                sender
            } else {
                None
            };

            let handle = scope.spawn(move || {
                let mut thread_vm = VM::new_with_globals(globals);

                let mut sched = PerScheduler {
                    id: i,
                    ready_queue: VecDeque::new(),
                    inbox: receiver,
                    parked_fibers: HashMap::new(),
                    current_pid: 0,
                    steal_worker: worker,
                    stealers,
                    spawn_counter: i, // Offset for better distribution
                    io_parked: HashMap::new(),
                };

                scheduler_loop(
                    &mut sched,
                    &shared,
                    &mut thread_vm,
                    i == 0,
                    result_sender,
                    if i == 0 { Some(&body_clone) } else { None },
                );
            });

            handles.push(handle);
        }

        // Wait for all threads
        for handle in handles {
            handle.join().unwrap();
        }
    });

    // Drop tokio runtime (waits for spawned tasks to finish)
    drop(tokio_rt);

    // Restore captured output
    let captured = shared.captured_output.lock().unwrap().take();
    vm.captured_output = captured;

    // Get result
    match result_rx.try_recv() {
        Ok(result) => result,
        Err(_) => Ok(Value::nil()),
    }
}
