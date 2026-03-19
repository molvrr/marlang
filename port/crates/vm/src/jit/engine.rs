//! JIT compilation engine — manages the Cranelift JIT module and compiles functions.

use crate::bytecode::FunctionPrototype;
use crate::jit::translate;
use cranelift_codegen::ir::types;
use cranelift_codegen::ir::{AbiParam, Function};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::FunctionBuilderContext;
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module};

/// Pointer-sized integer type.
const PTR_TY: types::Type = types::I64;

/// The JIT function type: (vm: *mut u8, closure: *mut u8, locals: *mut u8) -> (tag: u8, payload: u64)
pub type JitFn = unsafe extern "C" fn(*mut u8, *mut u8, *mut u8) -> JitReturn;

/// Return value from a JIT function — packed as (tag, payload).
#[repr(C)]
pub struct JitReturn {
    pub tag: u8,
    pub _pad: [u8; 7],
    pub payload: u64,
}

/// The JIT compilation engine.
pub struct JitEngine {
    module: JITModule,
    func_ctx: FunctionBuilderContext,
    func_counter: u32,
    runtime_func_ids: RuntimeFuncIds,
}

struct RuntimeFuncIds {
    values_equal: FuncId,
    call: FuncId,
    get_upvalue: FuncId,
    get_global: FuncId,
    set_global: FuncId,
    construct_adt: FuncId,
    get_field: FuncId,
    match_tag: FuncId,
    create_closure: FuncId,
    perform: FuncId,
}

impl JitEngine {
    /// Create a new JIT engine for the host architecture.
    pub fn new() -> Result<Self, String> {
        let mut flag_builder = settings::builder();
        flag_builder
            .set("use_colocated_libcalls", "false")
            .map_err(|e| e.to_string())?;
        flag_builder
            .set("is_pic", "false")
            .map_err(|e| e.to_string())?;
        flag_builder
            .set("opt_level", "speed")
            .map_err(|e| e.to_string())?;

        let isa_builder = cranelift_native::builder().map_err(|e| e.to_string())?;
        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .map_err(|e| e.to_string())?;

        let mut builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
        register_runtime_symbols(&mut builder);

        let mut module = JITModule::new(builder);
        let runtime_func_ids = declare_runtime_functions(&mut module)?;

        Ok(JitEngine {
            module,
            func_ctx: FunctionBuilderContext::new(),
            func_counter: 0,
            runtime_func_ids,
        })
    }

    /// Compile a function prototype to native code.
    /// Returns `Some(JitFn)` on success, `None` if the function can't be JIT-compiled.
    pub fn compile(&mut self, proto: &FunctionPrototype) -> Option<JitFn> {
        if !translate::can_jit(proto) {
            return None;
        }

        // Check if this is a pure integer recursive function — if so, compile a
        // specialized "fast" inner function with lean calling convention (raw i64
        // args/return, no tags, no bail checks, no vm/closure pointers).
        if translate::is_integer_pure_recursive(proto) {
            return self.compile_integer_pure(proto);
        }

        let func_name = format!("marlang_jit_{}", self.func_counter);
        self.func_counter += 1;

        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(PTR_TY));
        sig.params.push(AbiParam::new(PTR_TY));
        sig.params.push(AbiParam::new(PTR_TY));
        sig.returns.push(AbiParam::new(types::I8));
        sig.returns.push(AbiParam::new(PTR_TY));

        let func_id = self
            .module
            .declare_function(&func_name, Linkage::Local, &sig)
            .ok()?;

        let mut ctx = self.module.make_context();
        let ptr_type = self.module.target_config().pointer_type();
        let runtime_refs = self.import_runtime_funcs(&mut ctx.func);

        // Declare a self-reference so the function can call itself directly
        let self_func_ref = self.module.declare_func_in_func(func_id, &mut ctx.func);

        let result = translate::translate_function(
            proto,
            ptr_type,
            &mut self.func_ctx,
            &mut ctx,
            &runtime_refs,
            self_func_ref,
        );

        if !result.success {
            self.module.clear_context(&mut ctx);
            return None;
        }

        if self.module.define_function(func_id, &mut ctx).is_err() {
            self.module.clear_context(&mut ctx);
            return None;
        }
        self.module.clear_context(&mut ctx);

        self.module.finalize_definitions().ok()?;
        let code_ptr = self.module.get_finalized_function(func_id);

        Some(unsafe { std::mem::transmute::<*const u8, JitFn>(code_ptr) })
    }

    /// Compile a pure integer recursive function with a specialized fast path.
    /// Generates two functions:
    ///   1. A "fast" inner function: `(arg0: I64, ...) -> I64` — lean self-recursion
    ///   2. A wrapper with the standard JIT signature that loads args and calls the fast fn
    fn compile_integer_pure(&mut self, proto: &FunctionPrototype) -> Option<JitFn> {
        let arity = proto.arity as usize;
        let local_count = proto.chunk.local_count as usize;

        // --- Compile the fast inner function ---
        let fast_name = format!("marlang_jit_fast_{}", self.func_counter);
        self.func_counter += 1;

        let mut fast_sig = self.module.make_signature();
        for _ in 0..arity {
            fast_sig.params.push(AbiParam::new(PTR_TY));
        }
        fast_sig.returns.push(AbiParam::new(PTR_TY));

        let fast_func_id = self
            .module
            .declare_function(&fast_name, Linkage::Local, &fast_sig)
            .ok()?;

        let mut ctx = self.module.make_context();
        let fast_self_ref = self
            .module
            .declare_func_in_func(fast_func_id, &mut ctx.func);

        let result =
            translate::translate_fast_function(proto, &mut self.func_ctx, &mut ctx, fast_self_ref);

        if !result.success {
            self.module.clear_context(&mut ctx);
            return None;
        }

        if self.module.define_function(fast_func_id, &mut ctx).is_err() {
            self.module.clear_context(&mut ctx);
            return None;
        }
        self.module.clear_context(&mut ctx);

        // --- Compile the wrapper function ---
        let wrapper_name = format!("marlang_jit_wrap_{}", self.func_counter);
        self.func_counter += 1;

        let mut wrapper_sig = self.module.make_signature();
        wrapper_sig.params.push(AbiParam::new(PTR_TY)); // vm
        wrapper_sig.params.push(AbiParam::new(PTR_TY)); // closure
        wrapper_sig.params.push(AbiParam::new(PTR_TY)); // locals_ptr
        wrapper_sig.returns.push(AbiParam::new(types::I8)); // tag
        wrapper_sig.returns.push(AbiParam::new(PTR_TY)); // payload

        let wrapper_func_id = self
            .module
            .declare_function(&wrapper_name, Linkage::Local, &wrapper_sig)
            .ok()?;

        let mut ctx = self.module.make_context();
        // Build wrapper body: load args from locals_ptr, call fast fn, return (TAG_INT, result)
        {
            use cranelift_codegen::ir::InstBuilder;
            use cranelift_frontend::FunctionBuilder;

            ctx.func.signature = wrapper_sig;
            let mut bcx = FunctionBuilder::new(&mut ctx.func, &mut self.func_ctx);
            let entry = bcx.create_block();
            bcx.switch_to_block(entry);
            bcx.append_block_params_for_function_params(entry);

            let _vm_ptr = bcx.block_params(entry)[0];
            let _closure_ptr = bcx.block_params(entry)[1];
            let locals_ptr = bcx.block_params(entry)[2];

            // Load arguments from locals_ptr (payload only, skip tag)
            let fast_ref = self.module.declare_func_in_func(fast_func_id, bcx.func);
            let mut args = Vec::with_capacity(arity);
            for i in 0..arity {
                let offset = (i * 16 + 8) as i32; // skip tag (8 bytes), load payload
                let payload = bcx.ins().load(
                    PTR_TY,
                    cranelift_codegen::ir::MemFlags::trusted(),
                    locals_ptr,
                    offset,
                );
                args.push(payload);
            }

            let call = bcx.ins().call(fast_ref, &args);
            let result = bcx.inst_results(call)[0];
            let tag_int = bcx.ins().iconst(types::I8, 0); // TAG_INT
            bcx.ins().return_(&[tag_int, result]);

            bcx.seal_all_blocks();
            bcx.finalize();
        }

        if self
            .module
            .define_function(wrapper_func_id, &mut ctx)
            .is_err()
        {
            self.module.clear_context(&mut ctx);
            return None;
        }
        self.module.clear_context(&mut ctx);

        // Finalize both functions
        self.module.finalize_definitions().ok()?;

        // The wrapper uses locals_ptr offset 8 to load the payload. This matches
        // Value's #[repr(C)] layout: tag at offset 0, payload at offset 8.
        // local_count is used by the caller to allocate the locals array.
        let _ = local_count;

        let code_ptr = self.module.get_finalized_function(wrapper_func_id);
        Some(unsafe { std::mem::transmute::<*const u8, JitFn>(code_ptr) })
    }

    fn import_runtime_funcs(&mut self, func: &mut Function) -> translate::RuntimeFuncRefs {
        translate::RuntimeFuncRefs {
            values_equal: self
                .module
                .declare_func_in_func(self.runtime_func_ids.values_equal, func),
            call: self
                .module
                .declare_func_in_func(self.runtime_func_ids.call, func),
            get_upvalue: self
                .module
                .declare_func_in_func(self.runtime_func_ids.get_upvalue, func),
            get_global: self
                .module
                .declare_func_in_func(self.runtime_func_ids.get_global, func),
            set_global: self
                .module
                .declare_func_in_func(self.runtime_func_ids.set_global, func),
            construct_adt: self
                .module
                .declare_func_in_func(self.runtime_func_ids.construct_adt, func),
            get_field: self
                .module
                .declare_func_in_func(self.runtime_func_ids.get_field, func),
            match_tag: self
                .module
                .declare_func_in_func(self.runtime_func_ids.match_tag, func),
            create_closure: self
                .module
                .declare_func_in_func(self.runtime_func_ids.create_closure, func),
            perform: self
                .module
                .declare_func_in_func(self.runtime_func_ids.perform, func),
        }
    }
}

fn register_runtime_symbols(builder: &mut JITBuilder) {
    use crate::jit::runtime;
    builder.symbol(
        "marlang_jit_values_equal",
        runtime::marlang_jit_values_equal as *const u8,
    );
    builder.symbol("marlang_jit_call", runtime::marlang_jit_call as *const u8);
    builder.symbol(
        "marlang_jit_get_upvalue",
        runtime::marlang_jit_get_upvalue as *const u8,
    );
    builder.symbol(
        "marlang_jit_get_global",
        runtime::marlang_jit_get_global as *const u8,
    );
    builder.symbol(
        "marlang_jit_set_global",
        runtime::marlang_jit_set_global as *const u8,
    );
    builder.symbol(
        "marlang_jit_construct_adt",
        runtime::marlang_jit_construct_adt as *const u8,
    );
    builder.symbol(
        "marlang_jit_get_field",
        runtime::marlang_jit_get_field as *const u8,
    );
    builder.symbol(
        "marlang_jit_match_tag",
        runtime::marlang_jit_match_tag as *const u8,
    );
    builder.symbol(
        "marlang_jit_create_closure",
        runtime::marlang_jit_create_closure as *const u8,
    );
    builder.symbol(
        "marlang_jit_perform",
        runtime::marlang_jit_perform as *const u8,
    );
}

fn declare_runtime_functions(module: &mut JITModule) -> Result<RuntimeFuncIds, String> {
    let map_err = |e: cranelift_module::ModuleError| e.to_string();

    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(PTR_TY));
    sig.params.push(AbiParam::new(PTR_TY));
    sig.params.push(AbiParam::new(PTR_TY));
    sig.params.push(AbiParam::new(PTR_TY));
    sig.returns.push(AbiParam::new(types::I32));
    let values_equal = module
        .declare_function("marlang_jit_values_equal", Linkage::Import, &sig)
        .map_err(map_err)?;

    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(PTR_TY));
    sig.params.push(AbiParam::new(PTR_TY));
    sig.params.push(AbiParam::new(PTR_TY));
    sig.params.push(AbiParam::new(PTR_TY));
    sig.params.push(AbiParam::new(types::I32));
    sig.params.push(AbiParam::new(PTR_TY));
    sig.params.push(AbiParam::new(PTR_TY));
    sig.returns.push(AbiParam::new(types::I32));
    let call = module
        .declare_function("marlang_jit_call", Linkage::Import, &sig)
        .map_err(map_err)?;

    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(PTR_TY));
    sig.params.push(AbiParam::new(types::I32));
    sig.params.push(AbiParam::new(PTR_TY));
    sig.params.push(AbiParam::new(PTR_TY));
    let get_upvalue = module
        .declare_function("marlang_jit_get_upvalue", Linkage::Import, &sig)
        .map_err(map_err)?;

    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(PTR_TY));
    sig.params.push(AbiParam::new(types::I32));
    sig.params.push(AbiParam::new(PTR_TY));
    sig.params.push(AbiParam::new(PTR_TY));
    sig.params.push(AbiParam::new(PTR_TY));
    sig.params.push(AbiParam::new(PTR_TY));
    sig.returns.push(AbiParam::new(types::I32));
    let get_global = module
        .declare_function("marlang_jit_get_global", Linkage::Import, &sig)
        .map_err(map_err)?;

    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(PTR_TY));
    sig.params.push(AbiParam::new(types::I32));
    sig.params.push(AbiParam::new(PTR_TY));
    sig.params.push(AbiParam::new(PTR_TY));
    sig.params.push(AbiParam::new(PTR_TY));
    sig.params.push(AbiParam::new(PTR_TY));
    let set_global = module
        .declare_function("marlang_jit_set_global", Linkage::Import, &sig)
        .map_err(map_err)?;

    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(PTR_TY));
    sig.params.push(AbiParam::new(PTR_TY));
    sig.params.push(AbiParam::new(PTR_TY));
    sig.params.push(AbiParam::new(types::I32));
    sig.params.push(AbiParam::new(PTR_TY));
    sig.params.push(AbiParam::new(PTR_TY));
    let construct_adt = module
        .declare_function("marlang_jit_construct_adt", Linkage::Import, &sig)
        .map_err(map_err)?;

    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(PTR_TY));
    sig.params.push(AbiParam::new(PTR_TY));
    sig.params.push(AbiParam::new(types::I32));
    sig.params.push(AbiParam::new(PTR_TY));
    sig.params.push(AbiParam::new(PTR_TY));
    sig.returns.push(AbiParam::new(types::I32));
    let get_field = module
        .declare_function("marlang_jit_get_field", Linkage::Import, &sig)
        .map_err(map_err)?;

    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(PTR_TY));
    sig.params.push(AbiParam::new(PTR_TY));
    sig.params.push(AbiParam::new(PTR_TY));
    sig.params.push(AbiParam::new(PTR_TY));
    sig.returns.push(AbiParam::new(types::I32));
    let match_tag = module
        .declare_function("marlang_jit_match_tag", Linkage::Import, &sig)
        .map_err(map_err)?;

    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(PTR_TY));
    sig.params.push(AbiParam::new(types::I32));
    sig.params.push(AbiParam::new(PTR_TY));
    sig.params.push(AbiParam::new(PTR_TY));
    sig.params.push(AbiParam::new(PTR_TY));
    sig.params.push(AbiParam::new(PTR_TY));
    let create_closure = module
        .declare_function("marlang_jit_create_closure", Linkage::Import, &sig)
        .map_err(map_err)?;

    // marlang_jit_perform(vm, op_name_ptr, op_name_len, args_ptr, num_args, out_tag, out_payload) -> u32
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(PTR_TY)); // vm
    sig.params.push(AbiParam::new(PTR_TY)); // op_name_ptr
    sig.params.push(AbiParam::new(types::I32)); // op_name_len
    sig.params.push(AbiParam::new(PTR_TY)); // args_ptr
    sig.params.push(AbiParam::new(types::I32)); // num_args
    sig.params.push(AbiParam::new(PTR_TY)); // out_tag
    sig.params.push(AbiParam::new(PTR_TY)); // out_payload
    sig.returns.push(AbiParam::new(types::I32));
    let perform = module
        .declare_function("marlang_jit_perform", Linkage::Import, &sig)
        .map_err(map_err)?;

    Ok(RuntimeFuncIds {
        values_equal,
        call,
        get_upvalue,
        get_global,
        set_global,
        construct_adt,
        get_field,
        match_tag,
        create_closure,
        perform,
    })
}
