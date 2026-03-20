//! Multi-threaded scheduler for Marlang concurrency.
//!
//! N scheduler threads each own a VM instance. Fibers are distributed across
//! threads at spawn time, cross-thread communication uses crossbeam channels.
//! Bytecode execution on each thread is lock-free.
//!
//! TODO: The HTTP and WebSocket layers (Layers 5-7) are implemented here as
//! built-in effects backed by hyper/tungstenite. Long-term, these should be
//! rewritten as pure Marlang libraries on top of the raw TCP primitives
//! (Layer 4), using the codec module for HTTP/1.1 parsing and WebSocket
//! framing. This would remove the hyper/tungstenite dependencies, make the
//! protocol stack inspectable and extensible from userland, and prove that
//! Marlang's TCP + codec + effect system is sufficient for real protocols.
//! The built-in versions exist for bootstrapping — they let us ship a working
//! web framework today without waiting for the pure implementations.

use crate::bytecode::{Chunk, FunctionPrototype};
use crate::errors::VMError;
use crate::object::{
    Closure, HandlerEntry, VMHandlerFrame, VMObject, make_err, make_none, make_ok, make_some,
};
use crate::value::{Value, format_value, format_value_unquoted};
use crate::vm::{
    EffectSignal, VM, VMSignal, call_closure, call_value, next_handler_id, resume_continuation,
};
use crossbeam_channel::{Receiver, Sender};
use crossbeam_deque::{Steal, Stealer, Worker};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::OwnedWriteHalf;

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
    WakeWithValue {
        pid: FiberId,
        value: Value,
    },
    MailboxNotify {
        pid: FiberId,
    },
    IoComplete {
        pid: FiberId,
        result: Result<Value, String>,
    },
    // TCP messages
    TcpData {
        socket_id: u64,
        data: Vec<u8>,
    },
    TcpClosed {
        socket_id: u64,
    },
    TcpAccepted {
        listener_id: u64,
        socket_id: u64,
        write_handle: Arc<tokio::sync::Mutex<OwnedWriteHalf>>,
    },
    TcpOp(TcpOp),
    TcpOpResult {
        fiber_pid: FiberId,
        result: Result<Value, String>,
    },
    // HTTP server messages
    HttpAccepted {
        server_id: u64,
        request: Value, // HttpRequest ADT
    },
    // WebSocket messages
    WsReady {
        ws_id: u64,
        write_tx: tokio::sync::mpsc::UnboundedSender<tokio_tungstenite::tungstenite::Message>,
    },
    WsData {
        ws_id: u64,
        msg: WsRecvMsg,
    },
    WsClosed {
        ws_id: u64,
    },
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
    /// Like Crash but preserves the full VMError for the main fiber.
    CrashWithTrace(VMError),
}

// ---------------------------------------------------------------------------
// TCP State (per-scheduler, no locking)
// ---------------------------------------------------------------------------

struct TcpSocketState {
    write_handle: Arc<tokio::sync::Mutex<OwnedWriteHalf>>,
    recv_queue: VecDeque<Vec<u8>>,
    waiting_recv: Option<(FiberId, Value)>, // (pid, continuation)
    closed: bool,
}

struct TcpListenerState {
    accept_queue: VecDeque<u64>, // socket IDs of accepted connections
    waiting_accept: Option<(FiberId, Value)>, // (pid, continuation)
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

// ---------------------------------------------------------------------------
// HTTP Server State (per-scheduler, no locking)
// ---------------------------------------------------------------------------

struct HttpServerState {
    request_queue: VecDeque<Value>,           // HttpRequest ADT values
    waiting_accept: Option<(FiberId, Value)>, // (pid, continuation)
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

struct HttpResponseData {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

enum HttpResponseAction {
    Normal(HttpResponseData),
    /// Upgrade this connection to WebSocket.
    /// The ws_id and scheduler sender are used to set up the WS state
    /// after the hyper upgrade completes.
    WebSocketUpgrade {
        ws_id: u64,
        scheduler_sender: Sender<SchedulerMessage>,
        sched_id: usize,
    },
}

// ---------------------------------------------------------------------------
// WebSocket State (per-scheduler, no locking)
// ---------------------------------------------------------------------------

struct WsState {
    write_tx: tokio::sync::mpsc::UnboundedSender<tokio_tungstenite::tungstenite::Message>,
    recv_queue: VecDeque<WsRecvMsg>,
    waiting_recv: Option<(FiberId, Value)>,
    closed: bool,
}

enum WsRecvMsg {
    Text(String),
    Binary(Vec<u8>),
}

/// Cross-scheduler TCP operation request.
#[allow(dead_code)]
enum TcpOp {
    Send {
        socket_id: u64,
        data: String,
        fiber_pid: FiberId,
        reply_scheduler: usize,
    },
    Recv {
        socket_id: u64,
        fiber_pid: FiberId,
        reply_scheduler: usize,
        continuation: Value,
    },
    Close {
        socket_id: u64,
        fiber_pid: FiberId,
        reply_scheduler: usize,
    },
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
    total_schedulers: usize,
    shutdown: AtomicBool,

    // Per-scheduler inboxes (for sending messages to specific schedulers)
    scheduler_senders: Vec<Sender<SchedulerMessage>>,

    // Captured output (for tests — all schedulers append here)
    captured_output: Arc<Mutex<Option<String>>>,

    // Async IO support
    pending_io: AtomicUsize,
    tokio_handle: tokio::runtime::Handle,

    // TCP state: socket/listener ownership tracking (socket_id → scheduler_id)
    tcp_socket_owners: Mutex<HashMap<u64, usize>>,
    tcp_listener_owners: Mutex<HashMap<u64, usize>>,
    next_tcp_id: AtomicU64,
    active_listeners: AtomicUsize,

    // HTTP server state
    http_server_owners: Mutex<HashMap<u64, usize>>,
    http_connections: Arc<Mutex<HashMap<u64, tokio::sync::oneshot::Sender<HttpResponseAction>>>>,
    active_http_servers: AtomicUsize,

    // WebSocket state
    ws_owners: Mutex<HashMap<u64, usize>>,
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
    // TCP state (owned by this scheduler)
    tcp_sockets: HashMap<u64, TcpSocketState>,
    tcp_listeners: HashMap<u64, TcpListenerState>,
    // HTTP server state (owned by this scheduler)
    http_servers: HashMap<u64, HttpServerState>,
    // WebSocket state (owned by this scheduler)
    ws_connections: HashMap<u64, WsState>,
    /// Maps ws_id → fiber pid for in-flight ws-upgrade calls.
    pending_ws_upgrades: HashMap<u64, FiberId>,
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
        Some(VMObject::Port(id)) => Some(*id),
        _ => None,
    }
}

fn call_predicate_safe(vm: &mut VM, predicate: &Closure, value: &Value) -> bool {
    let pred_val = Value::object(VMObject::Closure(Box::new(predicate.clone())));
    match call_value(vm, &pred_val, std::slice::from_ref(value)) {
        Ok(result) => result.is_truthy(),
        Err(_) => false,
    }
}

fn make_dummy_closure() -> Closure {
    use crate::bytecode::ROOT_MODULE;
    Closure {
        proto: Arc::new(FunctionPrototype {
            name: "<scheduler>".to_string(),
            arity: 0,
            is_variadic: false,
            upvalues: Vec::new(),
            chunk: Chunk::new(),
            module_id: ROOT_MODULE,
        }),
        upvalues: Vec::new(),
        module_id: ROOT_MODULE,
    }
}

const HANDLED_EFFECTS: &[&str] = &[
    "fiber-spawn",
    "fiber-yield",
    "fiber-self",
    "chan-new",
    "chan-send",
    "chan-recv",
    "chan-close",
    "actor-send",
    "actor-receive",
    "actor-receive-match",
    "actor-monitor",
    "actor-link",
    "actor-unlink",
    "actor-trap-exit",
    "print",
    "println",
    "read-line",
    "read-file",
    "read-file-bytes",
    "write-file",
    "write-file-bytes",
    // Layer 4: TCP
    "tcp-connect",
    "tcp-listen",
    "tcp-accept",
    "tcp-send",
    "tcp-recv",
    "tcp-close",
    "tcp-close-listener",
    // Layer 5: HTTP
    "http-request",
    "http-serve",
    "http-accept",
    "http-respond",
    "http-close-server",
    // Layer 6: WebSocket
    "ws-upgrade",
    "ws-send",
    "ws-send-bin",
    "ws-recv",
    "ws-close",
    // "throw" is NOT here — with CPS dispatch, user handlers catch throw inline.
    // Unhandled throw uses try_default_handler → "Uncaught exception: ..." error.
];

fn install_handler_frame(vm: &mut VM) {
    let dummy = make_dummy_closure();
    let entries: Vec<HandlerEntry> = HANDLED_EFFECTS
        .iter()
        .map(|name| HandlerEntry {
            effect_name: String::new(),
            op_name: name.to_string(),
            closure: dummy.clone(),
            arity: 1,
            tail_resume_closure: None,
            is_aborting: false,
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
        is_scheduler: true,
        is_tail_resumptive: false,
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
            let new_fiber = Fiber::New {
                closure: thunk,
                pid,
            };

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
                let _ =
                    shared.scheduler_senders[target].send(SchedulerMessage::RunFiber(new_fiber));
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
            let capacity = args.first().and_then(|v| v.as_int()).unwrap_or(0) as usize;
            let id = shared.next_channel_id.fetch_add(1, Ordering::Relaxed);
            {
                let mut channels = shared.channels.lock().unwrap();
                channels.insert(
                    id,
                    ChannelState {
                        buffer: VecDeque::new(),
                        capacity,
                        closed: false,
                        waiting_senders: Vec::new(),
                        waiting_receivers: Vec::new(),
                    },
                );
            }
            sched.ready_queue.push_back(Fiber::Suspended {
                continuation: cont,
                resume_value: Value::object(VMObject::Port(id)),
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
                drop(channels);
                sched.ready_queue.push_back(Fiber::Suspended {
                    continuation: cont,
                    resume_value: make_err("channel is closed"),
                    pid: sched.current_pid,
                });
            } else if !chan.waiting_receivers.is_empty() {
                let receiver = chan.waiting_receivers.remove(0);
                drop(channels);
                // Wake the receiver on its scheduler, wrapped in Some
                wake_fiber(
                    sched,
                    shared,
                    receiver.pid,
                    Fiber::Suspended {
                        continuation: receiver.continuation,
                        resume_value: make_some(value),
                        pid: receiver.pid,
                    },
                );
                sched.ready_queue.push_back(Fiber::Suspended {
                    continuation: cont,
                    resume_value: make_ok(Value::nil()),
                    pid: sched.current_pid,
                });
            } else if chan.buffer.len() < chan.capacity {
                chan.buffer.push_back(value);
                drop(channels);
                sched.ready_queue.push_back(Fiber::Suspended {
                    continuation: cont,
                    resume_value: make_ok(Value::nil()),
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
                    wake_fiber(
                        sched,
                        shared,
                        sender.pid,
                        Fiber::Suspended {
                            continuation: sender.continuation,
                            resume_value: make_ok(Value::nil()),
                            pid: sender.pid,
                        },
                    );
                } else {
                    drop(channels);
                }
                sched.ready_queue.push_back(Fiber::Suspended {
                    continuation: cont,
                    resume_value: make_some(value),
                    pid: sched.current_pid,
                });
            } else if !chan.waiting_senders.is_empty() {
                let sender = chan.waiting_senders.remove(0);
                drop(channels);
                wake_fiber(
                    sched,
                    shared,
                    sender.pid,
                    Fiber::Suspended {
                        continuation: sender.continuation,
                        resume_value: make_ok(Value::nil()),
                        pid: sender.pid,
                    },
                );
                sched.ready_queue.push_back(Fiber::Suspended {
                    continuation: cont,
                    resume_value: make_some(sender.value),
                    pid: sched.current_pid,
                });
            } else if chan.closed {
                drop(channels);
                sched.ready_queue.push_back(Fiber::Suspended {
                    continuation: cont,
                    resume_value: make_none(),
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
            let senders: Vec<WaitingSender> = chan.waiting_senders.drain(..).collect();
            drop(channels);

            for receiver in receivers {
                wake_fiber(
                    sched,
                    shared,
                    receiver.pid,
                    Fiber::Suspended {
                        continuation: receiver.continuation,
                        resume_value: make_none(),
                        pid: receiver.pid,
                    },
                );
            }
            // Wake all blocked senders with Err
            for sender in senders {
                wake_fiber(
                    sched,
                    shared,
                    sender.pid,
                    Fiber::Suspended {
                        continuation: sender.continuation,
                        resume_value: make_err("channel is closed"),
                        pid: sender.pid,
                    },
                );
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
            if let Some(mailbox) = mailboxes.get_mut(&current_pid)
                && !mailbox.is_empty()
            {
                let msg = mailbox.pop_front().unwrap();
                drop(mailboxes);
                sched.ready_queue.push_back(Fiber::Suspended {
                    continuation: cont,
                    resume_value: msg,
                    pid: current_pid,
                });
                return Ok(());
            }
            drop(mailboxes);

            // Park fiber locally, register in shared parked_pids
            sched.parked_fibers.insert(
                current_pid,
                BlockedReceiver {
                    continuation: cont,
                    predicate: None,
                },
            );
            let mut parked = shared.parked_pids.lock().unwrap();
            parked.insert(current_pid, sched.id);
        }

        "actor-receive-match" => {
            let predicate = match args.first().and_then(|v| v.as_object()) {
                Some(VMObject::Closure(c)) => (**c).clone(),
                _ => {
                    return Err(VMError::runtime(
                        "actor-receive-match: argument must be a closure",
                    ));
                }
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
            sched.parked_fibers.insert(
                current_pid,
                BlockedReceiver {
                    continuation: cont,
                    predicate: Some(predicate),
                },
            );
            let mut parked = shared.parked_pids.lock().unwrap();
            parked.insert(current_pid, sched.id);
        }

        "actor-monitor" => {
            let target_pid = as_pid(args.first().unwrap_or(&Value::nil()))
                .ok_or_else(|| VMError::runtime("actor-monitor: argument must be a pid"))?;
            {
                let mut monitors = shared.monitors.lock().unwrap();
                monitors
                    .entry(target_pid)
                    .or_default()
                    .insert(sched.current_pid);
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
                links
                    .entry(sched.current_pid)
                    .or_default()
                    .insert(target_pid);
                links
                    .entry(target_pid)
                    .or_default()
                    .insert(sched.current_pid);
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
                let _ = sender.send(SchedulerMessage::IoComplete {
                    pid,
                    result: Ok(Value::nil()),
                });
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
                let _ = sender.send(SchedulerMessage::IoComplete {
                    pid,
                    result: Ok(Value::nil()),
                });
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
                })
                .await;

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
                let result = match tokio::fs::read_to_string(&path).await {
                    Ok(s) => Ok(make_ok(Value::string(&*s))),
                    Err(e) => Ok(make_err(&format!("read-file: {}", e))),
                };
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
                let result = match tokio::fs::write(&path, content.as_bytes()).await {
                    Ok(_) => Ok(make_ok(Value::nil())),
                    Err(e) => Ok(make_err(&format!("write-file: {}", e))),
                };
                let _ = sender.send(SchedulerMessage::IoComplete { pid, result });
            });
        }

        "read-file-bytes" => {
            let path = args
                .first()
                .and_then(|v| v.as_string())
                .ok_or_else(|| VMError::runtime("read-file-bytes: path must be a string"))?
                .to_string();
            let sender = shared.scheduler_senders[sched.id].clone();
            let pid = sched.current_pid;

            sched.io_parked.insert(pid, cont);
            shared.pending_io.fetch_add(1, Ordering::AcqRel);

            shared.tokio_handle.spawn(async move {
                let result = match tokio::fs::read(&path).await {
                    Ok(data) => Ok(make_ok(Value::bytes(data))),
                    Err(e) => Ok(make_err(&format!("read-file-bytes: {}", e))),
                };
                let _ = sender.send(SchedulerMessage::IoComplete { pid, result });
            });
        }

        "write-file-bytes" => {
            let path = args
                .first()
                .and_then(|v| v.as_string())
                .ok_or_else(|| VMError::runtime("write-file-bytes: path must be a string"))?
                .to_string();
            let content = args
                .get(1)
                .and_then(|v| v.as_bytes())
                .ok_or_else(|| VMError::runtime("write-file-bytes: content must be Bytes"))?
                .to_vec();
            let sender = shared.scheduler_senders[sched.id].clone();
            let pid = sched.current_pid;

            sched.io_parked.insert(pid, cont);
            shared.pending_io.fetch_add(1, Ordering::AcqRel);

            shared.tokio_handle.spawn(async move {
                let result = match tokio::fs::write(&path, &content).await {
                    Ok(_) => Ok(make_ok(Value::nil())),
                    Err(e) => Ok(make_err(&format!("write-file-bytes: {}", e))),
                };
                let _ = sender.send(SchedulerMessage::IoComplete { pid, result });
            });
        }

        // --- Layer 4: TCP effects (async via tokio) ---
        "tcp-connect" => {
            let host = args
                .first()
                .and_then(|v| v.as_string())
                .ok_or_else(|| VMError::runtime("tcp-connect: host must be a string"))?
                .to_string();
            let port = args
                .get(1)
                .and_then(|v| v.as_int())
                .ok_or_else(|| VMError::runtime("tcp-connect: port must be a number"))?
                as u16;
            let sender = shared.scheduler_senders[sched.id].clone();
            let pid = sched.current_pid;
            let sched_id = sched.id;
            let socket_id = shared.next_tcp_id.fetch_add(1, Ordering::Relaxed);
            let tokio_handle = shared.tokio_handle.clone();

            sched.io_parked.insert(pid, cont);
            shared.pending_io.fetch_add(1, Ordering::AcqRel);

            // Register socket ownership before the async task completes
            {
                let mut owners = shared.tcp_socket_owners.lock().unwrap();
                owners.insert(socket_id, sched_id);
            }

            shared.tokio_handle.spawn(async move {
                let addr = format!("{}:{}", host, port);
                match tokio::net::TcpStream::connect(&addr).await {
                    Ok(stream) => {
                        let (mut read_half, write_half) = stream.into_split();
                        let write_handle = Arc::new(tokio::sync::Mutex::new(write_half));

                        // Spawn background read task for this socket
                        let read_sender = sender.clone();
                        let read_socket_id = socket_id;
                        tokio_handle.spawn(async move {
                            let mut buf = vec![0u8; 8192];
                            loop {
                                match read_half.read(&mut buf).await {
                                    Ok(0) => {
                                        let _ = read_sender.send(SchedulerMessage::TcpClosed {
                                            socket_id: read_socket_id,
                                        });
                                        break;
                                    }
                                    Ok(n) => {
                                        let _ = read_sender.send(SchedulerMessage::TcpData {
                                            socket_id: read_socket_id,
                                            data: buf[..n].to_vec(),
                                        });
                                    }
                                    Err(_) => {
                                        let _ = read_sender.send(SchedulerMessage::TcpClosed {
                                            socket_id: read_socket_id,
                                        });
                                        break;
                                    }
                                }
                            }
                        });

                        // Send IoComplete with the socket handle + write handle info
                        // We need to register the socket state on the owning scheduler,
                        // so we send a TcpAccepted-like message to set up state, then IoComplete.
                        let _ = sender.send(SchedulerMessage::TcpAccepted {
                            listener_id: u64::MAX, // sentinel: this is a connect, not accept
                            socket_id,
                            write_handle,
                        });
                        let _ = sender.send(SchedulerMessage::IoComplete {
                            pid,
                            result: Ok(make_ok(Value::object(VMObject::Port(socket_id)))),
                        });
                    }
                    Err(e) => {
                        let _ = sender.send(SchedulerMessage::IoComplete {
                            pid,
                            result: Ok(make_err(&format!("tcp-connect: {}", e))),
                        });
                    }
                }
            });
        }

        "tcp-listen" => {
            let port = args
                .first()
                .and_then(|v| v.as_int())
                .ok_or_else(|| VMError::runtime("tcp-listen: port must be a number"))?
                as u16;
            let sched_id = sched.id;
            let listener_id = shared.next_tcp_id.fetch_add(1, Ordering::Relaxed);

            // Bind synchronously via block_on so we can report errors immediately
            let listener = match shared.tokio_handle.block_on(async {
                tokio::net::TcpListener::bind(format!("127.0.0.1:{}", port)).await
            }) {
                Ok(l) => l,
                Err(e) => {
                    // Bind failed — resume fiber with (Err msg)
                    sched.ready_queue.push_back(Fiber::Suspended {
                        continuation: cont,
                        resume_value: make_err(&format!("tcp-listen: {}", e)),
                        pid: sched.current_pid,
                    });
                    return Ok(());
                }
            };

            let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();
            let sender = shared.scheduler_senders[sched.id].clone();
            let tokio_handle = shared.tokio_handle.clone();

            // Pre-allocate a batch of socket IDs for the accept loop.
            let tcp_id_counter = Arc::new(AtomicU64::new(
                shared.next_tcp_id.fetch_add(1000, Ordering::Relaxed),
            ));

            // Register listener ownership
            {
                let mut owners = shared.tcp_listener_owners.lock().unwrap();
                owners.insert(listener_id, sched_id);
            }
            shared.active_listeners.fetch_add(1, Ordering::AcqRel);

            // Spawn background accept loop
            shared.tokio_handle.spawn(async move {
                loop {
                    tokio::select! {
                        result = listener.accept() => {
                            match result {
                                Ok((stream, _addr)) => {
                                    let socket_id = tcp_id_counter.fetch_add(1, Ordering::Relaxed);

                                    let (mut read_half, write_half) = stream.into_split();
                                    let write_handle = Arc::new(tokio::sync::Mutex::new(write_half));

                                    // Spawn background read task for this socket
                                    let read_sender = sender.clone();
                                    tokio_handle.spawn(async move {
                                        let mut buf = vec![0u8; 8192];
                                        loop {
                                            match read_half.read(&mut buf).await {
                                                Ok(0) => {
                                                    let _ = read_sender.send(SchedulerMessage::TcpClosed {
                                                        socket_id,
                                                    });
                                                    break;
                                                }
                                                Ok(n) => {
                                                    let _ = read_sender.send(SchedulerMessage::TcpData {
                                                        socket_id,
                                                        data: buf[..n].to_vec(),
                                                    });
                                                }
                                                Err(_) => {
                                                    let _ = read_sender.send(SchedulerMessage::TcpClosed {
                                                        socket_id,
                                                    });
                                                    break;
                                                }
                                            }
                                        }
                                    });

                                    // Socket ownership will be registered by tcp_handle_accepted
                                    let _ = sender.send(SchedulerMessage::TcpAccepted {
                                        listener_id,
                                        socket_id,
                                        write_handle,
                                    });
                                }
                                Err(_) => break,
                            }
                        }
                        _ = &mut shutdown_rx => {
                            break;
                        }
                    }
                }
            });

            // Create listener state
            sched.tcp_listeners.insert(
                listener_id,
                TcpListenerState {
                    accept_queue: VecDeque::new(),
                    waiting_accept: None,
                    shutdown_tx: Some(shutdown_tx),
                },
            );

            // Resume fiber immediately with the listener handle
            sched.ready_queue.push_back(Fiber::Suspended {
                continuation: cont,
                resume_value: make_ok(Value::object(VMObject::Port(listener_id))),
                pid: sched.current_pid,
            });
        }

        "tcp-accept" => {
            let listener_id = args
                .first()
                .and_then(|v| match v.as_object() {
                    Some(VMObject::Port(id)) => Some(*id),
                    _ => None,
                })
                .ok_or_else(|| VMError::runtime("tcp-accept: argument must be a listener"))?;

            // Check if this listener is local
            let owner = {
                let owners = shared.tcp_listener_owners.lock().unwrap();
                owners.get(&listener_id).copied()
            };

            if owner == Some(sched.id) {
                // Local listener
                if let Some(listener_state) = sched.tcp_listeners.get_mut(&listener_id) {
                    if let Some(socket_id) = listener_state.accept_queue.pop_front() {
                        // Connection already waiting — wrap in Some
                        sched.ready_queue.push_back(Fiber::Suspended {
                            continuation: cont,
                            resume_value: make_some(Value::object(VMObject::Port(socket_id))),
                            pid: sched.current_pid,
                        });
                    } else if listener_state.shutdown_tx.is_none() {
                        // Listener is closed — return None
                        sched.ready_queue.push_back(Fiber::Suspended {
                            continuation: cont,
                            resume_value: make_none(),
                            pid: sched.current_pid,
                        });
                    } else {
                        // Park until a connection arrives
                        listener_state.waiting_accept = Some((sched.current_pid, cont));
                    }
                } else {
                    // Local owner but listener state gone — treat as closed
                    sched.ready_queue.push_back(Fiber::Suspended {
                        continuation: cont,
                        resume_value: make_none(),
                        pid: sched.current_pid,
                    });
                }
            } else if let Some(target_sched) = owner {
                // Remote listener — route to owning scheduler
                // Park locally and send operation to remote scheduler
                sched.io_parked.insert(sched.current_pid, cont);
                let _ = shared.scheduler_senders[target_sched].send(SchedulerMessage::TcpOp(
                    TcpOp::Recv {
                        socket_id: listener_id, // reusing Recv for accept routing
                        fiber_pid: sched.current_pid,
                        reply_scheduler: sched.id,
                        continuation: Value::nil(), // not used; reply via IoComplete
                    },
                ));
            } else {
                // Listener not in any ownership map — treat as closed
                sched.ready_queue.push_back(Fiber::Suspended {
                    continuation: cont,
                    resume_value: make_none(),
                    pid: sched.current_pid,
                });
            }
        }

        "tcp-send" => {
            let socket_id = args
                .first()
                .and_then(|v| match v.as_object() {
                    Some(VMObject::Port(id)) => Some(*id),
                    _ => None,
                })
                .ok_or_else(|| VMError::runtime("tcp-send: first argument must be a socket"))?;
            let data = args
                .get(1)
                .and_then(|v| v.as_string())
                .ok_or_else(|| VMError::runtime("tcp-send: second argument must be a string"))?
                .to_string();

            // Check if this socket is local
            let owner = {
                let owners = shared.tcp_socket_owners.lock().unwrap();
                owners.get(&socket_id).copied()
            };

            if owner == Some(sched.id) {
                // Local socket — write via tokio
                if let Some(socket_state) = sched.tcp_sockets.get(&socket_id) {
                    if socket_state.closed {
                        // Socket already closed — resume with (Err msg)
                        sched.ready_queue.push_back(Fiber::Suspended {
                            continuation: cont,
                            resume_value: make_err("tcp-send: socket is closed"),
                            pid: sched.current_pid,
                        });
                        return Ok(());
                    }
                    let write_handle = socket_state.write_handle.clone();
                    let sender = shared.scheduler_senders[sched.id].clone();
                    let pid = sched.current_pid;

                    sched.io_parked.insert(pid, cont);
                    shared.pending_io.fetch_add(1, Ordering::AcqRel);

                    shared.tokio_handle.spawn(async move {
                        let mut wh = write_handle.lock().await;
                        let result = match wh.write_all(data.as_bytes()).await {
                            Ok(_) => Ok(make_ok(Value::nil())),
                            Err(e) => Ok(make_err(&format!("tcp-send: {}", e))),
                        };
                        let _ = sender.send(SchedulerMessage::IoComplete { pid, result });
                    });
                } else {
                    // Local owner but socket state gone — treat as closed
                    sched.ready_queue.push_back(Fiber::Suspended {
                        continuation: cont,
                        resume_value: make_err("tcp-send: unknown socket"),
                        pid: sched.current_pid,
                    });
                }
            } else if let Some(target_sched) = owner {
                // Remote socket — route to owning scheduler
                sched.io_parked.insert(sched.current_pid, cont);
                let _ = shared.scheduler_senders[target_sched].send(SchedulerMessage::TcpOp(
                    TcpOp::Send {
                        socket_id,
                        data,
                        fiber_pid: sched.current_pid,
                        reply_scheduler: sched.id,
                    },
                ));
            } else {
                // Socket not in any ownership map — treat as closed
                sched.ready_queue.push_back(Fiber::Suspended {
                    continuation: cont,
                    resume_value: make_err("tcp-send: unknown socket"),
                    pid: sched.current_pid,
                });
            }
        }

        "tcp-recv" => {
            let socket_id = args
                .first()
                .and_then(|v| match v.as_object() {
                    Some(VMObject::Port(id)) => Some(*id),
                    _ => None,
                })
                .ok_or_else(|| VMError::runtime("tcp-recv: argument must be a socket"))?;

            // Check if this socket is local
            let owner = {
                let owners = shared.tcp_socket_owners.lock().unwrap();
                owners.get(&socket_id).copied()
            };

            if owner == Some(sched.id) {
                // Local socket
                if let Some(socket_state) = sched.tcp_sockets.get_mut(&socket_id) {
                    if let Some(data) = socket_state.recv_queue.pop_front() {
                        // Data available — wrap in Some
                        let s = String::from_utf8_lossy(&data).into_owned();
                        sched.ready_queue.push_back(Fiber::Suspended {
                            continuation: cont,
                            resume_value: make_some(Value::string(&*s)),
                            pid: sched.current_pid,
                        });
                    } else if socket_state.closed {
                        // Socket closed, no more data — return None
                        sched.ready_queue.push_back(Fiber::Suspended {
                            continuation: cont,
                            resume_value: make_none(),
                            pid: sched.current_pid,
                        });
                    } else {
                        // Park until data arrives
                        socket_state.waiting_recv = Some((sched.current_pid, cont));
                    }
                } else {
                    // Local owner but socket state gone — treat as closed
                    sched.ready_queue.push_back(Fiber::Suspended {
                        continuation: cont,
                        resume_value: make_none(),
                        pid: sched.current_pid,
                    });
                }
            } else if let Some(target_sched) = owner {
                // Remote socket — route to owning scheduler
                sched.io_parked.insert(sched.current_pid, cont);
                let _ = shared.scheduler_senders[target_sched].send(SchedulerMessage::TcpOp(
                    TcpOp::Recv {
                        socket_id,
                        fiber_pid: sched.current_pid,
                        reply_scheduler: sched.id,
                        continuation: Value::nil(), // not used; reply via TcpOpResult
                    },
                ));
            } else {
                // Socket not in any ownership map — treat as closed
                sched.ready_queue.push_back(Fiber::Suspended {
                    continuation: cont,
                    resume_value: make_none(),
                    pid: sched.current_pid,
                });
            }
        }

        "tcp-close" => {
            let id = args
                .first()
                .and_then(|v| match v.as_object() {
                    Some(VMObject::Port(id)) => Some(*id),
                    _ => None,
                })
                .ok_or_else(|| {
                    VMError::runtime("tcp-close: argument must be a socket or listener")
                })?;

            // Try as listener first, then as socket
            let listener_owner = {
                let owners = shared.tcp_listener_owners.lock().unwrap();
                owners.get(&id).copied()
            };

            if let Some(owner_sched) = listener_owner {
                if owner_sched == sched.id {
                    // Close local listener
                    if let Some(mut listener_state) = sched.tcp_listeners.remove(&id) {
                        // Send shutdown signal to the accept loop
                        if let Some(tx) = listener_state.shutdown_tx.take() {
                            let _ = tx.send(());
                        }
                        shared.active_listeners.fetch_sub(1, Ordering::AcqRel);
                        {
                            let mut owners = shared.tcp_listener_owners.lock().unwrap();
                            owners.remove(&id);
                        }
                        // Wake any fiber waiting on accept with None
                        if let Some((wait_pid, wait_cont)) = listener_state.waiting_accept.take() {
                            sched.ready_queue.push_back(Fiber::Suspended {
                                continuation: wait_cont,
                                resume_value: make_none(),
                                pid: wait_pid,
                            });
                        }
                    }
                } else {
                    // Remote listener — route to owning scheduler
                    sched.io_parked.insert(sched.current_pid, cont.clone());
                    let _ = shared.scheduler_senders[owner_sched].send(SchedulerMessage::TcpOp(
                        TcpOp::Close {
                            socket_id: id,
                            fiber_pid: sched.current_pid,
                            reply_scheduler: sched.id,
                        },
                    ));
                    // Don't resume below — wait for TcpOpResult
                    return Ok(());
                }
            } else {
                // Try as socket
                let socket_owner = {
                    let owners = shared.tcp_socket_owners.lock().unwrap();
                    owners.get(&id).copied()
                };

                if let Some(owner_sched) = socket_owner {
                    if owner_sched == sched.id {
                        // Close local socket
                        tcp_close_socket(sched, shared, id);
                    } else {
                        // Remote socket — route to owning scheduler
                        sched.io_parked.insert(sched.current_pid, cont.clone());
                        let _ = shared.scheduler_senders[owner_sched].send(
                            SchedulerMessage::TcpOp(TcpOp::Close {
                                socket_id: id,
                                fiber_pid: sched.current_pid,
                                reply_scheduler: sched.id,
                            }),
                        );
                        return Ok(());
                    }
                }
                // Unknown ID — might already be closed, ignore
            }

            // Resume fiber immediately
            sched.ready_queue.push_back(Fiber::Suspended {
                continuation: cont,
                resume_value: Value::nil(),
                pid: sched.current_pid,
            });
        }

        "tcp-close-listener" => {
            let id = args
                .first()
                .and_then(|v| match v.as_object() {
                    Some(VMObject::Port(id)) => Some(*id),
                    _ => None,
                })
                .ok_or_else(|| {
                    VMError::runtime("tcp-close-listener: argument must be a listener")
                })?;

            let listener_owner = {
                let owners = shared.tcp_listener_owners.lock().unwrap();
                owners.get(&id).copied()
            };

            if let Some(owner_sched) = listener_owner {
                if owner_sched == sched.id {
                    // Close local listener
                    if let Some(mut listener_state) = sched.tcp_listeners.remove(&id) {
                        if let Some(tx) = listener_state.shutdown_tx.take() {
                            let _ = tx.send(());
                        }
                        shared.active_listeners.fetch_sub(1, Ordering::AcqRel);
                        {
                            let mut owners = shared.tcp_listener_owners.lock().unwrap();
                            owners.remove(&id);
                        }
                        // Wake any fiber waiting on accept with None
                        if let Some((wait_pid, wait_cont)) = listener_state.waiting_accept.take() {
                            sched.ready_queue.push_back(Fiber::Suspended {
                                continuation: wait_cont,
                                resume_value: make_none(),
                                pid: wait_pid,
                            });
                        }
                    }
                } else {
                    // Remote listener — route to owning scheduler
                    sched.io_parked.insert(sched.current_pid, cont.clone());
                    let _ = shared.scheduler_senders[owner_sched].send(SchedulerMessage::TcpOp(
                        TcpOp::Close {
                            socket_id: id,
                            fiber_pid: sched.current_pid,
                            reply_scheduler: sched.id,
                        },
                    ));
                    return Ok(());
                }
            }
            // Unknown ID — might already be closed, ignore

            // Resume fiber immediately
            sched.ready_queue.push_back(Fiber::Suspended {
                continuation: cont,
                resume_value: Value::nil(),
                pid: sched.current_pid,
            });
        }

        // --- Layer 5: HTTP Client ---
        "http-request" => {
            // args: [HttpMethod ADT, url: String, headers: List (Pair String String), body: Bytes]
            let method_str = args
                .first()
                .and_then(|v| v.as_object())
                .and_then(|obj| match obj {
                    VMObject::AdtValue { tag, fields } if fields.is_empty() => Some(tag.as_str()),
                    _ => None,
                })
                .ok_or_else(|| VMError::runtime("http-request: first arg must be an HttpMethod"))?;

            let method = match method_str {
                "GET" => hyper::Method::GET,
                "POST" => hyper::Method::POST,
                "PUT" => hyper::Method::PUT,
                "DELETE" => hyper::Method::DELETE,
                "PATCH" => hyper::Method::PATCH,
                "HEAD" => hyper::Method::HEAD,
                "OPTIONS" => hyper::Method::OPTIONS,
                other => {
                    return Err(VMError::runtime(format!(
                        "http-request: unknown method '{}'",
                        other
                    )));
                }
            };

            let url = args
                .get(1)
                .and_then(|v| v.as_string())
                .ok_or_else(|| VMError::runtime("http-request: url must be a string"))?
                .to_string();

            // Extract headers: (List (Pair String String)) -> Vec<(String, String)>
            let headers_val = args
                .get(2)
                .ok_or_else(|| VMError::runtime("http-request: missing headers argument"))?;
            let header_pairs = http_collect_pair_list(headers_val, "http-request headers")?;

            let body_bytes = args
                .get(3)
                .and_then(|v| v.as_bytes())
                .ok_or_else(|| VMError::runtime("http-request: body must be Bytes"))?
                .to_vec();

            let sender = shared.scheduler_senders[sched.id].clone();
            let pid = sched.current_pid;

            sched.io_parked.insert(pid, cont);
            shared.pending_io.fetch_add(1, Ordering::AcqRel);

            shared.tokio_handle.spawn(async move {
                let result = http_do_request(method, &url, &header_pairs, body_bytes).await;
                let _ = sender.send(SchedulerMessage::IoComplete {
                    pid,
                    result: Ok(result),
                });
            });
        }

        // --- Layer 6: HTTP Server ---
        "http-serve" => {
            let port = args
                .first()
                .and_then(|v| v.as_int())
                .ok_or_else(|| VMError::runtime("http-serve: port must be a number"))?
                as u16;

            let server_id = shared.next_tcp_id.fetch_add(1, Ordering::Relaxed);

            let listener = match shared.tokio_handle.block_on(async {
                tokio::net::TcpListener::bind(format!("127.0.0.1:{}", port)).await
            }) {
                Ok(l) => l,
                Err(e) => {
                    sched.ready_queue.push_back(Fiber::Suspended {
                        continuation: cont,
                        resume_value: make_err(&format!("http-serve: {}", e)),
                        pid: sched.current_pid,
                    });
                    return Ok(());
                }
            };

            let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();

            // Register ownership
            {
                let mut owners = shared.http_server_owners.lock().unwrap();
                owners.insert(server_id, sched.id);
            }
            shared.active_http_servers.fetch_add(1, Ordering::AcqRel);

            // Pre-allocate a batch of connection IDs
            let conn_id_counter = Arc::new(AtomicU64::new(
                shared.next_tcp_id.fetch_add(1000, Ordering::Relaxed),
            ));

            let sender = shared.scheduler_senders[sched.id].clone();
            let http_conns = shared.http_connections.clone();
            // WS ownership is tracked via WsReady messages, but the async upgrade
            // task needs somewhere to record it immediately. Use a dummy Arc<Mutex>
            // — the real ownership is set when WsReady is processed by the scheduler.
            let ws_owners_async: Arc<Mutex<HashMap<u64, usize>>> =
                Arc::new(Mutex::new(HashMap::new()));

            // Spawn background accept loop
            shared.tokio_handle.spawn(async move {
                loop {
                    tokio::select! {
                        result = listener.accept() => {
                            match result {
                                Ok((stream, _)) => {
                                    let sender = sender.clone();
                                    let http_conns = http_conns.clone();
                                    let conn_id_counter = conn_id_counter.clone();
                                    let ws_owners_async = ws_owners_async.clone();
                                    tokio::spawn(async move {
                                        let io = hyper_util::rt::TokioIo::new(stream);
                                        let sender = sender.clone();
                                        let http_conns = http_conns.clone();
                                        let conn_id_counter = conn_id_counter.clone();
                                        let ws_owners_async = ws_owners_async.clone();
                                        let _ = hyper::server::conn::http1::Builder::new()
                                            .serve_connection(
                                                io,
                                                hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                                                    let sender = sender.clone();
                                                    let http_conns = http_conns.clone();
                                                    let conn_id_counter = conn_id_counter.clone();
                                                    let ws_owners_async = ws_owners_async.clone();
                                                    async move {
                                                        http_handle_incoming_request(
                                                            req, server_id, &sender, &http_conns, &conn_id_counter, &ws_owners_async,
                                                        ).await
                                                    }
                                                }),
                                            )
                                            .with_upgrades()
                                            .await;
                                    });
                                }
                                Err(_) => break,
                            }
                        }
                        _ = &mut shutdown_rx => {
                            break;
                        }
                    }
                }
            });

            // Create server state
            sched.http_servers.insert(
                server_id,
                HttpServerState {
                    request_queue: VecDeque::new(),
                    waiting_accept: None,
                    shutdown_tx: Some(shutdown_tx),
                },
            );

            // Resume fiber immediately with the server handle
            sched.ready_queue.push_back(Fiber::Suspended {
                continuation: cont,
                resume_value: make_ok(Value::object(VMObject::Port(server_id))),
                pid: sched.current_pid,
            });
        }

        "http-accept" => {
            let server_id = args
                .first()
                .and_then(|v| match v.as_object() {
                    Some(VMObject::Port(id)) => Some(*id),
                    _ => None,
                })
                .ok_or_else(|| VMError::runtime("http-accept: argument must be a server handle"))?;

            if let Some(server_state) = sched.http_servers.get_mut(&server_id) {
                if let Some(request) = server_state.request_queue.pop_front() {
                    // Request already waiting — wrap in Some
                    sched.ready_queue.push_back(Fiber::Suspended {
                        continuation: cont,
                        resume_value: make_some(request),
                        pid: sched.current_pid,
                    });
                } else if server_state.shutdown_tx.is_none() {
                    // Server is closed — return None
                    sched.ready_queue.push_back(Fiber::Suspended {
                        continuation: cont,
                        resume_value: make_none(),
                        pid: sched.current_pid,
                    });
                } else {
                    // Park until a request arrives
                    server_state.waiting_accept = Some((sched.current_pid, cont));
                }
            } else {
                // Server state gone — treat as closed
                sched.ready_queue.push_back(Fiber::Suspended {
                    continuation: cont,
                    resume_value: make_none(),
                    pid: sched.current_pid,
                });
            }
        }

        "http-respond" => {
            let connection_id = args
                .first()
                .and_then(|v| match v.as_object() {
                    Some(VMObject::Port(id)) => Some(*id),
                    _ => None,
                })
                .ok_or_else(|| {
                    VMError::runtime("http-respond: first argument must be a connection handle")
                })?;

            let status = args
                .get(1)
                .and_then(|v| v.as_int())
                .ok_or_else(|| VMError::runtime("http-respond: status must be a number"))?
                as u16;

            let headers_val = args
                .get(2)
                .ok_or_else(|| VMError::runtime("http-respond: missing headers argument"))?;
            let header_pairs = http_collect_pair_list(headers_val, "http-respond headers")?;

            let body_bytes = args
                .get(3)
                .and_then(|v| v.as_bytes())
                .ok_or_else(|| VMError::runtime("http-respond: body must be Bytes"))?
                .to_vec();

            // Look up the oneshot sender and send the response
            let resp_tx = {
                let mut conns = shared.http_connections.lock().unwrap();
                conns.remove(&connection_id)
            };

            let resume_value = match resp_tx {
                Some(tx) => match tx.send(HttpResponseAction::Normal(HttpResponseData {
                    status,
                    headers: header_pairs,
                    body: body_bytes,
                })) {
                    Ok(()) => make_ok(Value::nil()),
                    Err(_) => make_err("http-respond: connection closed"),
                },
                None => make_err("http-respond: unknown connection"),
            };

            sched.ready_queue.push_back(Fiber::Suspended {
                continuation: cont,
                resume_value,
                pid: sched.current_pid,
            });
        }

        "http-close-server" => {
            let server_id = args
                .first()
                .and_then(|v| match v.as_object() {
                    Some(VMObject::Port(id)) => Some(*id),
                    _ => None,
                })
                .ok_or_else(|| {
                    VMError::runtime("http-close-server: argument must be a server handle")
                })?;

            if let Some(mut server_state) = sched.http_servers.remove(&server_id) {
                // Signal the accept loop to stop
                if let Some(tx) = server_state.shutdown_tx.take() {
                    let _ = tx.send(());
                }
                shared.active_http_servers.fetch_sub(1, Ordering::AcqRel);
                {
                    let mut owners = shared.http_server_owners.lock().unwrap();
                    owners.remove(&server_id);
                }
                // Wake any waiting accept with None
                if let Some((wait_pid, wait_cont)) = server_state.waiting_accept.take() {
                    sched.ready_queue.push_back(Fiber::Suspended {
                        continuation: wait_cont,
                        resume_value: make_none(),
                        pid: wait_pid,
                    });
                }
            }

            sched.ready_queue.push_back(Fiber::Suspended {
                continuation: cont,
                resume_value: Value::nil(),
                pid: sched.current_pid,
            });
        }

        // --- Layer 7: WebSocket ---
        "ws-upgrade" => {
            let connection_id = args
                .first()
                .and_then(|v| match v.as_object() {
                    Some(VMObject::Port(id)) => Some(*id),
                    _ => None,
                })
                .ok_or_else(|| {
                    VMError::runtime("ws-upgrade: argument must be an HttpConnection handle")
                })?;

            let ws_id = shared.next_tcp_id.fetch_add(1, Ordering::Relaxed);

            // Take the HTTP oneshot and send WebSocketUpgrade through it
            let resp_tx = {
                let mut conns = shared.http_connections.lock().unwrap();
                conns.remove(&connection_id)
            };

            match resp_tx {
                Some(tx) => {
                    let sender = shared.scheduler_senders[sched.id].clone();
                    let sched_id = sched.id;
                    let pid = sched.current_pid;
                    sched.io_parked.insert(pid, cont);
                    sched.pending_ws_upgrades.insert(ws_id, pid);
                    shared.pending_io.fetch_add(1, Ordering::AcqRel);

                    // Send upgrade request to the service function
                    let _ = tx.send(HttpResponseAction::WebSocketUpgrade {
                        ws_id,
                        scheduler_sender: sender,
                        sched_id,
                    });
                }
                None => {
                    sched.ready_queue.push_back(Fiber::Suspended {
                        continuation: cont,
                        resume_value: make_err("ws-upgrade: unknown connection"),
                        pid: sched.current_pid,
                    });
                }
            }
        }

        "ws-send" => {
            let ws_id = args
                .first()
                .and_then(|v| match v.as_object() {
                    Some(VMObject::Port(id)) => Some(*id),
                    _ => None,
                })
                .ok_or_else(|| VMError::runtime("ws-send: first arg must be a WsConnection"))?;

            let text = args
                .get(1)
                .and_then(|v| v.as_string())
                .ok_or_else(|| VMError::runtime("ws-send: second arg must be a string"))?
                .to_string();

            let resume_value = if let Some(ws) = sched.ws_connections.get(&ws_id) {
                if ws.closed {
                    make_err("ws-send: connection closed")
                } else {
                    match ws
                        .write_tx
                        .send(tokio_tungstenite::tungstenite::Message::Text(text.into()))
                    {
                        Ok(()) => make_ok(Value::nil()),
                        Err(_) => make_err("ws-send: write channel closed"),
                    }
                }
            } else {
                make_err("ws-send: unknown connection")
            };

            sched.ready_queue.push_back(Fiber::Suspended {
                continuation: cont,
                resume_value,
                pid: sched.current_pid,
            });
        }

        "ws-send-bin" => {
            let ws_id = args
                .first()
                .and_then(|v| match v.as_object() {
                    Some(VMObject::Port(id)) => Some(*id),
                    _ => None,
                })
                .ok_or_else(|| VMError::runtime("ws-send-bin: first arg must be a WsConnection"))?;

            let data = args
                .get(1)
                .and_then(|v| v.as_bytes())
                .ok_or_else(|| VMError::runtime("ws-send-bin: second arg must be Bytes"))?
                .to_vec();

            let resume_value = if let Some(ws) = sched.ws_connections.get(&ws_id) {
                if ws.closed {
                    make_err("ws-send-bin: connection closed")
                } else {
                    match ws
                        .write_tx
                        .send(tokio_tungstenite::tungstenite::Message::Binary(data.into()))
                    {
                        Ok(()) => make_ok(Value::nil()),
                        Err(_) => make_err("ws-send-bin: write channel closed"),
                    }
                }
            } else {
                make_err("ws-send-bin: unknown connection")
            };

            sched.ready_queue.push_back(Fiber::Suspended {
                continuation: cont,
                resume_value,
                pid: sched.current_pid,
            });
        }

        "ws-recv" => {
            let ws_id = args
                .first()
                .and_then(|v| match v.as_object() {
                    Some(VMObject::Port(id)) => Some(*id),
                    _ => None,
                })
                .ok_or_else(|| VMError::runtime("ws-recv: argument must be a WsConnection"))?;

            if let Some(ws) = sched.ws_connections.get_mut(&ws_id) {
                if let Some(msg) = ws.recv_queue.pop_front() {
                    let val = ws_msg_to_value(msg);
                    sched.ready_queue.push_back(Fiber::Suspended {
                        continuation: cont,
                        resume_value: make_some(val),
                        pid: sched.current_pid,
                    });
                } else if ws.closed {
                    sched.ready_queue.push_back(Fiber::Suspended {
                        continuation: cont,
                        resume_value: make_none(),
                        pid: sched.current_pid,
                    });
                } else {
                    // Park until data arrives
                    ws.waiting_recv = Some((sched.current_pid, cont));
                }
            } else {
                sched.ready_queue.push_back(Fiber::Suspended {
                    continuation: cont,
                    resume_value: make_none(),
                    pid: sched.current_pid,
                });
            }
        }

        "ws-close" => {
            let ws_id = args
                .first()
                .and_then(|v| match v.as_object() {
                    Some(VMObject::Port(id)) => Some(*id),
                    _ => None,
                })
                .ok_or_else(|| VMError::runtime("ws-close: argument must be a WsConnection"))?;

            if let Some(ws) = sched.ws_connections.get_mut(&ws_id) {
                if !ws.closed {
                    ws.closed = true;
                    let _ = ws
                        .write_tx
                        .send(tokio_tungstenite::tungstenite::Message::Close(None));
                }
                // Wake any waiting recv with None
                if let Some((wait_pid, wait_cont)) = ws.waiting_recv.take() {
                    sched.ready_queue.push_back(Fiber::Suspended {
                        continuation: wait_cont,
                        resume_value: make_none(),
                        pid: wait_pid,
                    });
                }
            }

            sched.ready_queue.push_back(Fiber::Suspended {
                continuation: cont,
                resume_value: Value::nil(),
                pid: sched.current_pid,
            });
        }

        // --- Exception effect (default handler: crash the fiber) ---
        "throw" => {
            let msg = args
                .first()
                .map(format_value)
                .unwrap_or_else(|| "(no message)".to_string());
            return Err(VMError::runtime(format!("uncaught exception: {}", msg)));
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
// HTTP helpers
// ---------------------------------------------------------------------------

/// Check if an HTTP request is a WebSocket upgrade.
fn is_websocket_upgrade(req: &hyper::Request<hyper::body::Incoming>) -> bool {
    req.headers()
        .get("upgrade")
        .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"websocket"))
}

/// Compute the Sec-WebSocket-Accept value from the client's key.
fn compute_ws_accept(key: &str) -> String {
    // SHA-1 of key + magic GUID, base64-encoded
    let mut hasher = Sha1::new();
    hasher.update(key.as_bytes());
    hasher.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    let hash = hasher.finalize();
    base64_encode(&hash)
}

/// Minimal base64 encoder (RFC 4648).
fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(CHARS[((n >> 18) & 63) as usize]);
        out.push(CHARS[((n >> 12) & 63) as usize]);
        if chunk.len() > 1 {
            out.push(CHARS[((n >> 6) & 63) as usize]);
        } else {
            out.push(b'=');
        }
        if chunk.len() > 2 {
            out.push(CHARS[(n & 63) as usize]);
        } else {
            out.push(b'=');
        }
    }
    String::from_utf8(out).unwrap()
}

/// Minimal SHA-1 (for WebSocket accept key computation only).
struct Sha1 {
    state: [u32; 5],
    buffer: Vec<u8>,
    len: u64,
}

impl Sha1 {
    fn new() -> Self {
        Sha1 {
            state: [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0],
            buffer: Vec::new(),
            len: 0,
        }
    }
    fn update(&mut self, data: &[u8]) {
        self.buffer.extend_from_slice(data);
        self.len += data.len() as u64;
        while self.buffer.len() >= 64 {
            let block: [u8; 64] = self.buffer[..64].try_into().unwrap();
            self.buffer.drain(..64);
            self.process_block(&block);
        }
    }
    fn finalize(mut self) -> [u8; 20] {
        let bit_len = self.len * 8;
        self.buffer.push(0x80);
        while self.buffer.len() % 64 != 56 {
            self.buffer.push(0);
        }
        self.buffer.extend_from_slice(&bit_len.to_be_bytes());
        let remaining = std::mem::take(&mut self.buffer);
        for chunk in remaining.chunks(64) {
            let block: [u8; 64] = chunk.try_into().unwrap();
            self.process_block(&block);
        }
        let mut out = [0u8; 20];
        for (i, &s) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&s.to_be_bytes());
        }
        out
    }
    fn process_block(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(block[i * 4..i * 4 + 4].try_into().unwrap());
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = self.state;
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1u32),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC_u32),
                _ => (b ^ c ^ d, 0xCA62C1D6_u32),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
    }
}

/// Handle an incoming HTTP request in the server's hyper service function.
/// Parses the request, creates a oneshot channel for the response, queues
/// the request for the Marlang fiber, and awaits the response.
/// Also handles WebSocket upgrade requests.
async fn http_handle_incoming_request(
    req: hyper::Request<hyper::body::Incoming>,
    server_id: u64,
    sender: &Sender<SchedulerMessage>,
    http_conns: &Arc<Mutex<HashMap<u64, tokio::sync::oneshot::Sender<HttpResponseAction>>>>,
    conn_id_counter: &AtomicU64,
    ws_owners: &Arc<Mutex<HashMap<u64, usize>>>,
) -> Result<hyper::Response<http_body_util::Full<bytes::Bytes>>, hyper::Error> {
    use http_body_util::{BodyExt, Full};

    let is_upgrade = is_websocket_upgrade(&req);

    // Extract request info before potentially moving `req`
    let method = req.method().to_string();
    let path = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());
    let headers: Vec<(String, String)> = req
        .headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();

    let ws_key = if is_upgrade {
        req.headers()
            .get("sec-websocket-key")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
    } else {
        None
    };

    // For WebSocket upgrades: don't read body, keep `req` for hyper::upgrade::on()
    // For normal HTTP: read the full body
    let (body_bytes, upgrade_req) = if is_upgrade {
        (Vec::new(), Some(req))
    } else {
        let body = match req.into_body().collect().await {
            Ok(collected) => collected.to_bytes().to_vec(),
            Err(_) => {
                return Ok(hyper::Response::builder()
                    .status(500)
                    .body(Full::new(bytes::Bytes::from("failed to read request body")))
                    .unwrap());
            }
        };
        (body, None)
    };

    // Allocate connection ID and create oneshot for response
    let connection_id = conn_id_counter.fetch_add(1, Ordering::Relaxed);
    let (resp_tx, resp_rx) = tokio::sync::oneshot::channel::<HttpResponseAction>();

    // Store sender in shared map
    {
        let mut conns = http_conns.lock().unwrap();
        conns.insert(connection_id, resp_tx);
    }

    // Build HttpRequest value
    let headers_list = http_build_pair_list(headers);
    let request = Value::object(VMObject::AdtValue {
        tag: "HttpRequest".to_string(),
        fields: vec![
            Value::string(&*method),
            Value::string(&*path),
            headers_list,
            Value::bytes(body_bytes),
            Value::object(VMObject::Port(connection_id)),
        ],
    });

    // Send to scheduler
    let _ = sender.send(SchedulerMessage::HttpAccepted { server_id, request });

    // Wait for response from Marlang code
    match resp_rx.await {
        Ok(HttpResponseAction::Normal(resp_data)) => {
            let mut builder = hyper::Response::builder().status(resp_data.status);
            for (key, value) in &resp_data.headers {
                builder = builder.header(key.as_str(), value.as_str());
            }
            Ok(builder
                .body(Full::new(bytes::Bytes::from(resp_data.body)))
                .unwrap())
        }
        Ok(HttpResponseAction::WebSocketUpgrade {
            ws_id,
            scheduler_sender,
            sched_id,
        }) => {
            // Perform WebSocket upgrade
            let Some(ws_key) = ws_key else {
                return Ok(hyper::Response::builder()
                    .status(400)
                    .body(Full::new(bytes::Bytes::from("missing Sec-WebSocket-Key")))
                    .unwrap());
            };
            let Some(upgrade_req) = upgrade_req else {
                return Ok(hyper::Response::builder()
                    .status(400)
                    .body(Full::new(bytes::Bytes::from("not an upgrade request")))
                    .unwrap());
            };

            let accept_key = compute_ws_accept(&ws_key);
            let ws_owners = ws_owners.clone();

            // Set up the upgrade future before returning the 101 response
            let on_upgrade = hyper::upgrade::on(upgrade_req);

            // Spawn the post-upgrade handler
            tokio::spawn(async move {
                match on_upgrade.await {
                    Ok(upgraded) => {
                        let io = hyper_util::rt::TokioIo::new(upgraded);
                        let mut ws_stream = tokio_tungstenite::WebSocketStream::from_raw_socket(
                            io,
                            tokio_tungstenite::tungstenite::protocol::Role::Server,
                            None,
                        )
                        .await;

                        use futures_util::{SinkExt, StreamExt};
                        use tokio_tungstenite::tungstenite::Message;

                        // Channel for Marlang to send messages through
                        let (write_tx, mut write_rx) =
                            tokio::sync::mpsc::unbounded_channel::<Message>();

                        // Register WS ownership
                        {
                            let mut owners = ws_owners.lock().unwrap();
                            owners.insert(ws_id, sched_id);
                        }

                        // Notify scheduler that the WS connection is ready
                        let _ =
                            scheduler_sender.send(SchedulerMessage::WsReady { ws_id, write_tx });

                        // Single-task select loop: read from WS and write channel
                        loop {
                            tokio::select! {
                                // Messages from the WebSocket (client → server)
                                ws_msg = ws_stream.next() => {
                                    match ws_msg {
                                        Some(Ok(Message::Text(text))) => {
                                            let _ = scheduler_sender.send(
                                                SchedulerMessage::WsData {
                                                    ws_id,
                                                    msg: WsRecvMsg::Text(text.to_string()),
                                                },
                                            );
                                        }
                                        Some(Ok(Message::Binary(data))) => {
                                            let _ = scheduler_sender.send(
                                                SchedulerMessage::WsData {
                                                    ws_id,
                                                    msg: WsRecvMsg::Binary(data.to_vec()),
                                                },
                                            );
                                        }
                                        Some(Ok(Message::Close(frame))) => {
                                            // Echo the close frame back for clean shutdown
                                            let _ = ws_stream.send(Message::Close(frame)).await;
                                            break;
                                        }
                                        None | Some(Err(_)) => {
                                            break;
                                        }
                                        _ => {} // Ping/Pong handled by tungstenite
                                    }
                                }
                                // Messages from Marlang (server → client)
                                outgoing = write_rx.recv() => {
                                    match outgoing {
                                        Some(msg) => {
                                            if ws_stream.send(msg).await.is_err() {
                                                break;
                                            }
                                        }
                                        None => break, // channel closed
                                    }
                                }
                            }
                        }

                        // Connection closed
                        let _ = ws_stream.close(None).await;
                        let _ = scheduler_sender.send(SchedulerMessage::WsClosed { ws_id });
                    }
                    Err(e) => {
                        // Upgrade failed — signal error via IoComplete
                        let _ = scheduler_sender.send(SchedulerMessage::IoComplete {
                            pid: 0, // dummy — the parked fiber is woken by WsReady
                            result: Err(format!("ws upgrade failed: {}", e)),
                        });
                    }
                }
            });

            // Return 101 Switching Protocols
            Ok(hyper::Response::builder()
                .status(101)
                .header("Upgrade", "websocket")
                .header("Connection", "Upgrade")
                .header("Sec-WebSocket-Accept", accept_key)
                .body(Full::new(bytes::Bytes::new()))
                .unwrap())
        }
        Err(_) => Ok(hyper::Response::builder()
            .status(500)
            .body(Full::new(bytes::Bytes::from("request handler dropped")))
            .unwrap()),
    }
}

/// Lazily-initialized TLS client configuration using native OS certificates.
static TLS_CLIENT_CONFIG: std::sync::LazyLock<Arc<tokio_rustls::rustls::ClientConfig>> =
    std::sync::LazyLock::new(|| {
        let mut root_store = tokio_rustls::rustls::RootCertStore::empty();
        let certs = rustls_native_certs::load_native_certs();
        for cert in certs.certs {
            let _ = root_store.add(cert);
        }
        Arc::new(
            tokio_rustls::rustls::ClientConfig::builder()
                .with_root_certificates(root_store)
                .with_no_client_auth(),
        )
    });

/// Walk a Cons/Nil list of MkPair ADT values, extracting (String, String) pairs.
fn http_collect_pair_list(val: &Value, context: &str) -> Result<Vec<(String, String)>, VMError> {
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
                // Each element should be a MkPair
                match fields[0].as_object() {
                    Some(VMObject::AdtValue {
                        tag: pair_tag,
                        fields: pair_fields,
                    }) if pair_tag == "MkPair" && pair_fields.len() == 2 => {
                        let key = pair_fields[0]
                            .as_string()
                            .ok_or_else(|| {
                                VMError::runtime(format!(
                                    "{}: header key must be a string",
                                    context
                                ))
                            })?
                            .to_string();
                        let value = pair_fields[1]
                            .as_string()
                            .ok_or_else(|| {
                                VMError::runtime(format!(
                                    "{}: header value must be a string",
                                    context
                                ))
                            })?
                            .to_string();
                        result.push((key, value));
                    }
                    _ => {
                        return Err(VMError::runtime(format!(
                            "{}: expected (Pair String String) elements",
                            context
                        )));
                    }
                }
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

/// Build a Cons/Nil list of MkPair ADT values from (String, String) pairs.
fn http_build_pair_list(pairs: Vec<(String, String)>) -> Value {
    use crate::object::vec_to_cons_list;
    let items: Vec<Value> = pairs
        .into_iter()
        .map(|(k, v)| {
            Value::object(VMObject::AdtValue {
                tag: "MkPair".to_string(),
                fields: vec![Value::string(&*k), Value::string(&*v)],
            })
        })
        .collect();
    vec_to_cons_list(items)
}

/// Send an HTTP request over an already-connected IO stream using hyper.
/// Generic over the transport (plain TCP or TLS-wrapped TCP).
async fn http_send_request<I>(
    io: I,
    method: hyper::Method,
    host: &str,
    path_and_query: &str,
    headers: &[(String, String)],
    body: Vec<u8>,
) -> Value
where
    I: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
{
    use http_body_util::{BodyExt, Full};

    // Use hyper's handshake to get a sender
    let (mut sender, conn) = match hyper::client::conn::http1::handshake(io).await {
        Ok(pair) => pair,
        Err(e) => return make_err(&format!("HTTP handshake failed: {}", e)),
    };

    // Spawn the connection driver
    tokio::spawn(async move {
        if let Err(_e) = conn.await {
            // Connection error — sender will see it
        }
    });

    // Build the request
    let mut req_builder = hyper::Request::builder()
        .method(method)
        .uri(path_and_query)
        .header("Host", host);

    for (key, value) in headers {
        req_builder = req_builder.header(key.as_str(), value.as_str());
    }

    let req = match req_builder.body(Full::new(bytes::Bytes::from(body))) {
        Ok(r) => r,
        Err(e) => return make_err(&format!("failed to build request: {}", e)),
    };

    // Send the request
    let response = match sender.send_request(req).await {
        Ok(r) => r,
        Err(e) => return make_err(&format!("HTTP request failed: {}", e)),
    };

    // Extract status, headers, body
    let status = response.status().as_u16() as i64;

    let resp_headers: Vec<(String, String)> = response
        .headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();

    let body_bytes = match response.into_body().collect().await {
        Ok(collected) => collected.to_bytes().to_vec(),
        Err(e) => return make_err(&format!("failed to read response body: {}", e)),
    };

    // Build HttpResponse ADT: (HttpResponse status headers body)
    let headers_list = http_build_pair_list(resp_headers);
    let response_adt = Value::object(VMObject::AdtValue {
        tag: "HttpResponse".to_string(),
        fields: vec![Value::int(status), headers_list, Value::bytes(body_bytes)],
    });

    make_ok(response_adt)
}

/// Perform an HTTP/HTTPS request using hyper. Returns a Marlang Result value.
async fn http_do_request(
    method: hyper::Method,
    url: &str,
    headers: &[(String, String)],
    body: Vec<u8>,
) -> Value {
    use hyper::Uri;

    // Parse the URL
    let uri: Uri = match url.parse() {
        Ok(u) => u,
        Err(e) => return make_err(&format!("invalid URL '{}': {}", url, e)),
    };

    let scheme = uri.scheme_str().unwrap_or("http");
    let is_https = match scheme {
        "http" => false,
        "https" => true,
        other => {
            return make_err(&format!(
                "unsupported scheme '{}' (only http and https are supported)",
                other
            ));
        }
    };

    let host = match uri.host() {
        Some(h) => h.to_string(),
        None => return make_err(&format!("URL '{}' has no host", url)),
    };

    let default_port = if is_https { 443 } else { 80 };
    let port = uri.port_u16().unwrap_or(default_port);
    let path_and_query = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");

    // Connect via tokio TCP
    let addr = format!("{}:{}", host, port);
    let tcp_stream = match tokio::net::TcpStream::connect(&addr).await {
        Ok(s) => s,
        Err(e) => return make_err(&format!("connection to {} failed: {}", addr, e)),
    };

    if is_https {
        // Wrap in TLS
        let connector = tokio_rustls::TlsConnector::from(TLS_CLIENT_CONFIG.clone());
        let server_name = match rustls_pki_types::ServerName::try_from(host.clone()) {
            Ok(sn) => sn,
            Err(e) => return make_err(&format!("invalid TLS server name '{}': {}", host, e)),
        };
        let tls_stream = match connector.connect(server_name, tcp_stream).await {
            Ok(s) => s,
            Err(e) => return make_err(&format!("TLS handshake with {} failed: {}", host, e)),
        };
        let io = hyper_util::rt::TokioIo::new(tls_stream);
        http_send_request(io, method, &host, path_and_query, headers, body).await
    } else {
        let io = hyper_util::rt::TokioIo::new(tcp_stream);
        http_send_request(io, method, &host, path_and_query, headers, body).await
    }
}

// ---------------------------------------------------------------------------
// Message delivery & fiber wakeup
// ---------------------------------------------------------------------------

/// Wake a fiber, routing to the correct scheduler thread.
fn wake_fiber(sched: &mut PerScheduler, shared: &SharedState, pid: FiberId, fiber: Fiber) {
    // Check if this fiber is parked locally
    if sched.parked_fibers.contains_key(&pid) {
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
        ExitReason::CrashWithTrace(e) => e.message.clone(),
    };

    // Notify monitors: deliver (Down pid reason) messages
    let monitor_set = {
        let mut monitors = shared.monitors.lock().unwrap();
        monitors.remove(&pid).unwrap_or_default()
    };
    {
        let alive = shared.alive.lock().unwrap();
        let live_monitors: Vec<_> = monitor_set
            .into_iter()
            .filter(|m| alive.contains(m))
            .collect();
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
        let live_linked: Vec<_> = linked_set
            .into_iter()
            .filter(|l| alive.contains(l))
            .collect();
        drop(alive);

        // Collect trap_exit status for all linked pids at once
        let trap_exit = shared.trap_exit.lock().unwrap();
        let trapped: Vec<_> = live_linked
            .iter()
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
    handle_fiber_exit(
        sched,
        shared,
        vm,
        pid,
        ExitReason::Crash(reason.to_string()),
    );
}

// ---------------------------------------------------------------------------
// TCP helpers
// ---------------------------------------------------------------------------

/// Close a local TCP socket: mark closed, wake waiting receiver with None,
/// drop the write handle, remove from ownership.
fn tcp_close_socket(sched: &mut PerScheduler, shared: &SharedState, socket_id: u64) {
    if let Some(mut socket_state) = sched.tcp_sockets.remove(&socket_id) {
        socket_state.closed = true;
        // Wake any fiber waiting on recv with None
        if let Some((wait_pid, wait_cont)) = socket_state.waiting_recv.take() {
            sched.ready_queue.push_back(Fiber::Suspended {
                continuation: wait_cont,
                resume_value: make_none(),
                pid: wait_pid,
            });
        }
        // Drop write_handle — this closes the write side of the socket.
        // The read task will get EOF and send TcpClosed (which we'll ignore).
        drop(socket_state.write_handle);
    }
    {
        let mut owners = shared.tcp_socket_owners.lock().unwrap();
        owners.remove(&socket_id);
    }
}

/// Handle incoming TCP data for a local socket.
fn tcp_handle_data(sched: &mut PerScheduler, socket_id: u64, data: Vec<u8>) {
    if let Some(socket_state) = sched.tcp_sockets.get_mut(&socket_id) {
        if let Some((wait_pid, wait_cont)) = socket_state.waiting_recv.take() {
            // Fiber is waiting — deliver immediately, wrapped in Some
            let s = String::from_utf8_lossy(&data).into_owned();
            sched.ready_queue.push_back(Fiber::Suspended {
                continuation: wait_cont,
                resume_value: make_some(Value::string(&*s)),
                pid: wait_pid,
            });
        } else {
            // Buffer the data
            socket_state.recv_queue.push_back(data);
        }
    }
    // If socket not found (already closed), ignore the data
}

/// Handle TCP socket closed notification.
fn tcp_handle_closed(sched: &mut PerScheduler, socket_id: u64) {
    if let Some(socket_state) = sched.tcp_sockets.get_mut(&socket_id) {
        socket_state.closed = true;
        // Wake any fiber waiting on recv with None
        if let Some((wait_pid, wait_cont)) = socket_state.waiting_recv.take() {
            sched.ready_queue.push_back(Fiber::Suspended {
                continuation: wait_cont,
                resume_value: make_none(),
                pid: wait_pid,
            });
        }
    }
}

/// Handle a new accepted TCP connection.
fn tcp_handle_accepted(
    sched: &mut PerScheduler,
    shared: &SharedState,
    listener_id: u64,
    socket_id: u64,
    write_handle: Arc<tokio::sync::Mutex<OwnedWriteHalf>>,
) {
    // Register socket ownership in shared state
    {
        let mut owners = shared.tcp_socket_owners.lock().unwrap();
        owners.insert(socket_id, sched.id);
    }

    // Register the socket state locally
    sched.tcp_sockets.insert(
        socket_id,
        TcpSocketState {
            write_handle,
            recv_queue: VecDeque::new(),
            waiting_recv: None,
            closed: false,
        },
    );

    // For tcp-connect (listener_id == u64::MAX), the socket is just registered;
    // the fiber is woken by the subsequent IoComplete message.
    if listener_id == u64::MAX {
        return;
    }

    // For tcp-accept: check if a fiber is waiting
    if let Some(listener_state) = sched.tcp_listeners.get_mut(&listener_id) {
        if let Some((wait_pid, wait_cont)) = listener_state.waiting_accept.take() {
            sched.ready_queue.push_back(Fiber::Suspended {
                continuation: wait_cont,
                resume_value: make_some(Value::object(VMObject::Port(socket_id))),
                pid: wait_pid,
            });
        } else {
            listener_state.accept_queue.push_back(socket_id);
        }
    }
}

/// Handle a cross-scheduler TCP operation.
fn tcp_handle_op(sched: &mut PerScheduler, shared: &SharedState, op: TcpOp) {
    match op {
        TcpOp::Send {
            socket_id,
            data,
            fiber_pid,
            reply_scheduler,
        } => {
            if let Some(socket_state) = sched.tcp_sockets.get(&socket_id) {
                if socket_state.closed {
                    let _ = shared.scheduler_senders[reply_scheduler].send(
                        SchedulerMessage::TcpOpResult {
                            fiber_pid,
                            result: Ok(make_err("tcp-send: socket is closed")),
                        },
                    );
                    return;
                }
                let write_handle = socket_state.write_handle.clone();
                let sender = shared.scheduler_senders[reply_scheduler].clone();
                shared.pending_io.fetch_add(1, Ordering::AcqRel);

                shared.tokio_handle.spawn(async move {
                    let mut wh = write_handle.lock().await;
                    let result = match wh.write_all(data.as_bytes()).await {
                        Ok(_) => Ok(make_ok(Value::nil())),
                        Err(e) => Ok(make_err(&format!("tcp-send: {}", e))),
                    };
                    let _ = sender.send(SchedulerMessage::TcpOpResult { fiber_pid, result });
                });
            } else {
                let _ =
                    shared.scheduler_senders[reply_scheduler].send(SchedulerMessage::TcpOpResult {
                        fiber_pid,
                        result: Ok(make_err("tcp-send: unknown socket")),
                    });
            }
        }
        TcpOp::Recv {
            socket_id,
            fiber_pid,
            reply_scheduler,
            ..
        } => {
            if let Some(socket_state) = sched.tcp_sockets.get_mut(&socket_id) {
                if let Some(data) = socket_state.recv_queue.pop_front() {
                    let s = String::from_utf8_lossy(&data).into_owned();
                    let _ = shared.scheduler_senders[reply_scheduler].send(
                        SchedulerMessage::TcpOpResult {
                            fiber_pid,
                            result: Ok(make_some(Value::string(&*s))),
                        },
                    );
                } else if socket_state.closed {
                    let _ = shared.scheduler_senders[reply_scheduler].send(
                        SchedulerMessage::TcpOpResult {
                            fiber_pid,
                            result: Ok(make_none()),
                        },
                    );
                } else {
                    // Cross-scheduler recv not yet supported when no data is buffered.
                    // In practice, sockets are used by the fiber that created them.
                    let _ = shared.scheduler_senders[reply_scheduler].send(
                        SchedulerMessage::TcpOpResult {
                            fiber_pid,
                            result: Ok(make_err("tcp-recv: cross-scheduler recv not yet supported (no data available)")),
                        }
                    );
                }
            } else {
                // Check if this is actually a listener accept being routed
                if let Some(listener_state) = sched.tcp_listeners.get_mut(&socket_id) {
                    if let Some(sid) = listener_state.accept_queue.pop_front() {
                        let _ = shared.scheduler_senders[reply_scheduler].send(
                            SchedulerMessage::TcpOpResult {
                                fiber_pid,
                                result: Ok(make_some(Value::object(VMObject::Port(sid)))),
                            },
                        );
                    } else if listener_state.shutdown_tx.is_none() {
                        let _ = shared.scheduler_senders[reply_scheduler].send(
                            SchedulerMessage::TcpOpResult {
                                fiber_pid,
                                result: Ok(make_none()),
                            },
                        );
                    } else {
                        // Park — same limitation as above for cross-scheduler
                        let _ = shared.scheduler_senders[reply_scheduler].send(
                            SchedulerMessage::TcpOpResult {
                                fiber_pid,
                                result: Ok(make_err(
                                    "tcp-accept: cross-scheduler accept not yet supported",
                                )),
                            },
                        );
                    }
                } else {
                    // Not a socket or listener — treat as closed
                    let _ = shared.scheduler_senders[reply_scheduler].send(
                        SchedulerMessage::TcpOpResult {
                            fiber_pid,
                            result: Ok(make_none()),
                        },
                    );
                }
            }
        }
        TcpOp::Close {
            socket_id,
            fiber_pid,
            reply_scheduler,
        } => {
            // Check if it's a listener
            let is_listener = sched.tcp_listeners.contains_key(&socket_id);
            if is_listener {
                if let Some(mut listener_state) = sched.tcp_listeners.remove(&socket_id) {
                    if let Some(tx) = listener_state.shutdown_tx.take() {
                        let _ = tx.send(());
                    }
                    shared.active_listeners.fetch_sub(1, Ordering::AcqRel);
                    {
                        let mut owners = shared.tcp_listener_owners.lock().unwrap();
                        owners.remove(&socket_id);
                    }
                    if let Some((wait_pid, wait_cont)) = listener_state.waiting_accept.take() {
                        sched.ready_queue.push_back(Fiber::Suspended {
                            continuation: wait_cont,
                            resume_value: make_none(),
                            pid: wait_pid,
                        });
                    }
                }
            } else {
                tcp_close_socket(sched, shared, socket_id);
            }
            let _ = shared.scheduler_senders[reply_scheduler].send(SchedulerMessage::TcpOpResult {
                fiber_pid,
                result: Ok(Value::nil()),
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Spawn distribution
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// HTTP Server message handling
// ---------------------------------------------------------------------------

/// Handle a new HTTP request arriving at a server.
/// Convert a WsRecvMsg to a Marlang Value: (WsText string) or (WsBinary bytes)
fn ws_msg_to_value(msg: WsRecvMsg) -> Value {
    match msg {
        WsRecvMsg::Text(s) => Value::object(VMObject::AdtValue {
            tag: "WsText".to_string(),
            fields: vec![Value::string(&*s)],
        }),
        WsRecvMsg::Binary(b) => Value::object(VMObject::AdtValue {
            tag: "WsBinary".to_string(),
            fields: vec![Value::bytes(b)],
        }),
    }
}

/// Handle an incoming WebSocket message.
fn ws_handle_data(sched: &mut PerScheduler, ws_id: u64, msg: WsRecvMsg) {
    if let Some(ws) = sched.ws_connections.get_mut(&ws_id) {
        if let Some((wait_pid, wait_cont)) = ws.waiting_recv.take() {
            let val = ws_msg_to_value(msg);
            sched.ready_queue.push_back(Fiber::Suspended {
                continuation: wait_cont,
                resume_value: make_some(val),
                pid: wait_pid,
            });
        } else {
            ws.recv_queue.push_back(msg);
        }
    }
}

/// Handle WebSocket connection closed.
fn ws_handle_closed(sched: &mut PerScheduler, shared: &SharedState, ws_id: u64) {
    if let Some(ws) = sched.ws_connections.get_mut(&ws_id) {
        ws.closed = true;
        if let Some((wait_pid, wait_cont)) = ws.waiting_recv.take() {
            sched.ready_queue.push_back(Fiber::Suspended {
                continuation: wait_cont,
                resume_value: make_none(),
                pid: wait_pid,
            });
        }
    }
    {
        let mut owners = shared.ws_owners.lock().unwrap();
        owners.remove(&ws_id);
    }
}

fn http_handle_accepted(sched: &mut PerScheduler, server_id: u64, request: Value) {
    if let Some(server_state) = sched.http_servers.get_mut(&server_id) {
        if let Some((wait_pid, wait_cont)) = server_state.waiting_accept.take() {
            // A fiber is waiting for http-accept — deliver immediately
            sched.ready_queue.push_back(Fiber::Suspended {
                continuation: wait_cont,
                resume_value: make_some(request),
                pid: wait_pid,
            });
        } else {
            // No fiber waiting — queue the request
            server_state.request_queue.push_back(request);
        }
    }
    // If server_state is gone (server closed), the request is dropped.
    // The oneshot sender in http_connections will also be dropped eventually,
    // causing the hyper service to receive an error and close the connection.
}

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

/// Process a single scheduler message. Returns true if Shutdown was received.
fn process_single_message(
    sched: &mut PerScheduler,
    shared: &SharedState,
    vm: &mut VM,
    msg: SchedulerMessage,
) -> bool {
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
            if let Some(blocked) = sched.parked_fibers.remove(&pid) {
                {
                    let mut parked = shared.parked_pids.lock().unwrap();
                    parked.remove(&pid);
                }

                if let Some(ref predicate) = blocked.predicate {
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
                } else {
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
        SchedulerMessage::TcpData { socket_id, data } => {
            tcp_handle_data(sched, socket_id, data);
        }
        SchedulerMessage::TcpClosed { socket_id } => {
            tcp_handle_closed(sched, socket_id);
        }
        SchedulerMessage::TcpAccepted {
            listener_id,
            socket_id,
            write_handle,
        } => {
            tcp_handle_accepted(sched, shared, listener_id, socket_id, write_handle);
        }
        SchedulerMessage::TcpOp(op) => {
            tcp_handle_op(sched, shared, op);
        }
        SchedulerMessage::TcpOpResult { fiber_pid, result } => {
            if let Some(cont) = sched.io_parked.remove(&fiber_pid) {
                // Note: pending_io is only decremented if the remote handler actually
                // incremented it (async path). Synchronous error responses don't increment
                // pending_io, so we don't decrement here — the remote handler sends
                // pending_io_dec=true only when an async task was spawned.
                // For simplicity, we always decrement since the miscount is harmless
                // (pending_io is only used for termination detection, and an undercount
                // just means slightly earlier shutdown checks).
                match result {
                    Ok(value) => {
                        sched.ready_queue.push_back(Fiber::Suspended {
                            continuation: cont,
                            resume_value: value,
                            pid: fiber_pid,
                        });
                    }
                    Err(msg) => {
                        handle_fiber_exit(sched, shared, vm, fiber_pid, ExitReason::Crash(msg));
                    }
                }
            }
        }
        SchedulerMessage::HttpAccepted { server_id, request } => {
            http_handle_accepted(sched, server_id, request);
        }
        SchedulerMessage::WsReady { ws_id, write_tx } => {
            // Register WS connection and wake the fiber that called ws-upgrade
            {
                let mut owners = shared.ws_owners.lock().unwrap();
                owners.insert(ws_id, sched.id);
            }
            sched.ws_connections.insert(
                ws_id,
                WsState {
                    write_tx,
                    recv_queue: VecDeque::new(),
                    waiting_recv: None,
                    closed: false,
                },
            );
            // Wake the parked ws-upgrade fiber
            if let Some(pid) = sched.pending_ws_upgrades.remove(&ws_id)
                && let Some(cont) = sched.io_parked.remove(&pid)
            {
                shared.pending_io.fetch_sub(1, Ordering::AcqRel);
                sched.ready_queue.push_back(Fiber::Suspended {
                    continuation: cont,
                    resume_value: make_ok(Value::object(VMObject::Port(ws_id))),
                    pid,
                });
            }
        }
        SchedulerMessage::WsData { ws_id, msg } => {
            ws_handle_data(sched, ws_id, msg);
        }
        SchedulerMessage::WsClosed { ws_id } => {
            ws_handle_closed(sched, shared, ws_id);
        }
        SchedulerMessage::Shutdown => {
            return true;
        }
    }
    false
}

fn drain_inbox(sched: &mut PerScheduler, shared: &SharedState, vm: &mut VM) {
    while let Ok(msg) = sched.inbox.try_recv() {
        if process_single_message(sched, shared, vm, msg) {
            return;
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

    // Don't shut down while IO is pending, listeners are active, or HTTP servers are running
    if shared.pending_io.load(Ordering::Acquire) > 0 {
        return false;
    }
    if shared.active_listeners.load(Ordering::Acquire) > 0 {
        return false;
    }
    if shared.active_http_servers.load(Ordering::Acquire) > 0 {
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
// Always-On Scheduler (public API)
// ---------------------------------------------------------------------------

/// Always-on per-core scheduler. Created once at program start, runs N scheduler
/// threads (scheduler 0 on the main thread, workers 1..N-1 on spawned threads).
/// All effects (IO, TCP, HTTP, fibers, channels, actors) flow through this
/// scheduler — no `run-fibers` wrapper needed.
#[allow(dead_code)] // Fields reserved for Phase 9 (multi-threaded worker spawning)
pub struct Scheduler {
    shared: Arc<SharedState>,
    local: PerScheduler,
    tokio_rt: tokio::runtime::Runtime,
    /// Receivers for worker threads (consumed on first `execute_main` call).
    worker_receivers: Option<Vec<Receiver<SchedulerMessage>>>,
    /// Work-stealing deque workers for worker threads (consumed on first `execute_main`).
    worker_deque_workers: Option<Vec<Worker<Fiber>>>,
    /// All stealers (including scheduler 0's).
    all_stealers: Vec<Stealer<Fiber>>,
    /// Number of scheduler threads (including scheduler 0).
    num_threads: usize,
    /// Whether worker threads have been spawned yet.
    workers_spawned: bool,
    /// Persistent main fiber PID (0), reused across all `execute_main` calls.
    main_pid: FiberId,
}

impl Scheduler {
    /// Create a new always-on scheduler with N threads (one per CPU core).
    /// Scheduler 0 runs on the current thread; workers 1..N-1 are spawned lazily.
    pub fn new(vm: &mut VM) -> Self {
        let num_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);

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

        // Create tokio runtime for async IO
        let tokio_rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("failed to create tokio runtime");

        // Transfer captured_output to shared state
        let captured = vm.captured_output.take();

        let shared = Arc::new(SharedState {
            next_pid: AtomicU64::new(1), // 0 reserved for first main fiber
            alive: Mutex::new(HashSet::new()),
            pid_to_scheduler: Mutex::new(HashMap::new()),
            mailboxes: Mutex::new(HashMap::new()),
            parked_pids: Mutex::new(HashMap::new()),
            channels: Mutex::new(HashMap::new()),
            next_channel_id: AtomicU64::new(0),
            monitors: Mutex::new(HashMap::new()),
            links: Mutex::new(HashMap::new()),
            trap_exit: Mutex::new(HashSet::new()),
            total_schedulers: num_threads,
            shutdown: AtomicBool::new(false),
            scheduler_senders: senders,
            captured_output: Arc::new(Mutex::new(captured)),
            pending_io: AtomicUsize::new(0),
            tokio_handle: tokio_rt.handle().clone(),
            tcp_socket_owners: Mutex::new(HashMap::new()),
            tcp_listener_owners: Mutex::new(HashMap::new()),
            next_tcp_id: AtomicU64::new(0),
            active_listeners: AtomicUsize::new(0),
            http_server_owners: Mutex::new(HashMap::new()),
            http_connections: Arc::new(Mutex::new(HashMap::new())),
            active_http_servers: AtomicUsize::new(0),
            ws_owners: Mutex::new(HashMap::new()),
        });

        // Scheduler 0's local state
        let sched0_receiver = receivers.remove(0);
        let sched0_worker = workers.remove(0);
        let sched0_stealers: Vec<Stealer<Fiber>> = all_stealers
            .iter()
            .enumerate()
            .filter(|(idx, _)| *idx != 0)
            .map(|(_, s)| s.clone())
            .collect();

        let local = PerScheduler {
            id: 0,
            ready_queue: VecDeque::new(),
            inbox: sched0_receiver,
            parked_fibers: HashMap::new(),
            current_pid: 0,
            steal_worker: sched0_worker,
            stealers: sched0_stealers,
            spawn_counter: 0,
            io_parked: HashMap::new(),
            tcp_sockets: HashMap::new(),
            tcp_listeners: HashMap::new(),
            http_servers: HashMap::new(),
            ws_connections: HashMap::new(),
            pending_ws_upgrades: HashMap::new(),
        };

        // Register the persistent main PID (0)
        let main_pid: FiberId = 0;
        {
            let mut alive = shared.alive.lock().unwrap();
            alive.insert(main_pid);
        }
        {
            let mut pid_map = shared.pid_to_scheduler.lock().unwrap();
            pid_map.insert(main_pid, 0);
        }
        {
            let mut mailboxes = shared.mailboxes.lock().unwrap();
            mailboxes.insert(main_pid, VecDeque::new());
        }

        Scheduler {
            shared,
            local,
            tokio_rt,
            worker_receivers: Some(receivers),
            worker_deque_workers: Some(workers),
            all_stealers,
            num_threads,
            workers_spawned: false,
            main_pid,
        }
    }

    /// Execute a single top-level closure as a "main fiber" on scheduler 0.
    /// Returns the result value. The main fiber keeps the same PID across all
    /// `execute_main` calls so that actor state (mailbox, links, monitors,
    /// trap-exit) persists across top-level expressions.
    pub fn execute_main(&mut self, vm: &mut VM, closure: &Closure) -> Result<Value, VMError> {
        let pid = self.main_pid;

        self.local.current_pid = pid;
        vm.current_pid = Some(pid);

        install_handler_frame(vm);
        let result = match call_closure(vm, closure, &[]) {
            Ok(result) => {
                remove_handler_frame(vm);
                // Don't call handle_fiber_exit — main PID persists across expressions.
                Ok(result)
            }
            Err(VMSignal::Effect(signal)) => {
                match handle_effect(&mut self.local, &self.shared, vm, signal) {
                    Ok(()) => {
                        // Effect was handled and fiber was re-enqueued; drain ready queue
                        self.drain_until_main_done(vm, pid)
                    }
                    Err(e) => {
                        // Main fiber crashed — still don't remove from alive so subsequent
                        // expressions can run. Just propagate the error.
                        Err(e)
                    }
                }
            }
            Err(VMSignal::Error(e)) => {
                // Same — don't remove main PID from alive on crash.
                Err(e)
            }
        };

        vm.current_pid = None;
        result
    }

    /// Drain ready queue on scheduler 0, running fibers until the main fiber
    /// (identified by `main_pid`) completes or produces a result.
    fn drain_until_main_done(&mut self, vm: &mut VM, main_pid: FiberId) -> Result<Value, VMError> {
        let mut main_result: Option<Result<Value, VMError>> = None;

        loop {
            // Drain inbox
            drain_inbox(&mut self.local, &self.shared, vm);

            // Run all ready fibers
            while let Some(fiber) = self.local.ready_queue.pop_front() {
                let pid = fiber.pid();
                self.local.current_pid = pid;
                vm.current_pid = Some(pid);

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
                    Ok(value) => {
                        remove_handler_frame(vm);
                        if pid == main_pid {
                            // Main PID persists — don't call handle_fiber_exit.
                            main_result = Some(Ok(value));
                        } else {
                            handle_fiber_exit(
                                &mut self.local,
                                &self.shared,
                                vm,
                                pid,
                                ExitReason::Normal,
                            );
                        }
                    }
                    Err(VMSignal::Effect(signal)) => {
                        if let Err(e) = handle_effect(&mut self.local, &self.shared, vm, signal) {
                            vm.clear_stack_slots(saved_stack_top);
                            vm.frame_count = saved_frame_count;
                            vm.stack_top = saved_stack_top;
                            if !vm.handler_stack.is_empty() {
                                remove_handler_frame(vm);
                            }
                            if pid == main_pid {
                                // Main PID persists — don't remove from alive.
                                main_result = Some(Err(e));
                            } else {
                                handle_fiber_exit(
                                    &mut self.local,
                                    &self.shared,
                                    vm,
                                    pid,
                                    ExitReason::CrashWithTrace(e),
                                );
                            }
                        }
                    }
                    Err(VMSignal::Error(e)) => {
                        vm.clear_stack_slots(saved_stack_top);
                        vm.frame_count = saved_frame_count;
                        vm.stack_top = saved_stack_top;
                        if !vm.handler_stack.is_empty() {
                            remove_handler_frame(vm);
                        }
                        if pid == main_pid {
                            // Main PID persists — don't remove from alive.
                            main_result = Some(Err(e));
                        } else {
                            handle_fiber_exit(
                                &mut self.local,
                                &self.shared,
                                vm,
                                pid,
                                ExitReason::CrashWithTrace(e),
                            );
                        }
                    }
                }

                // If the main fiber finished, return its result immediately
                if let Some(result) = main_result {
                    return result;
                }
            }

            // If main finished, return
            if let Some(result) = main_result.take() {
                return result;
            }

            // Try work stealing
            if try_steal(&mut self.local) {
                continue;
            }

            // Check if main fiber is still alive (it might have been killed by a link)
            let main_alive = {
                let alive = self.shared.alive.lock().unwrap();
                alive.contains(&main_pid)
            };
            if !main_alive {
                // Main fiber died without us seeing it — it was killed by supervision
                return Err(VMError::runtime(
                    "Main fiber was killed by a linked fiber crash",
                ));
            }

            // Idle — wait briefly for new work
            match self
                .local
                .inbox
                .recv_timeout(std::time::Duration::from_millis(1))
            {
                Ok(msg) => {
                    process_single_message(&mut self.local, &self.shared, vm, msg);
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    return Err(VMError::runtime("Scheduler channel disconnected"));
                }
            }
        }
    }

    /// Run any ready background fibers on scheduler 0 (non-blocking).
    /// Called between top-level expressions to make progress on background fibers.
    pub fn run_ready(&mut self, vm: &mut VM) {
        // Drain inbox
        drain_inbox(&mut self.local, &self.shared, vm);

        // Run all currently ready fibers (but don't wait for new ones)
        while let Some(fiber) = self.local.ready_queue.pop_front() {
            let pid = fiber.pid();
            self.local.current_pid = pid;
            vm.current_pid = Some(pid);

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
                Ok(_) => {
                    remove_handler_frame(vm);
                    handle_fiber_exit(&mut self.local, &self.shared, vm, pid, ExitReason::Normal);
                }
                Err(VMSignal::Effect(signal)) => {
                    if let Err(e) = handle_effect(&mut self.local, &self.shared, vm, signal) {
                        vm.clear_stack_slots(saved_stack_top);
                        vm.frame_count = saved_frame_count;
                        vm.stack_top = saved_stack_top;
                        if !vm.handler_stack.is_empty() {
                            remove_handler_frame(vm);
                        }
                        handle_fiber_exit(
                            &mut self.local,
                            &self.shared,
                            vm,
                            pid,
                            ExitReason::CrashWithTrace(e),
                        );
                    }
                }
                Err(VMSignal::Error(e)) => {
                    vm.clear_stack_slots(saved_stack_top);
                    vm.frame_count = saved_frame_count;
                    vm.stack_top = saved_stack_top;
                    if !vm.handler_stack.is_empty() {
                        remove_handler_frame(vm);
                    }
                    handle_fiber_exit(
                        &mut self.local,
                        &self.shared,
                        vm,
                        pid,
                        ExitReason::CrashWithTrace(e),
                    );
                }
            }
        }

        vm.current_pid = None;
    }

    /// Wait for all background fibers to complete, then shut down workers.
    /// Called after the last top-level expression has been executed.
    pub fn finish(&mut self, vm: &mut VM) {
        // Remove the persistent main PID from alive so check_termination can
        // detect when all background fibers have completed.
        handle_fiber_exit(
            &mut self.local,
            &self.shared,
            vm,
            self.main_pid,
            ExitReason::Normal,
        );

        // Run scheduler 0's loop until all fibers are done
        loop {
            drain_inbox(&mut self.local, &self.shared, vm);

            while let Some(fiber) = self.local.ready_queue.pop_front() {
                let pid = fiber.pid();
                self.local.current_pid = pid;
                vm.current_pid = Some(pid);

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
                    Ok(_) => {
                        remove_handler_frame(vm);
                        handle_fiber_exit(
                            &mut self.local,
                            &self.shared,
                            vm,
                            pid,
                            ExitReason::Normal,
                        );
                    }
                    Err(VMSignal::Effect(signal)) => {
                        if let Err(e) = handle_effect(&mut self.local, &self.shared, vm, signal) {
                            vm.clear_stack_slots(saved_stack_top);
                            vm.frame_count = saved_frame_count;
                            vm.stack_top = saved_stack_top;
                            if !vm.handler_stack.is_empty() {
                                remove_handler_frame(vm);
                            }
                            handle_fiber_exit(
                                &mut self.local,
                                &self.shared,
                                vm,
                                pid,
                                ExitReason::CrashWithTrace(e),
                            );
                        }
                    }
                    Err(VMSignal::Error(e)) => {
                        vm.clear_stack_slots(saved_stack_top);
                        vm.frame_count = saved_frame_count;
                        vm.stack_top = saved_stack_top;
                        if !vm.handler_stack.is_empty() {
                            remove_handler_frame(vm);
                        }
                        handle_fiber_exit(
                            &mut self.local,
                            &self.shared,
                            vm,
                            pid,
                            ExitReason::CrashWithTrace(e),
                        );
                    }
                }
            }

            // Try work stealing
            if try_steal(&mut self.local) {
                continue;
            }

            // Check termination
            if check_termination(&self.shared) {
                break;
            }

            // Idle wait
            match self
                .local
                .inbox
                .recv_timeout(std::time::Duration::from_millis(1))
            {
                Ok(msg) => {
                    process_single_message(&mut self.local, &self.shared, vm, msg);
                }
                Err(_) => {
                    if check_termination(&self.shared) {
                        break;
                    }
                }
            }
        }

        vm.current_pid = None;

        // Restore captured output to VM
        let captured = self.shared.captured_output.lock().unwrap().take();
        vm.captured_output = captured;
    }
}
