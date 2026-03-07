//! Bytecode-to-Cranelift-IR translator.
//!
//! Translates a `FunctionPrototype`'s bytecode into Cranelift IR by abstract
//! interpretation of the stack machine. A virtual stack of Cranelift `Value`s
//! tracks the compile-time SSA values that correspond to the runtime stack.

use crate::bytecode::{ConstantValue, FunctionPrototype, Instruction, Op};
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::instructions::BlockArg;
use cranelift_codegen::ir::types;
use cranelift_codegen::ir::{FuncRef, InstBuilder, MemFlags, StackSlotData, StackSlotKind};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};

/// Pointer-sized integer type (always I64 on 64-bit platforms).
const PTR_TY: types::Type = types::I64;

/// A tagged value in JIT code is represented as two SSA values: (tag: I8, payload: I64).
/// We track both components on the virtual stack.
#[derive(Clone, Copy)]
pub(crate) struct JitValue {
    pub tag: cranelift_codegen::ir::Value,
    pub payload: cranelift_codegen::ir::Value,
}

/// Result of translating a function prototype.
pub(crate) struct TranslationResult {
    /// Whether the function was successfully translated (false = contains unsupported ops).
    pub success: bool,
}

/// Check if a function prototype is eligible for JIT compilation.
/// Rejects functions with features we can't handle yet.
pub(crate) fn can_jit(proto: &FunctionPrototype) -> bool {
    // Reject variadic functions for now
    if proto.is_variadic {
        return false;
    }

    // Reject functions with mutable upvalues
    for uv in &proto.upvalues {
        if uv.is_mutable {
            return false;
        }
    }

    // Scan for unsupported opcodes
    for inst in &proto.chunk.code {
        match inst.op {
            // Supported
            Op::PushConst
            | Op::PushLocal
            | Op::SetLocal
            | Op::PushUpvalue
            | Op::PushGlobal
            | Op::SetGlobal
            | Op::Pop
            | Op::Dup
            | Op::AddInt
            | Op::SubInt
            | Op::MulInt
            | Op::DivInt
            | Op::NegInt
            | Op::AddFloat
            | Op::SubFloat
            | Op::MulFloat
            | Op::DivFloat
            | Op::NegFloat
            | Op::Eq
            | Op::LtInt
            | Op::GtInt
            | Op::LeInt
            | Op::GeInt
            | Op::LtFloat
            | Op::GtFloat
            | Op::LeFloat
            | Op::GeFloat
            | Op::Not
            | Op::Jump
            | Op::JumpIfFalse
            | Op::Return
            | Op::Call
            | Op::Closure
            | Op::Construct
            | Op::GetField
            | Op::MatchTag
            | Op::TailCall
            | Op::Perform => {}

            // Unsupported — bail out
            Op::SetUpvalue // mutable upvalues
            | Op::PushHandler
            | Op::PopHandler
            | Op::Resume
            | Op::Halt => return false,
        }
    }

    true
}

/// Compute the stack effect of a single instruction.
/// Returns (pops, pushes) — how many values are popped and pushed.
fn stack_effect(inst: &Instruction) -> (usize, usize) {
    match inst.op {
        Op::PushConst | Op::PushLocal | Op::PushUpvalue | Op::PushGlobal => (0, 1),
        Op::SetLocal | Op::SetGlobal | Op::Pop => (1, 0),
        Op::Dup => (0, 1), // peeks top, pushes copy
        Op::AddInt | Op::SubInt | Op::MulInt | Op::DivInt => (2, 1),
        Op::AddFloat | Op::SubFloat | Op::MulFloat | Op::DivFloat => (2, 1),
        Op::NegInt | Op::NegFloat | Op::Not => (1, 1),
        Op::LtInt | Op::GtInt | Op::LeInt | Op::GeInt => (2, 1),
        Op::LtFloat | Op::GtFloat | Op::LeFloat | Op::GeFloat => (2, 1),
        Op::Eq => (2, 1),
        Op::Jump => (0, 0),
        Op::JumpIfFalse => (1, 0),
        Op::Return => (1, 0),
        Op::TailCall => (1 + inst.operand as usize, 0), // callee + args (doesn't push, terminates)
        Op::Call => {
            let arity = inst.operand as usize;
            (1 + arity, 1) // callee + args popped, result pushed
        }
        Op::Closure => (0, 1),
        Op::Construct => {
            let arity = inst.operand2 as usize;
            (arity, 1)
        }
        Op::GetField => (0, 1), // peeks top (doesn't pop), pushes field
        Op::MatchTag => (0, 0), // peeks top (doesn't pop), branches
        Op::Perform => {
            let arity = inst.operand2 as usize;
            (arity, 1) // pops N args, pushes result
        }
        Op::SetUpvalue | Op::PushHandler | Op::PopHandler | Op::Resume | Op::Halt => (0, 0), // rejected by can_jit
    }
}

/// Compute the stack depth at each block entry point via abstract interpretation.
/// Returns a map from bytecode IP → stack depth at that block's entry.
fn compute_block_depths(
    code: &[Instruction],
    block_starts: &std::collections::BTreeSet<usize>,
) -> std::collections::HashMap<usize, usize> {
    let mut depths: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    depths.insert(0, 0); // entry block starts with empty stack

    let mut worklist: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
    worklist.push_back(0);

    while let Some(block_start) = worklist.pop_front() {
        let mut depth = depths[&block_start];
        let mut ip = block_start;

        while ip < code.len() {
            let inst = &code[ip];
            let (pops, pushes) = stack_effect(inst);
            depth = depth.saturating_sub(pops) + pushes;
            ip += 1;

            // Check if this instruction is a terminator
            match inst.op {
                Op::Jump => {
                    let target = inst.operand as usize;
                    if let std::collections::hash_map::Entry::Vacant(e) = depths.entry(target) {
                        e.insert(depth);
                        worklist.push_back(target);
                    }
                    break;
                }
                Op::JumpIfFalse => {
                    let target = inst.operand as usize;
                    // True branch falls through to ip
                    if let std::collections::hash_map::Entry::Vacant(e) = depths.entry(target) {
                        e.insert(depth);
                        worklist.push_back(target);
                    }
                    if let std::collections::hash_map::Entry::Vacant(e) = depths.entry(ip) {
                        e.insert(depth);
                        worklist.push_back(ip);
                    }
                    break;
                }
                Op::MatchTag => {
                    let fail_target = inst.operand2 as usize;
                    // Success falls through to ip; fail goes to fail_target
                    if let std::collections::hash_map::Entry::Vacant(e) = depths.entry(fail_target)
                    {
                        e.insert(depth);
                        worklist.push_back(fail_target);
                    }
                    if let std::collections::hash_map::Entry::Vacant(e) = depths.entry(ip) {
                        e.insert(depth);
                        worklist.push_back(ip);
                    }
                    break;
                }
                Op::Return | Op::TailCall | Op::Halt => {
                    break;
                }
                _ => {
                    // If the next instruction starts a new block, record depth and break
                    if block_starts.contains(&ip) {
                        if let std::collections::hash_map::Entry::Vacant(e) = depths.entry(ip) {
                            e.insert(depth);
                            worklist.push_back(ip);
                        }
                        break;
                    }
                }
            }
        }
    }

    depths
}

/// Identify bytecode indices of `Call` instructions that are self-recursive.
/// A Call at ip `i` is self-recursive if the callee was pushed by a `PushGlobal`
/// whose constant matches the function's own name, and the stack depth between
/// the PushGlobal and the Call equals exactly `arity` (the args).
fn find_self_recursive_calls(
    code: &[Instruction],
    constants: &[ConstantValue],
    self_name: &str,
) -> std::collections::HashSet<usize> {
    let mut result = std::collections::HashSet::new();

    // Simulate stack depth, tracking which push was a self-global-push
    let mut stack_depth: usize = 0;
    // self_ref_depth: if Some(d), the self-reference was pushed when stack was at depth d
    let mut self_ref_depth: Option<usize> = None;

    for (ip, inst) in code.iter().enumerate() {
        let (pops, pushes) = stack_effect(inst);

        match inst.op {
            Op::PushGlobal => {
                let name = match &constants[inst.operand as usize] {
                    ConstantValue::String(s) => s.as_str(),
                    _ => "",
                };
                if name == self_name {
                    self_ref_depth = Some(stack_depth);
                }
                stack_depth += 1;
            }
            Op::Call | Op::TailCall => {
                let arity = inst.operand as usize;
                // The callee is at stack position (stack_depth - arity - 1)
                // It was a self-ref if self_ref_depth == stack_depth - arity - 1
                if let Some(d) = self_ref_depth
                    && stack_depth > arity
                    && d == stack_depth - arity - 1
                {
                    result.insert(ip);
                }
                // Call pops callee + args, pushes result
                stack_depth = stack_depth.saturating_sub(pops) + pushes;
                // Clear self_ref tracking if it was consumed
                if let Some(d) = self_ref_depth
                    && d >= stack_depth
                {
                    self_ref_depth = None;
                }
            }
            Op::Jump | Op::JumpIfFalse | Op::MatchTag | Op::Return => {
                // At control flow boundaries, reset tracking
                stack_depth = stack_depth.saturating_sub(pops) + pushes;
                self_ref_depth = None;
            }
            _ => {
                // Check if the self-ref was popped by this instruction
                if let Some(d) = self_ref_depth {
                    let new_depth = stack_depth.saturating_sub(pops);
                    if d >= new_depth {
                        // The self-reference was consumed by a non-Call instruction
                        self_ref_depth = None;
                    }
                }
                stack_depth = stack_depth.saturating_sub(pops) + pushes;
            }
        }
    }

    result
}

/// Translate a function prototype's bytecode into Cranelift IR.
///
/// The generated function has signature:
///   `(vm_ptr: I64, closure_ptr: I64, locals_ptr: I64) -> (I8, I64)`
///
/// Where:
///   - `vm_ptr`: pointer to the VM (for runtime callbacks)
///   - `closure_ptr`: pointer to the Closure (for upvalue access)
///   - `locals_ptr`: pointer to the locals array (pre-populated with args by caller)
///
/// Returns `(tag, payload)` — the raw components of a Value.
pub(crate) fn translate_function(
    proto: &FunctionPrototype,
    ptr_type: types::Type,
    func_ctx: &mut FunctionBuilderContext,
    ctx: &mut cranelift_codegen::Context,
    runtime_funcs: &RuntimeFuncRefs,
    self_func_ref: cranelift_codegen::ir::FuncRef,
) -> TranslationResult {
    let code = &proto.chunk.code;
    let constants = &proto.chunk.constants;
    let local_count = proto.chunk.local_count as usize;

    // Identify self-recursive call sites for direct-call optimization
    let self_recursive_calls = find_self_recursive_calls(code, constants, &proto.name);

    // --- Build the function signature ---
    // (vm_ptr: I64, closure_ptr: I64, locals_ptr: I64) -> (tag: I8, payload: I64)
    ctx.func
        .signature
        .params
        .push(cranelift_codegen::ir::AbiParam::new(PTR_TY)); // vm_ptr
    ctx.func
        .signature
        .params
        .push(cranelift_codegen::ir::AbiParam::new(PTR_TY)); // closure_ptr
    ctx.func
        .signature
        .params
        .push(cranelift_codegen::ir::AbiParam::new(PTR_TY)); // locals_ptr
    ctx.func
        .signature
        .returns
        .push(cranelift_codegen::ir::AbiParam::new(types::I8)); // tag
    ctx.func
        .signature
        .returns
        .push(cranelift_codegen::ir::AbiParam::new(PTR_TY)); // payload

    let mut bcx = FunctionBuilder::new(&mut ctx.func, func_ctx);

    // --- Identify basic block boundaries ---
    let mut block_starts = std::collections::BTreeSet::new();
    block_starts.insert(0usize);
    for (i, inst) in code.iter().enumerate() {
        match inst.op {
            Op::Jump => {
                block_starts.insert(inst.operand as usize);
                if i + 1 < code.len() {
                    block_starts.insert(i + 1);
                }
            }
            Op::JumpIfFalse => {
                block_starts.insert(inst.operand as usize);
                if i + 1 < code.len() {
                    block_starts.insert(i + 1);
                }
            }
            Op::MatchTag => {
                block_starts.insert(inst.operand2 as usize);
                if i + 1 < code.len() {
                    block_starts.insert(i + 1);
                }
            }
            _ => {}
        }
    }

    // Compute stack depths at each block entry via abstract interpretation
    let block_depths = compute_block_depths(code, &block_starts);

    // Create Cranelift blocks for each bytecode block boundary.
    // Non-entry blocks get block parameters for values flowing across edges.
    let mut bc_to_block = std::collections::HashMap::new();
    let mut block_param_count: std::collections::HashMap<usize, usize> =
        std::collections::HashMap::new();
    for &start in &block_starts {
        let block = bcx.create_block();
        bc_to_block.insert(start, block);

        let depth = block_depths.get(&start).copied().unwrap_or(0);
        block_param_count.insert(start, depth);
        if start == 0 {
            // Entry block gets function params, no stack-value block params
        } else {
            // Add pairs of (tag: I8, payload: I64) for each stack value
            for _ in 0..depth {
                bcx.append_block_param(block, types::I8);
                bcx.append_block_param(block, PTR_TY);
            }
        }
    }

    // Check if we have any self-recursive tail calls — if so we need a loop header
    // for proper TCO in native code.
    let has_self_recursive_tailcalls = code
        .iter()
        .enumerate()
        .any(|(i, inst)| inst.op == Op::TailCall && self_recursive_calls.contains(&i));

    // Create entry trampoline block (receives function params).
    // This is separate from the bytecode block at IP 0, which allows self-recursive
    // tail calls to jump back to a loop header for proper TCO without needing
    // to pass function params as block arguments.
    let entry_block = bcx.create_block();
    bcx.switch_to_block(entry_block);
    bcx.append_block_params_for_function_params(entry_block);

    // Get function parameters
    let vm_ptr = bcx.block_params(entry_block)[0];
    let closure_ptr = bcx.block_params(entry_block)[1];
    let locals_ptr = bcx.block_params(entry_block)[2];

    // If self-recursive tail calls exist, create a loop header between entry and
    // bytecode IP 0. Self-recursive TailCalls store new args to locals memory and
    // jump to this block, giving us proper native TCO.
    let loop_header = if has_self_recursive_tailcalls {
        let lh = bcx.create_block();
        bcx.ins().jump(lh, &[]);
        bcx.switch_to_block(lh);
        Some(lh)
    } else {
        None
    };

    // Jump to the first bytecode block (which has depth 0 → no block params)
    // and switch the builder to it so the translation loop can start emitting code.
    let first_bytecode_block = bc_to_block[&0];
    bcx.ins().jump(first_bytecode_block, &[]);
    bcx.switch_to_block(first_bytecode_block);

    // Virtual stack for stack machine → SSA translation
    let mut vstack: Vec<JitValue> = Vec::new();

    /// Collect the current vstack as flat block arguments: [tag0, payload0, tag1, payload1, ...]
    fn vstack_as_block_args(vstack: &[JitValue]) -> Vec<BlockArg> {
        let mut args = Vec::with_capacity(vstack.len() * 2);
        for jv in vstack {
            args.push(BlockArg::Value(jv.tag));
            args.push(BlockArg::Value(jv.payload));
        }
        args
    }

    /// Populate the vstack from block parameters (pairs of tag, payload).
    fn vstack_from_block_params(
        bcx: &FunctionBuilder,
        block: cranelift_codegen::ir::Block,
        depth: usize,
    ) -> Vec<JitValue> {
        let params = bcx.block_params(block);
        let mut vstack = Vec::with_capacity(depth);
        for i in 0..depth {
            vstack.push(JitValue {
                tag: params[i * 2],
                payload: params[i * 2 + 1],
            });
        }
        vstack
    }

    // --- Translate bytecode ---
    let mut ip = 0;
    while ip < code.len() {
        // Check if this instruction starts a new block
        if ip > 0
            && let Some(&block) = bc_to_block.get(&ip)
        {
            let prev = &code[ip - 1];
            let is_terminated = matches!(
                prev.op,
                Op::Jump | Op::JumpIfFalse | Op::Return | Op::TailCall | Op::MatchTag
            );
            if !is_terminated {
                // Fallthrough: pass current vstack values as block args
                let args = vstack_as_block_args(&vstack);
                bcx.ins().jump(block, &args);
            }
            bcx.switch_to_block(block);
            let depth = block_param_count.get(&ip).copied().unwrap_or(0);
            vstack = vstack_from_block_params(&bcx, block, depth);
        }

        let inst = &code[ip];
        ip += 1;

        match inst.op {
            Op::PushConst => {
                let const_val = &constants[inst.operand as usize];
                let jv = emit_constant(&mut bcx, const_val);
                vstack.push(jv);
            }

            Op::PushLocal => {
                let slot = inst.operand as usize;
                let jv = emit_load_local(&mut bcx, locals_ptr, slot);
                vstack.push(jv);
            }

            Op::SetLocal => {
                let jv = vstack.pop().expect("vstack underflow on SetLocal");
                let slot = inst.operand as usize;
                emit_store_local(&mut bcx, locals_ptr, slot, &jv);
            }

            Op::PushUpvalue => {
                let upvalue_idx = inst.operand;
                let jv =
                    emit_get_upvalue(&mut bcx, closure_ptr, upvalue_idx, ptr_type, runtime_funcs);
                vstack.push(jv);
            }

            Op::PushGlobal => {
                let name = match &constants[inst.operand as usize] {
                    ConstantValue::String(s) => s.as_str(),
                    _ => panic!("PushGlobal expects string constant"),
                };
                let jv = emit_get_global(
                    &mut bcx,
                    vm_ptr,
                    name,
                    proto.module_id,
                    ptr_type,
                    runtime_funcs,
                );
                vstack.push(jv);
            }

            Op::SetGlobal => {
                let jv = vstack.pop().expect("vstack underflow on SetGlobal");
                let name = match &constants[inst.operand as usize] {
                    ConstantValue::String(s) => s.as_str(),
                    _ => panic!("SetGlobal expects string constant"),
                };
                emit_set_global(
                    &mut bcx,
                    vm_ptr,
                    name,
                    proto.module_id,
                    &jv,
                    ptr_type,
                    runtime_funcs,
                );
            }

            Op::Pop => {
                let _jv = vstack.pop().expect("vstack underflow on Pop");
                // Values on the virtual stack don't need explicit drops:
                // the locals array keeps references alive, and non-object values are trivial.
            }

            Op::Dup => {
                let top = *vstack.last().expect("vstack underflow on Dup");
                vstack.push(top);
            }

            // --- Integer Arithmetic ---
            Op::AddInt => {
                let b = vstack.pop().unwrap();
                let a = vstack.pop().unwrap();
                let result_payload = bcx.ins().iadd(a.payload, b.payload);
                let tag = bcx.ins().iconst(types::I8, 0); // TAG_INT
                vstack.push(JitValue {
                    tag,
                    payload: result_payload,
                });
            }

            Op::SubInt => {
                let b = vstack.pop().unwrap();
                let a = vstack.pop().unwrap();
                let result_payload = bcx.ins().isub(a.payload, b.payload);
                let tag = bcx.ins().iconst(types::I8, 0);
                vstack.push(JitValue {
                    tag,
                    payload: result_payload,
                });
            }

            Op::MulInt => {
                let b = vstack.pop().unwrap();
                let a = vstack.pop().unwrap();
                let result_payload = bcx.ins().imul(a.payload, b.payload);
                let tag = bcx.ins().iconst(types::I8, 0);
                vstack.push(JitValue {
                    tag,
                    payload: result_payload,
                });
            }

            Op::DivInt => {
                let b = vstack.pop().unwrap();
                let a = vstack.pop().unwrap();
                bcx.ins()
                    .trapz(b.payload, cranelift_codegen::ir::TrapCode::unwrap_user(1));
                let result_payload = bcx.ins().sdiv(a.payload, b.payload);
                let tag = bcx.ins().iconst(types::I8, 0);
                vstack.push(JitValue {
                    tag,
                    payload: result_payload,
                });
            }

            Op::NegInt => {
                let a = vstack.pop().unwrap();
                let result_payload = bcx.ins().ineg(a.payload);
                let tag = bcx.ins().iconst(types::I8, 0);
                vstack.push(JitValue {
                    tag,
                    payload: result_payload,
                });
            }

            // --- Float Arithmetic ---
            Op::AddFloat => {
                let b = vstack.pop().unwrap();
                let a = vstack.pop().unwrap();
                let a_f64 = bcx.ins().bitcast(types::F64, MemFlags::new(), a.payload);
                let b_f64 = bcx.ins().bitcast(types::F64, MemFlags::new(), b.payload);
                let result = bcx.ins().fadd(a_f64, b_f64);
                let result_payload = bcx.ins().bitcast(PTR_TY, MemFlags::new(), result);
                let tag = bcx.ins().iconst(types::I8, 1); // TAG_FLOAT
                vstack.push(JitValue {
                    tag,
                    payload: result_payload,
                });
            }

            Op::SubFloat => {
                let b = vstack.pop().unwrap();
                let a = vstack.pop().unwrap();
                let a_f64 = bcx.ins().bitcast(types::F64, MemFlags::new(), a.payload);
                let b_f64 = bcx.ins().bitcast(types::F64, MemFlags::new(), b.payload);
                let result = bcx.ins().fsub(a_f64, b_f64);
                let result_payload = bcx.ins().bitcast(PTR_TY, MemFlags::new(), result);
                let tag = bcx.ins().iconst(types::I8, 1);
                vstack.push(JitValue {
                    tag,
                    payload: result_payload,
                });
            }

            Op::MulFloat => {
                let b = vstack.pop().unwrap();
                let a = vstack.pop().unwrap();
                let a_f64 = bcx.ins().bitcast(types::F64, MemFlags::new(), a.payload);
                let b_f64 = bcx.ins().bitcast(types::F64, MemFlags::new(), b.payload);
                let result = bcx.ins().fmul(a_f64, b_f64);
                let result_payload = bcx.ins().bitcast(PTR_TY, MemFlags::new(), result);
                let tag = bcx.ins().iconst(types::I8, 1);
                vstack.push(JitValue {
                    tag,
                    payload: result_payload,
                });
            }

            Op::DivFloat => {
                let b = vstack.pop().unwrap();
                let a = vstack.pop().unwrap();
                let a_f64 = bcx.ins().bitcast(types::F64, MemFlags::new(), a.payload);
                let b_f64 = bcx.ins().bitcast(types::F64, MemFlags::new(), b.payload);
                let result = bcx.ins().fdiv(a_f64, b_f64);
                let result_payload = bcx.ins().bitcast(PTR_TY, MemFlags::new(), result);
                let tag = bcx.ins().iconst(types::I8, 1);
                vstack.push(JitValue {
                    tag,
                    payload: result_payload,
                });
            }

            Op::NegFloat => {
                let a = vstack.pop().unwrap();
                let a_f64 = bcx.ins().bitcast(types::F64, MemFlags::new(), a.payload);
                let result = bcx.ins().fneg(a_f64);
                let result_payload = bcx.ins().bitcast(PTR_TY, MemFlags::new(), result);
                let tag = bcx.ins().iconst(types::I8, 1);
                vstack.push(JitValue {
                    tag,
                    payload: result_payload,
                });
            }

            // --- Comparisons ---
            Op::LtInt => {
                let b = vstack.pop().unwrap();
                let a = vstack.pop().unwrap();
                emit_int_cmp(
                    &mut bcx,
                    &mut vstack,
                    a.payload,
                    b.payload,
                    IntCC::SignedLessThan,
                );
            }
            Op::GtInt => {
                let b = vstack.pop().unwrap();
                let a = vstack.pop().unwrap();
                emit_int_cmp(
                    &mut bcx,
                    &mut vstack,
                    a.payload,
                    b.payload,
                    IntCC::SignedGreaterThan,
                );
            }
            Op::LeInt => {
                let b = vstack.pop().unwrap();
                let a = vstack.pop().unwrap();
                emit_int_cmp(
                    &mut bcx,
                    &mut vstack,
                    a.payload,
                    b.payload,
                    IntCC::SignedLessThanOrEqual,
                );
            }
            Op::GeInt => {
                let b = vstack.pop().unwrap();
                let a = vstack.pop().unwrap();
                emit_int_cmp(
                    &mut bcx,
                    &mut vstack,
                    a.payload,
                    b.payload,
                    IntCC::SignedGreaterThanOrEqual,
                );
            }
            Op::LtFloat => {
                let b = vstack.pop().unwrap();
                let a = vstack.pop().unwrap();
                emit_float_cmp(
                    &mut bcx,
                    &mut vstack,
                    a.payload,
                    b.payload,
                    FloatCC::LessThan,
                );
            }
            Op::GtFloat => {
                let b = vstack.pop().unwrap();
                let a = vstack.pop().unwrap();
                emit_float_cmp(
                    &mut bcx,
                    &mut vstack,
                    a.payload,
                    b.payload,
                    FloatCC::GreaterThan,
                );
            }
            Op::LeFloat => {
                let b = vstack.pop().unwrap();
                let a = vstack.pop().unwrap();
                emit_float_cmp(
                    &mut bcx,
                    &mut vstack,
                    a.payload,
                    b.payload,
                    FloatCC::LessThanOrEqual,
                );
            }
            Op::GeFloat => {
                let b = vstack.pop().unwrap();
                let a = vstack.pop().unwrap();
                emit_float_cmp(
                    &mut bcx,
                    &mut vstack,
                    a.payload,
                    b.payload,
                    FloatCC::GreaterThanOrEqual,
                );
            }

            Op::Eq => {
                let b = vstack.pop().unwrap();
                let a = vstack.pop().unwrap();
                let jv = emit_values_equal(&mut bcx, &a, &b, ptr_type, runtime_funcs);
                vstack.push(jv);
            }

            Op::Not => {
                let a = vstack.pop().unwrap();
                let is_nil = bcx.ins().icmp_imm(IntCC::Equal, a.tag, 2);
                let is_false = bcx.ins().icmp_imm(IntCC::Equal, a.tag, 4);
                let is_falsy = bcx.ins().bor(is_nil, is_false);
                let tag_true = bcx.ins().iconst(types::I8, 3);
                let tag_false = bcx.ins().iconst(types::I8, 4);
                let result_tag = bcx.ins().select(is_falsy, tag_true, tag_false);
                let zero_payload = bcx.ins().iconst(PTR_TY, 0);
                vstack.push(JitValue {
                    tag: result_tag,
                    payload: zero_payload,
                });
            }

            // --- Control flow ---
            Op::Jump => {
                let target = inst.operand as usize;
                let target_block = bc_to_block[&target];
                let args = vstack_as_block_args(&vstack);
                bcx.ins().jump(target_block, &args);
            }

            Op::JumpIfFalse => {
                let target = inst.operand as usize;
                let cond = vstack.pop().unwrap();
                let is_nil = bcx.ins().icmp_imm(IntCC::Equal, cond.tag, 2);
                let is_false = bcx.ins().icmp_imm(IntCC::Equal, cond.tag, 4);
                let is_falsy = bcx.ins().bor(is_nil, is_false);
                let target_block = bc_to_block[&target];
                let fallthrough_block = bc_to_block[&ip];
                let target_args = vstack_as_block_args(&vstack);
                let fallthrough_args = vstack_as_block_args(&vstack);
                bcx.ins().brif(
                    is_falsy,
                    target_block,
                    &target_args,
                    fallthrough_block,
                    &fallthrough_args,
                );
            }

            Op::Return => {
                let result = vstack.pop().unwrap();
                bcx.ins().return_(&[result.tag, result.payload]);
            }

            // --- Function calls ---
            Op::Call => {
                let call_ip = ip - 1; // ip was already incremented
                let arity = inst.operand as usize;
                let mut args: Vec<JitValue> = Vec::with_capacity(arity);
                for _ in 0..arity {
                    args.push(vstack.pop().unwrap());
                }
                args.reverse();
                let callee = vstack.pop().unwrap();

                if self_recursive_calls.contains(&call_ip) {
                    // Direct self-recursive call: allocate locals on the native stack
                    let result = emit_self_call(
                        &mut bcx,
                        vm_ptr,
                        closure_ptr,
                        &args,
                        local_count,
                        ptr_type,
                        self_func_ref,
                    );
                    vstack.push(result);
                } else {
                    let result =
                        emit_call(&mut bcx, vm_ptr, &callee, &args, ptr_type, runtime_funcs);
                    vstack.push(result);
                }
            }

            Op::TailCall => {
                let call_ip = ip - 1;
                let arity = inst.operand as usize;
                let mut args: Vec<JitValue> = Vec::with_capacity(arity);
                for _ in 0..arity {
                    args.push(vstack.pop().unwrap());
                }
                args.reverse();
                let _callee = vstack.pop().unwrap();

                if self_recursive_calls.contains(&call_ip)
                    && let Some(lh) = loop_header
                {
                    // Self-recursive tail call with TCO: store new args to locals
                    // and jump back to the loop header. No native stack growth.
                    for (i, arg) in args.iter().enumerate() {
                        emit_store_local(&mut bcx, locals_ptr, i, arg);
                    }
                    bcx.ins().jump(lh, &[]);
                } else {
                    // General tail call: go through the runtime trampoline and return.
                    // TCO is maintained by the interpreter's dispatch logic.
                    let result =
                        emit_call(&mut bcx, vm_ptr, &_callee, &args, ptr_type, runtime_funcs);
                    bcx.ins().return_(&[result.tag, result.payload]);
                }
            }

            Op::Closure => {
                let jv = emit_closure_create(
                    &mut bcx,
                    &constants[inst.operand as usize],
                    closure_ptr,
                    locals_ptr,
                    local_count,
                    ptr_type,
                    runtime_funcs,
                );
                vstack.push(jv);
            }

            // --- ADTs ---
            Op::Construct => {
                let tag_name = match &constants[inst.operand as usize] {
                    ConstantValue::String(s) => s.as_str(),
                    _ => panic!("Construct expects string constant"),
                };
                let arity = inst.operand2 as usize;
                let mut fields: Vec<JitValue> = Vec::with_capacity(arity);
                for _ in 0..arity {
                    fields.push(vstack.pop().unwrap());
                }
                fields.reverse();
                let jv = emit_construct_adt(&mut bcx, tag_name, &fields, ptr_type, runtime_funcs);
                vstack.push(jv);
            }

            Op::GetField => {
                let field_idx = inst.operand;
                let top = *vstack.last().unwrap();
                let jv = emit_get_field(&mut bcx, &top, field_idx, ptr_type, runtime_funcs);
                vstack.push(jv);
            }

            Op::MatchTag => {
                let tag_name = match &constants[inst.operand as usize] {
                    ConstantValue::String(s) => s.as_str(),
                    _ => panic!("MatchTag expects string constant"),
                };
                let fail_target = inst.operand2 as usize;
                let top = *vstack.last().unwrap();
                let fail_block = bc_to_block[&fail_target];
                let success_block = bc_to_block[&ip];
                let fail_args = vstack_as_block_args(&vstack);
                let success_args = vstack_as_block_args(&vstack);
                emit_match_tag_with_args(
                    &mut bcx,
                    &top,
                    tag_name,
                    fail_block,
                    &fail_args,
                    success_block,
                    &success_args,
                    ptr_type,
                    runtime_funcs,
                );
            }

            // --- Effect operations ---
            Op::Perform => {
                let op_name = match &constants[inst.operand as usize] {
                    ConstantValue::String(s) => s.as_str(),
                    _ => panic!("Perform expects string constant for op name"),
                };
                let arity = inst.operand2 as usize;
                let mut args: Vec<JitValue> = Vec::with_capacity(arity);
                for _ in 0..arity {
                    args.push(vstack.pop().unwrap());
                }
                args.reverse();

                let result =
                    emit_perform(&mut bcx, vm_ptr, op_name, &args, ptr_type, runtime_funcs);
                vstack.push(result);
            }

            // These should have been rejected by can_jit
            Op::SetUpvalue | Op::PushHandler | Op::PopHandler | Op::Resume | Op::Halt => {
                unreachable!(
                    "Unsupported opcode {:?} should have been rejected by can_jit",
                    inst.op
                );
            }
        }
    }

    bcx.seal_all_blocks();
    bcx.finalize();

    TranslationResult { success: true }
}

// =============================================================================
// Helper emission functions
// =============================================================================

fn emit_constant(bcx: &mut FunctionBuilder, constant: &ConstantValue) -> JitValue {
    match constant {
        ConstantValue::Int(n) => {
            let tag = bcx.ins().iconst(types::I8, 0);
            let payload = bcx.ins().iconst(PTR_TY, *n);
            JitValue { tag, payload }
        }
        ConstantValue::Float(n) => {
            let tag = bcx.ins().iconst(types::I8, 1);
            let payload = bcx.ins().iconst(PTR_TY, n.to_bits() as i64);
            JitValue { tag, payload }
        }
        ConstantValue::Boolean(true) => {
            let tag = bcx.ins().iconst(types::I8, 3);
            let payload = bcx.ins().iconst(PTR_TY, 0);
            JitValue { tag, payload }
        }
        ConstantValue::Boolean(false) => {
            let tag = bcx.ins().iconst(types::I8, 4);
            let payload = bcx.ins().iconst(PTR_TY, 0);
            JitValue { tag, payload }
        }
        ConstantValue::Nil => {
            let tag = bcx.ins().iconst(types::I8, 2);
            let payload = bcx.ins().iconst(PTR_TY, 0);
            JitValue { tag, payload }
        }
        ConstantValue::String(_) => {
            // Allocate the string Value at translation time and embed the raw pointer.
            // The Arc reference is intentionally leaked — it lives as long as the JIT code.
            let val = constant.as_value();
            let tag_val = val.tag();
            let payload_val = val.payload();
            std::mem::forget(val);
            let tag = bcx.ins().iconst(types::I8, tag_val as i64);
            let payload = bcx.ins().iconst(PTR_TY, payload_val as i64);
            JitValue { tag, payload }
        }
        ConstantValue::Prototype(_)
        | ConstantValue::FfiSigs(_)
        | ConstantValue::FfiBound { .. } => {
            let tag = bcx.ins().iconst(types::I8, 2);
            let payload = bcx.ins().iconst(PTR_TY, 0);
            JitValue { tag, payload }
        }
    }
}

fn emit_load_local(
    bcx: &mut FunctionBuilder,
    locals_ptr: cranelift_codegen::ir::Value,
    slot: usize,
) -> JitValue {
    let offset = (slot * 16) as i32;
    let tag = bcx
        .ins()
        .load(types::I8, MemFlags::trusted(), locals_ptr, offset);
    let payload = bcx
        .ins()
        .load(PTR_TY, MemFlags::trusted(), locals_ptr, offset + 8);
    JitValue { tag, payload }
}

fn emit_store_local(
    bcx: &mut FunctionBuilder,
    locals_ptr: cranelift_codegen::ir::Value,
    slot: usize,
    jv: &JitValue,
) {
    let offset = (slot * 16) as i32;
    bcx.ins()
        .store(MemFlags::trusted(), jv.tag, locals_ptr, offset);
    bcx.ins()
        .store(MemFlags::trusted(), jv.payload, locals_ptr, offset + 8);
}

fn emit_int_cmp(
    bcx: &mut FunctionBuilder,
    vstack: &mut Vec<JitValue>,
    a: cranelift_codegen::ir::Value,
    b: cranelift_codegen::ir::Value,
    cc: IntCC,
) {
    let cmp = bcx.ins().icmp(cc, a, b);
    let tag_true = bcx.ins().iconst(types::I8, 3);
    let tag_false = bcx.ins().iconst(types::I8, 4);
    let tag = bcx.ins().select(cmp, tag_true, tag_false);
    let payload = bcx.ins().iconst(PTR_TY, 0);
    vstack.push(JitValue { tag, payload });
}

fn emit_float_cmp(
    bcx: &mut FunctionBuilder,
    vstack: &mut Vec<JitValue>,
    a_payload: cranelift_codegen::ir::Value,
    b_payload: cranelift_codegen::ir::Value,
    cc: FloatCC,
) {
    let a_f64 = bcx.ins().bitcast(types::F64, MemFlags::new(), a_payload);
    let b_f64 = bcx.ins().bitcast(types::F64, MemFlags::new(), b_payload);
    let cmp = bcx.ins().fcmp(cc, a_f64, b_f64);
    let tag_true = bcx.ins().iconst(types::I8, 3);
    let tag_false = bcx.ins().iconst(types::I8, 4);
    let tag = bcx.ins().select(cmp, tag_true, tag_false);
    let payload = bcx.ins().iconst(PTR_TY, 0);
    vstack.push(JitValue { tag, payload });
}

fn emit_values_equal(
    bcx: &mut FunctionBuilder,
    a: &JitValue,
    b: &JitValue,
    ptr_type: types::Type,
    runtime_funcs: &RuntimeFuncRefs,
) -> JitValue {
    let func_ref = runtime_funcs.values_equal;
    let a_tag_ext = bcx.ins().uextend(types::I64, a.tag);
    let b_tag_ext = bcx.ins().uextend(types::I64, b.tag);
    let call = bcx
        .ins()
        .call(func_ref, &[a_tag_ext, a.payload, b_tag_ext, b.payload]);
    let result = bcx.inst_results(call)[0];
    let is_equal = bcx.ins().icmp_imm(IntCC::NotEqual, result, 0);
    let tag_true = bcx.ins().iconst(types::I8, 3);
    let tag_false = bcx.ins().iconst(types::I8, 4);
    let tag = bcx.ins().select(is_equal, tag_true, tag_false);
    let payload = bcx.ins().iconst(ptr_type, 0);
    JitValue { tag, payload }
}

fn emit_call(
    bcx: &mut FunctionBuilder,
    vm_ptr: cranelift_codegen::ir::Value,
    callee: &JitValue,
    args: &[JitValue],
    ptr_type: types::Type,
    runtime_funcs: &RuntimeFuncRefs,
) -> JitValue {
    let num_args = args.len();
    let args_slot = if num_args > 0 {
        let slot = bcx.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            (num_args * 16) as u32,
            8,
        ));
        for (i, arg) in args.iter().enumerate() {
            let offset = (i * 16) as i32;
            bcx.ins().stack_store(arg.tag, slot, offset);
            bcx.ins().stack_store(arg.payload, slot, offset + 8);
        }
        Some(slot)
    } else {
        None
    };

    let result_slot =
        bcx.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 16, 8));
    let args_ptr = if let Some(slot) = args_slot {
        bcx.ins().stack_addr(ptr_type, slot, 0)
    } else {
        bcx.ins().iconst(ptr_type, 0)
    };
    let callee_tag_ext = bcx.ins().uextend(types::I64, callee.tag);
    let num_args_val = bcx.ins().iconst(types::I32, num_args as i64);
    let out_tag_ptr = bcx.ins().stack_addr(ptr_type, result_slot, 0);
    let out_payload_ptr = bcx.ins().stack_addr(ptr_type, result_slot, 8);

    let func_ref = runtime_funcs.call;
    let call = bcx.ins().call(
        func_ref,
        &[
            vm_ptr,
            callee_tag_ext,
            callee.payload,
            args_ptr,
            num_args_val,
            out_tag_ptr,
            out_payload_ptr,
        ],
    );
    let status = bcx.inst_results(call)[0];

    // Check if we need to bail (status == 2: cross-boundary effect)
    let bail_cond = bcx.ins().icmp_imm(IntCC::Equal, status, 2);
    let bail_block = bcx.create_block();
    let continue_block = bcx.create_block();
    bcx.ins()
        .brif(bail_cond, bail_block, &[], continue_block, &[]);

    // Bail block: return the bail sentinel (tag=0xFF, payload=0)
    bcx.switch_to_block(bail_block);
    bcx.seal_block(bail_block);
    let bail_tag = bcx.ins().iconst(types::I8, 0xFF_i64);
    let bail_payload = bcx.ins().iconst(ptr_type, 0);
    bcx.ins().return_(&[bail_tag, bail_payload]);

    // Continue block: load the result
    bcx.switch_to_block(continue_block);
    bcx.seal_block(continue_block);

    let result_tag = bcx.ins().stack_load(types::I8, result_slot, 0);
    let result_payload = bcx.ins().stack_load(ptr_type, result_slot, 8);
    JitValue {
        tag: result_tag,
        payload: result_payload,
    }
}

/// Emit a direct self-recursive call. Allocates callee locals on the native stack
/// and calls the function's own FuncRef directly, bypassing the runtime trampoline.
fn emit_self_call(
    bcx: &mut FunctionBuilder,
    vm_ptr: cranelift_codegen::ir::Value,
    closure_ptr: cranelift_codegen::ir::Value,
    args: &[JitValue],
    local_count: usize,
    ptr_type: types::Type,
    self_func_ref: cranelift_codegen::ir::FuncRef,
) -> JitValue {
    // Allocate locals on the native stack (16 bytes per Value: 8 tag + 8 payload)
    let locals_size = (local_count * 16) as u32;
    let locals_slot = bcx.create_sized_stack_slot(StackSlotData::new(
        StackSlotKind::ExplicitSlot,
        locals_size,
        8,
    ));

    // Store arguments into the first local slots.
    // Remaining locals are uninitialized — the bytecode compiler guarantees
    // SetLocal before any PushLocal for non-argument slots.
    for (i, arg) in args.iter().enumerate() {
        let offset = (i * 16) as i32;
        bcx.ins().stack_store(arg.tag, locals_slot, offset);
        bcx.ins().stack_store(arg.payload, locals_slot, offset + 8);
    }

    let locals_ptr = bcx.ins().stack_addr(ptr_type, locals_slot, 0);

    // Direct call to self — same signature: (vm, closure, locals) -> (tag, payload)
    let call = bcx
        .ins()
        .call(self_func_ref, &[vm_ptr, closure_ptr, locals_ptr]);
    let results = bcx.inst_results(call);
    let result_tag = results[0];
    let result_payload = results[1];

    // Check if the callee bailed (tag == 0xFF) — propagate bail up
    let bail_cond = bcx.ins().icmp_imm(IntCC::Equal, result_tag, 0xFF_i64);
    let bail_block = bcx.create_block();
    let continue_block = bcx.create_block();
    bcx.ins()
        .brif(bail_cond, bail_block, &[], continue_block, &[]);

    bcx.switch_to_block(bail_block);
    bcx.seal_block(bail_block);
    let bail_tag = bcx.ins().iconst(types::I8, 0xFF_i64);
    let bail_payload = bcx.ins().iconst(ptr_type, 0);
    bcx.ins().return_(&[bail_tag, bail_payload]);

    bcx.switch_to_block(continue_block);
    bcx.seal_block(continue_block);

    JitValue {
        tag: result_tag,
        payload: result_payload,
    }
}

fn emit_get_upvalue(
    bcx: &mut FunctionBuilder,
    closure_ptr: cranelift_codegen::ir::Value,
    upvalue_idx: u32,
    ptr_type: types::Type,
    runtime_funcs: &RuntimeFuncRefs,
) -> JitValue {
    let result_slot =
        bcx.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 16, 8));
    let out_tag_ptr = bcx.ins().stack_addr(ptr_type, result_slot, 0);
    let out_payload_ptr = bcx.ins().stack_addr(ptr_type, result_slot, 8);
    let idx_val = bcx.ins().iconst(types::I32, upvalue_idx as i64);
    bcx.ins().call(
        runtime_funcs.get_upvalue,
        &[closure_ptr, idx_val, out_tag_ptr, out_payload_ptr],
    );
    let tag = bcx.ins().stack_load(types::I8, result_slot, 0);
    let payload = bcx.ins().stack_load(ptr_type, result_slot, 8);
    JitValue { tag, payload }
}

fn emit_get_global(
    bcx: &mut FunctionBuilder,
    vm_ptr: cranelift_codegen::ir::Value,
    name: &str,
    module_id: u32,
    ptr_type: types::Type,
    runtime_funcs: &RuntimeFuncRefs,
) -> JitValue {
    let name_ptr = bcx.ins().iconst(ptr_type, name.as_ptr() as i64);
    let name_len = bcx.ins().iconst(ptr_type, name.len() as i64);
    let module_id_val = bcx.ins().iconst(types::I32, module_id as i64);
    let result_slot =
        bcx.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 16, 8));
    let out_tag_ptr = bcx.ins().stack_addr(ptr_type, result_slot, 0);
    let out_payload_ptr = bcx.ins().stack_addr(ptr_type, result_slot, 8);
    bcx.ins().call(
        runtime_funcs.get_global,
        &[
            vm_ptr,
            module_id_val,
            name_ptr,
            name_len,
            out_tag_ptr,
            out_payload_ptr,
        ],
    );
    let tag = bcx.ins().stack_load(types::I8, result_slot, 0);
    let payload = bcx.ins().stack_load(ptr_type, result_slot, 8);
    JitValue { tag, payload }
}

fn emit_set_global(
    bcx: &mut FunctionBuilder,
    vm_ptr: cranelift_codegen::ir::Value,
    name: &str,
    module_id: u32,
    jv: &JitValue,
    ptr_type: types::Type,
    runtime_funcs: &RuntimeFuncRefs,
) {
    let name_ptr = bcx.ins().iconst(ptr_type, name.as_ptr() as i64);
    let name_len = bcx.ins().iconst(ptr_type, name.len() as i64);
    let module_id_val = bcx.ins().iconst(types::I32, module_id as i64);
    let tag_ext = bcx.ins().uextend(types::I64, jv.tag);
    bcx.ins().call(
        runtime_funcs.set_global,
        &[
            vm_ptr,
            module_id_val,
            name_ptr,
            name_len,
            tag_ext,
            jv.payload,
        ],
    );
}

fn emit_closure_create(
    bcx: &mut FunctionBuilder,
    constant: &ConstantValue,
    closure_ptr: cranelift_codegen::ir::Value,
    locals_ptr: cranelift_codegen::ir::Value,
    local_count: usize,
    ptr_type: types::Type,
    runtime_funcs: &RuntimeFuncRefs,
) -> JitValue {
    let proto = match constant {
        ConstantValue::Prototype(p) => p,
        _ => panic!("Closure expects prototype constant"),
    };
    let proto_ptr = bcx.ins().iconst(ptr_type, proto as *const _ as i64);
    let locals_len = bcx.ins().iconst(types::I32, local_count as i64);
    let result_slot =
        bcx.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 16, 8));
    let out_tag_ptr = bcx.ins().stack_addr(ptr_type, result_slot, 0);
    let out_payload_ptr = bcx.ins().stack_addr(ptr_type, result_slot, 8);
    bcx.ins().call(
        runtime_funcs.create_closure,
        &[
            locals_ptr,
            locals_len,
            closure_ptr,
            proto_ptr,
            out_tag_ptr,
            out_payload_ptr,
        ],
    );
    let tag = bcx.ins().stack_load(types::I8, result_slot, 0);
    let payload = bcx.ins().stack_load(ptr_type, result_slot, 8);
    JitValue { tag, payload }
}

fn emit_construct_adt(
    bcx: &mut FunctionBuilder,
    tag_name: &str,
    fields: &[JitValue],
    ptr_type: types::Type,
    runtime_funcs: &RuntimeFuncRefs,
) -> JitValue {
    let num_fields = fields.len();
    let fields_slot = if num_fields > 0 {
        let slot = bcx.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            (num_fields * 16) as u32,
            8,
        ));
        for (i, f) in fields.iter().enumerate() {
            let offset = (i * 16) as i32;
            bcx.ins().stack_store(f.tag, slot, offset);
            bcx.ins().stack_store(f.payload, slot, offset + 8);
        }
        Some(slot)
    } else {
        None
    };
    let tag_ptr = bcx.ins().iconst(ptr_type, tag_name.as_ptr() as i64);
    let tag_len = bcx.ins().iconst(ptr_type, tag_name.len() as i64);
    let fields_ptr = if let Some(slot) = fields_slot {
        bcx.ins().stack_addr(ptr_type, slot, 0)
    } else {
        bcx.ins().iconst(ptr_type, 0)
    };
    let num_fields_val = bcx.ins().iconst(types::I32, num_fields as i64);
    let result_slot =
        bcx.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 16, 8));
    let out_tag_ptr = bcx.ins().stack_addr(ptr_type, result_slot, 0);
    let out_payload_ptr = bcx.ins().stack_addr(ptr_type, result_slot, 8);
    bcx.ins().call(
        runtime_funcs.construct_adt,
        &[
            tag_ptr,
            tag_len,
            fields_ptr,
            num_fields_val,
            out_tag_ptr,
            out_payload_ptr,
        ],
    );
    let tag = bcx.ins().stack_load(types::I8, result_slot, 0);
    let payload = bcx.ins().stack_load(ptr_type, result_slot, 8);
    JitValue { tag, payload }
}

fn emit_get_field(
    bcx: &mut FunctionBuilder,
    value: &JitValue,
    field_idx: u32,
    ptr_type: types::Type,
    runtime_funcs: &RuntimeFuncRefs,
) -> JitValue {
    let result_slot =
        bcx.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 16, 8));
    let out_tag_ptr = bcx.ins().stack_addr(ptr_type, result_slot, 0);
    let out_payload_ptr = bcx.ins().stack_addr(ptr_type, result_slot, 8);
    let tag_ext = bcx.ins().uextend(types::I64, value.tag);
    let field_idx_val = bcx.ins().iconst(types::I32, field_idx as i64);
    bcx.ins().call(
        runtime_funcs.get_field,
        &[
            tag_ext,
            value.payload,
            field_idx_val,
            out_tag_ptr,
            out_payload_ptr,
        ],
    );
    let tag = bcx.ins().stack_load(types::I8, result_slot, 0);
    let payload = bcx.ins().stack_load(ptr_type, result_slot, 8);
    JitValue { tag, payload }
}

#[allow(clippy::too_many_arguments)]
fn emit_match_tag_with_args(
    bcx: &mut FunctionBuilder,
    value: &JitValue,
    tag_name: &str,
    fail_block: cranelift_codegen::ir::Block,
    fail_args: &[BlockArg],
    success_block: cranelift_codegen::ir::Block,
    success_args: &[BlockArg],
    ptr_type: types::Type,
    runtime_funcs: &RuntimeFuncRefs,
) {
    let tag_ptr = bcx.ins().iconst(ptr_type, tag_name.as_ptr() as i64);
    let tag_len = bcx.ins().iconst(ptr_type, tag_name.len() as i64);
    let tag_ext = bcx.ins().uextend(types::I64, value.tag);
    let call = bcx.ins().call(
        runtime_funcs.match_tag,
        &[tag_ext, value.payload, tag_ptr, tag_len],
    );
    let matches = bcx.inst_results(call)[0];
    let is_match = bcx.ins().icmp_imm(IntCC::NotEqual, matches, 0);
    bcx.ins()
        .brif(is_match, success_block, success_args, fail_block, fail_args);
}

/// Emit a Perform effect operation call.
///
/// Calls the `marlang_jit_perform` runtime helper which attempts the tail-resumptive
/// fast path. If the handler requires full continuation capture (status == 2), the
/// function returns a bail sentinel (tag = 0xFF) to its caller.
fn emit_perform(
    bcx: &mut FunctionBuilder,
    vm_ptr: cranelift_codegen::ir::Value,
    op_name: &str,
    args: &[JitValue],
    ptr_type: types::Type,
    runtime_funcs: &RuntimeFuncRefs,
) -> JitValue {
    let num_args = args.len();

    // Spill args to a stack slot (array of Values, 16 bytes each)
    let args_slot = if num_args > 0 {
        let slot = bcx.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            (num_args * 16) as u32,
            8,
        ));
        for (i, arg) in args.iter().enumerate() {
            let offset = (i * 16) as i32;
            bcx.ins().stack_store(arg.tag, slot, offset);
            bcx.ins().stack_store(arg.payload, slot, offset + 8);
        }
        Some(slot)
    } else {
        None
    };

    // Result slot (tag + payload)
    let result_slot =
        bcx.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 16, 8));

    let op_name_ptr = bcx.ins().iconst(ptr_type, op_name.as_ptr() as i64);
    let op_name_len = bcx.ins().iconst(types::I32, op_name.len() as i64);
    let args_ptr = if let Some(slot) = args_slot {
        bcx.ins().stack_addr(ptr_type, slot, 0)
    } else {
        bcx.ins().iconst(ptr_type, 0)
    };
    let num_args_val = bcx.ins().iconst(types::I32, num_args as i64);
    let out_tag_ptr = bcx.ins().stack_addr(ptr_type, result_slot, 0);
    let out_payload_ptr = bcx.ins().stack_addr(ptr_type, result_slot, 8);

    let call = bcx.ins().call(
        runtime_funcs.perform,
        &[
            vm_ptr,
            op_name_ptr,
            op_name_len,
            args_ptr,
            num_args_val,
            out_tag_ptr,
            out_payload_ptr,
        ],
    );
    let status = bcx.inst_results(call)[0];

    // Check if we need to bail (status == 2)
    let bail_cond = bcx.ins().icmp_imm(IntCC::Equal, status, 2);

    let bail_block = bcx.create_block();
    let continue_block = bcx.create_block();
    bcx.ins()
        .brif(bail_cond, bail_block, &[], continue_block, &[]);

    // Bail block: return the bail sentinel (tag=0xFF, payload=0)
    bcx.switch_to_block(bail_block);
    bcx.seal_block(bail_block);
    let bail_tag = bcx.ins().iconst(types::I8, 0xFF_i64);
    let bail_payload = bcx.ins().iconst(ptr_type, 0);
    bcx.ins().return_(&[bail_tag, bail_payload]);

    // Continue block: load the result
    bcx.switch_to_block(continue_block);
    bcx.seal_block(continue_block);
    let result_tag = bcx.ins().stack_load(types::I8, result_slot, 0);
    let result_payload = bcx.ins().stack_load(ptr_type, result_slot, 8);
    JitValue {
        tag: result_tag,
        payload: result_payload,
    }
}

// =============================================================================
// Runtime function references
// =============================================================================

/// Pre-declared references to runtime helper functions.
pub(crate) struct RuntimeFuncRefs {
    pub values_equal: FuncRef,
    pub call: FuncRef,
    pub get_upvalue: FuncRef,
    pub get_global: FuncRef,
    pub set_global: FuncRef,
    pub construct_adt: FuncRef,
    pub get_field: FuncRef,
    pub match_tag: FuncRef,
    pub create_closure: FuncRef,
    pub perform: FuncRef,
}
