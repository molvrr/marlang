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
    })
}
