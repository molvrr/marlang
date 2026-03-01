use std::collections::{HashMap, HashSet};

use marlang_core::ast::{self, Expr, LispValue};
use marlang_core::intern::{Interner, Symbol};

use crate::bytecode::{
    Chunk, ConstantValue, FfiSigEntry, FunctionPrototype, Instruction, Op, UpvalueDescriptor,
};
use crate::errors::CompileError;

// --- Known binary operators ---

fn binary_op(sym: Symbol, interner: &Interner) -> Option<Op> {
    let name = interner.resolve(sym);
    match name {
        "+" => Some(Op::Add),
        "-" => Some(Op::Sub),
        "*" => Some(Op::Mul),
        "/" => Some(Op::Div),
        "<" => Some(Op::Lt),
        ">" => Some(Op::Gt),
        "=" => Some(Op::Eq),
        "<=" => Some(Op::Le),
        ">=" => Some(Op::Ge),
        _ => None,
    }
}

// --- Pre-interned known symbols ---

struct KnownSymbols {
    define: Symbol,
    lambda: Symbol,
    if_: Symbol,
    let_: Symbol,
    begin: Symbol,
    cond: Symbol,
    match_: Symbol,
    data: Symbol,
    effect: Symbol,
    perform: Symbol,
    handle: Symbol,
    resume: Symbol,
    import: Symbol,
    class: Symbol,
    instance: Symbol,
    type_: Symbol,
    else_: Symbol,
    arrow: Symbol,
    return_: Symbol,
    colon: Symbol,
    wildcard: Symbol,
    hash_t: Symbol,
    hash_f: Symbol,
    nil: Symbol,
    negate: Symbol,
    minus: Symbol,
    foreign: Symbol,
    ffi_lib: Symbol,
    quote: Symbol,
    quasiquote: Symbol,
    unquote: Symbol,
    unquote_splicing: Symbol,
    defmacro: Symbol,
    dot: Symbol,
}

impl KnownSymbols {
    fn new(interner: &Interner) -> Self {
        KnownSymbols {
            define: interner.intern("define"),
            lambda: interner.intern("lambda"),
            if_: interner.intern("if"),
            let_: interner.intern("let"),
            begin: interner.intern("begin"),
            cond: interner.intern("cond"),
            match_: interner.intern("match"),
            data: interner.intern("data"),
            effect: interner.intern("effect"),
            perform: interner.intern("perform"),
            handle: interner.intern("handle"),
            resume: interner.intern("resume"),
            import: interner.intern("import"),
            class: interner.intern("class"),
            instance: interner.intern("instance"),
            type_: interner.intern("type"),
            else_: interner.intern("else"),
            arrow: interner.intern("->"),
            return_: interner.intern("return"),
            colon: interner.intern(":"),
            wildcard: interner.intern("_"),
            hash_t: interner.intern("#t"),
            hash_f: interner.intern("#f"),
            nil: interner.intern("nil"),
            negate: interner.intern("negate"),
            minus: interner.intern("-"),
            foreign: interner.intern("foreign"),
            ffi_lib: interner.intern("foreign-lib"),
            quote: interner.intern("quote"),
            quasiquote: interner.intern("quasiquote"),
            unquote: interner.intern("unquote"),
            unquote_splicing: interner.intern("unquote-splicing"),
            defmacro: interner.intern("defmacro"),
            dot: interner.intern("."),
        }
    }
}

/// Convert an FFI type name (from foreign-lib syntax) to a C type string for ffi-open.
fn ffi_type_to_c_string(name: &str) -> String {
    match name {
        "I8" => "i8".to_string(),
        "U8" => "u8".to_string(),
        "I16" => "i16".to_string(),
        "U16" => "u16".to_string(),
        "I32" => "i32".to_string(),
        "U32" => "u32".to_string(),
        "I64" | "Int" => "i64".to_string(),
        "U64" => "u64".to_string(),
        "F32" => "f32".to_string(),
        "F64" | "Float" => "f64".to_string(),
        "CString" => "cstring".to_string(),
        "Bool" => "bool".to_string(),
        "Void" => "void".to_string(),
        "Ptr" => "ptr".to_string(),
        // Struct names pass through as-is (e.g. "Color" → "Color")
        other => other.to_string(),
    }
}

// --- Local variable ---

#[derive(Debug, Clone)]
struct Local {
    name: Symbol,
    depth: u32,
    slot: u32,
}

// --- Parent scope snapshot for upvalue resolution ---

#[derive(Debug, Clone)]
struct ParentScope {
    locals: Vec<Local>,
    upvalues: Vec<UpvalueDescriptor>,
    enclosing: Option<Box<ParentScope>>,
}

// --- Fail patches for pattern matching ---

#[derive(Debug, Clone, Copy)]
enum FailPatchKind {
    Jump,
    MatchTag,
}

#[derive(Debug, Clone, Copy)]
struct FailPatch {
    idx: usize,
    kind: FailPatchKind,
}

// --- Compiler ---

struct Compiler<'a> {
    chunk: Chunk,
    locals: Vec<Local>,
    local_count: u32,
    upvalues: Vec<UpvalueDescriptor>,
    scope_depth: u32,
    enclosing: Option<Box<ParentScope>>,
    #[allow(dead_code)]
    function_name: String,
    interner: &'a Interner,
    known_symbols: &'a KnownSymbols,
    known_constructors: HashSet<Symbol>,
    known_effect_ops: HashMap<Symbol, Symbol>, // opName → effectName
}

impl<'a> Compiler<'a> {
    fn new(
        name: impl Into<String>,
        interner: &'a Interner,
        known_symbols: &'a KnownSymbols,
        enclosing: Option<Box<ParentScope>>,
        known_constructors: HashSet<Symbol>,
        known_effect_ops: HashMap<Symbol, Symbol>,
    ) -> Self {
        Compiler {
            chunk: Chunk::new(),
            locals: Vec::new(),
            local_count: 0,
            upvalues: Vec::new(),
            scope_depth: 0,
            enclosing,
            function_name: name.into(),
            interner,
            known_symbols,
            known_constructors,
            known_effect_ops,
        }
    }

    /// Snapshot this compiler's state for child upvalue resolution.
    fn snapshot_scope(&self) -> Box<ParentScope> {
        Box::new(ParentScope {
            locals: self.locals.clone(),
            upvalues: self.upvalues.clone(),
            enclosing: self.enclosing.clone(),
        })
    }

    // --- Emission helpers ---

    fn emit(&mut self, op: Op, line: u32) -> usize {
        let idx = self.chunk.code.len();
        self.chunk.code.push(Instruction::new(op, line));
        idx
    }

    fn emit_op(&mut self, op: Op, line: u32, operand: u32) -> usize {
        let idx = self.chunk.code.len();
        self.chunk
            .code
            .push(Instruction::with_operand(op, line, operand));
        idx
    }

    fn emit_op2(&mut self, op: Op, line: u32, operand: u32, operand2: u32) -> usize {
        let idx = self.chunk.code.len();
        self.chunk
            .code
            .push(Instruction::with_operands(op, line, operand, operand2));
        idx
    }

    fn add_constant(&mut self, value: ConstantValue) -> u32 {
        // Deduplicate simple constants
        for (i, c) in self.chunk.constants.iter().enumerate() {
            if c.dedup_eq(&value) {
                return i as u32;
            }
        }
        self.chunk.constants.push(value);
        (self.chunk.constants.len() - 1) as u32
    }

    fn emit_constant(&mut self, value: ConstantValue, line: u32) {
        let idx = self.add_constant(value);
        self.emit_op(Op::PushConst, line, idx);
    }

    fn emit_jump(&mut self, op: Op, line: u32) -> usize {
        self.emit_op(op, line, 0xFFFF)
    }

    fn patch_jump(&mut self, jump_idx: usize) {
        self.chunk.code[jump_idx].operand = self.chunk.code.len() as u32;
    }

    fn emit_match_tag(&mut self, name_idx: u32, line: u32) -> usize {
        self.emit_op2(Op::MatchTag, line, name_idx, 0xFFFF)
    }

    fn current_pos(&self) -> usize {
        self.chunk.code.len()
    }

    // --- Scope management ---

    fn begin_scope(&mut self) {
        self.scope_depth += 1;
    }

    fn end_scope(&mut self) {
        self.scope_depth -= 1;
        while let Some(last) = self.locals.last() {
            if last.depth > self.scope_depth {
                self.locals.pop();
            } else {
                break;
            }
        }
    }

    fn add_local(&mut self, name: Symbol) -> u32 {
        let slot = self.local_count;
        self.local_count += 1;
        self.locals.push(Local {
            name,
            depth: self.scope_depth,
            slot,
        });
        slot
    }

    // --- Variable resolution ---

    fn resolve_local(&self, name: Symbol) -> Option<u32> {
        for local in self.locals.iter().rev() {
            if local.name == name {
                return Some(local.slot);
            }
        }
        None
    }

    fn add_upvalue(&mut self, is_local: bool, index: u16) -> u32 {
        for (i, uv) in self.upvalues.iter().enumerate() {
            if uv.is_local == is_local && uv.index == index {
                return i as u32;
            }
        }
        self.upvalues.push(UpvalueDescriptor { is_local, index });
        (self.upvalues.len() - 1) as u32
    }

    fn resolve_upvalue(&mut self, name: Symbol) -> Option<u32> {
        let enclosing = self.enclosing.as_ref()?;

        // Check enclosing locals
        for local in enclosing.locals.iter().rev() {
            if local.name == name {
                return Some(self.add_upvalue(true, local.slot as u16));
            }
        }

        // Walk enclosing chain for upvalue captures beyond immediate parent
        if let Some(grandparent) = &enclosing.enclosing {
            // Check grandparent locals
            for local in grandparent.locals.iter().rev() {
                if local.name == name {
                    // Parent should have captured this — add as non-local upvalue
                    // First, check if it's in enclosing's upvalues
                    for (i, uv) in enclosing.upvalues.iter().enumerate() {
                        if uv.is_local && uv.index == local.slot as u16 {
                            return Some(self.add_upvalue(false, i as u16));
                        }
                    }
                    // Not already captured by parent — this is a deeper case
                    // For simplicity, fall through to global
                    return None;
                }
            }
        }

        None
    }

    fn emit_variable(&mut self, name: Symbol, line: u32) {
        if let Some(slot) = self.resolve_local(name) {
            self.emit_op(Op::PushLocal, line, slot);
            return;
        }

        if let Some(idx) = self.resolve_upvalue(name) {
            self.emit_op(Op::PushUpvalue, line, idx);
            return;
        }

        // Global
        let name_str = self.interner.resolve(name).to_string();
        let name_idx = self.add_constant(ConstantValue::String(name_str));
        self.emit_op(Op::PushGlobal, line, name_idx);
    }

    // --- AST helpers ---

    fn get_line(expr: &Expr) -> u32 {
        expr.span().start
    }

    fn get_symbol(expr: &Expr) -> Option<Symbol> {
        match expr {
            Expr::Atom(LispValue::Symbol(sym), _) => Some(*sym),
            _ => None,
        }
    }

    fn get_param_name(&self, p: &Expr) -> Result<Symbol, CompileError> {
        match p {
            Expr::Atom(LispValue::Symbol(sym), _) => Ok(*sym),
            Expr::List { .. } => {
                let parts = ast::list_to_array(p);
                if parts.len() == 3 {
                    if let Some(sym) = Self::get_symbol(parts[0]) {
                        if let Some(colon) = Self::get_symbol(parts[1]) {
                            if colon == self.known_symbols.colon {
                                return Ok(sym);
                            }
                        }
                    }
                }
                Err(CompileError::new(
                    "Parameter must be a symbol or (symbol : Type)",
                    Self::get_line(p),
                ))
            }
            _ => Err(CompileError::new(
                "Parameter must be a symbol or (symbol : Type)",
                Self::get_line(p),
            )),
        }
    }

    /// Check if a symbol is a known constructor, with fallback to stripped name.
    fn is_known_constructor(&self, sym: Symbol) -> bool {
        if self.known_constructors.contains(&sym) {
            return true;
        }
        // Fallback: strip hygiene marks and check again
        let name = self.interner.resolve(sym);
        let stripped = crate::expand::strip_all_marks(name);
        if stripped != name {
            let stripped_sym = self.interner.intern(stripped);
            return self.known_constructors.contains(&stripped_sym);
        }
        false
    }

    /// Resolve a constructor name, stripping marks if needed to match the actual tag.
    fn resolve_constructor_name(&self, sym: Symbol) -> String {
        let name = self.interner.resolve(sym);
        if self.known_constructors.contains(&sym) {
            return name.to_string();
        }
        let stripped = crate::expand::strip_all_marks(name);
        stripped.to_string()
    }

    fn get_define_name(&self, expr: &Expr) -> Result<Symbol, CompileError> {
        match expr {
            Expr::Atom(LispValue::Symbol(sym), _) => Ok(*sym),
            Expr::List { .. } => {
                let parts = ast::list_to_array(expr);
                if parts.len() == 3 {
                    if let Some(sym) = Self::get_symbol(parts[0]) {
                        if let Some(colon) = Self::get_symbol(parts[1]) {
                            if colon == self.known_symbols.colon {
                                return Ok(sym);
                            }
                        }
                    }
                }
                Err(CompileError::new(
                    "define expects a symbol or (symbol : Type)",
                    Self::get_line(expr),
                ))
            }
            _ => Err(CompileError::new(
                "define expects a symbol or (symbol : Type)",
                Self::get_line(expr),
            )),
        }
    }

    // --- Main compilation ---

    fn compile_expr(&mut self, expr: &Expr, is_tail: bool) -> Result<(), CompileError> {
        let line = Self::get_line(expr);

        match expr {
            // Nil
            Expr::Nil(_) => {
                self.emit_constant(ConstantValue::Nil, line);
            }

            // Atom
            Expr::Atom(value, _) => match value {
                LispValue::Number(n, _) => {
                    self.emit_constant(ConstantValue::Number(*n), line);
                }
                LispValue::String(s) => {
                    self.emit_constant(ConstantValue::String(s.to_string()), line);
                }
                LispValue::Symbol(sym) => {
                    if *sym == self.known_symbols.hash_t {
                        self.emit_constant(ConstantValue::Boolean(true), line);
                    } else if *sym == self.known_symbols.hash_f {
                        self.emit_constant(ConstantValue::Boolean(false), line);
                    } else if *sym == self.known_symbols.nil {
                        self.emit_constant(ConstantValue::Nil, line);
                    } else {
                        self.emit_variable(*sym, line);
                    }
                }
            },

            // List — check for special forms
            Expr::List { .. } => {
                let elements = ast::list_to_array(expr);
                if elements.is_empty() {
                    self.emit_constant(ConstantValue::Nil, line);
                    return Ok(());
                }

                let first = elements[0];
                if let Some(sym) = Self::get_symbol(first) {
                    let ks = self.known_symbols;

                    if sym == ks.define {
                        return self.compile_define(&elements, line);
                    } else if sym == ks.lambda {
                        return self.compile_lambda(&elements, line);
                    } else if sym == ks.if_ {
                        return self.compile_if(&elements, line, is_tail);
                    } else if sym == ks.let_ {
                        return self.compile_let(&elements, line, is_tail);
                    } else if sym == ks.begin {
                        return self.compile_begin(&elements, line, is_tail);
                    } else if sym == ks.cond {
                        return self.compile_cond(&elements, line, is_tail);
                    } else if sym == ks.match_ {
                        return self.compile_match(&elements, line, is_tail);
                    } else if sym == ks.data {
                        return self.compile_data(&elements, line);
                    } else if sym == ks.effect {
                        return self.compile_effect(&elements, line);
                    } else if sym == ks.perform {
                        return self.compile_perform(&elements, line);
                    } else if sym == ks.handle {
                        return self.compile_handle(&elements, line);
                    } else if sym == ks.resume {
                        return self.compile_resume(&elements, line);
                    } else if sym == ks.import {
                        return Err(CompileError::new(
                            "import is not yet supported in the VM",
                            line,
                        ));
                    } else if sym == ks.ffi_lib {
                        return self.compile_ffi_lib(&elements, line);
                    } else if sym == ks.quote {
                        return self.compile_quote(&elements, line);
                    } else if sym == ks.quasiquote {
                        return self.compile_quasiquote(&elements, line);
                    } else if sym == ks.defmacro {
                        // defmacro should be consumed by the macro expander; at compile time, emit nil
                        self.emit_constant(ConstantValue::Nil, line);
                        return Ok(());
                    } else if sym == ks.class
                        || sym == ks.instance
                        || sym == ks.type_
                        || sym == ks.foreign
                    {
                        self.emit_constant(ConstantValue::Nil, line);
                        return Ok(());
                    }

                    // Binary operator optimization
                    if let Some(bin_op) = binary_op(sym, self.interner) {
                        if elements.len() == 3 {
                            self.compile_expr(elements[1], false)?;
                            self.compile_expr(elements[2], false)?;
                            self.emit(bin_op, line);
                            return Ok(());
                        }
                    }

                    // Unary negate: (- x) or (negate x)
                    if (sym == ks.minus || sym == ks.negate) && elements.len() == 2 {
                        self.compile_expr(elements[1], false)?;
                        self.emit(Op::Neg, line);
                        return Ok(());
                    }
                }

                // Function application
                self.compile_call(&elements, line, is_tail)?;
            }
        }

        Ok(())
    }

    // --- Special form compilation ---

    fn compile_define(&mut self, elements: &[&Expr], line: u32) -> Result<(), CompileError> {
        if elements.len() != 3 {
            return Err(CompileError::new("define expects 2 arguments", line));
        }
        let name = self.get_define_name(elements[1])?;
        self.compile_expr(elements[2], false)?;
        let name_str = self.interner.resolve(name).to_string();
        let name_idx = self.add_constant(ConstantValue::String(name_str));
        self.emit_op(Op::SetGlobal, line, name_idx);
        self.emit_constant(ConstantValue::Nil, line); // define returns nil
        Ok(())
    }

    fn compile_lambda(&mut self, elements: &[&Expr], line: u32) -> Result<(), CompileError> {
        if elements.len() < 3 {
            return Err(CompileError::new(
                "lambda expects at least 2 arguments",
                line,
            ));
        }

        let params_expr = elements[1];
        let param_exprs = ast::list_to_array(params_expr);

        // Detect dot notation for rest params: (a b . rest)
        let dot_pos = param_exprs.iter().position(|p| {
            matches!(p, Expr::Atom(LispValue::Symbol(sym), _) if *sym == self.known_symbols.dot)
        });

        let (required_params, rest_param, is_variadic) = if let Some(pos) = dot_pos {
            if pos + 1 >= param_exprs.len() {
                return Err(CompileError::new(
                    "rest parameter name expected after '.'",
                    line,
                ));
            }
            if pos + 2 != param_exprs.len() {
                return Err(CompileError::new(
                    "only one rest parameter name allowed after '.'",
                    line,
                ));
            }
            let mut required = Vec::new();
            for p in &param_exprs[..pos] {
                required.push(self.get_param_name(p)?);
            }
            let rest = self.get_param_name(param_exprs[pos + 1])?;
            (required, Some(rest), true)
        } else {
            let mut params = Vec::new();
            for p in &param_exprs {
                params.push(self.get_param_name(p)?);
            }
            (params, None, false)
        };

        let body_exprs: Vec<&Expr> = elements[2..].to_vec();

        // Create child compiler
        let parent_scope = self.snapshot_scope();
        let mut fn_compiler = Compiler::new(
            "<lambda>",
            self.interner,
            self.known_symbols,
            Some(parent_scope),
            self.known_constructors.clone(),
            self.known_effect_ops.clone(),
        );

        // Add required parameters as locals
        for &param in &required_params {
            fn_compiler.add_local(param);
        }
        // Add rest parameter as a local (will hold the packed list at runtime)
        if let Some(rest) = rest_param {
            fn_compiler.add_local(rest);
        }

        // Compile body
        fn_compiler.compile_sequence(&body_exprs, true)?;
        fn_compiler.emit(Op::Return, line);
        fn_compiler.chunk.local_count = fn_compiler.local_count;

        // Create prototype — arity is the number of REQUIRED params
        let proto = FunctionPrototype {
            name: "<lambda>".to_string(),
            arity: required_params.len() as u8,
            is_variadic,
            upvalues: fn_compiler.upvalues,
            chunk: fn_compiler.chunk,
        };

        // Emit CLOSURE in this (enclosing) context
        let proto_idx = self.add_constant(ConstantValue::Prototype(proto));
        self.emit_op(Op::Closure, line, proto_idx);
        Ok(())
    }

    fn compile_if(
        &mut self,
        elements: &[&Expr],
        line: u32,
        is_tail: bool,
    ) -> Result<(), CompileError> {
        if elements.len() < 3 || elements.len() > 4 {
            return Err(CompileError::new("if expects 2 or 3 arguments", line));
        }

        self.compile_expr(elements[1], false)?; // condition
        let jump_to_else = self.emit_jump(Op::JumpIfFalse, line);

        self.compile_expr(elements[2], is_tail)?; // then branch
        let jump_to_end = self.emit_jump(Op::Jump, line);

        self.patch_jump(jump_to_else);
        if elements.len() == 4 {
            self.compile_expr(elements[3], is_tail)?; // else branch
        } else {
            self.emit_constant(ConstantValue::Nil, line); // no else → nil
        }

        self.patch_jump(jump_to_end);
        Ok(())
    }

    fn compile_let(
        &mut self,
        elements: &[&Expr],
        line: u32,
        is_tail: bool,
    ) -> Result<(), CompileError> {
        if elements.len() < 3 {
            return Err(CompileError::new("let expects at least 2 arguments", line));
        }

        let bindings_expr = elements[1];
        let bindings_list = ast::list_to_array(bindings_expr);
        let body_exprs: Vec<&Expr> = elements[2..].to_vec();

        self.begin_scope();

        for binding in &bindings_list {
            let pair = ast::list_to_array(binding);
            if pair.len() != 2 {
                return Err(CompileError::new("let binding must be (name value)", line));
            }
            let name = self.get_param_name(pair[0])?;
            self.compile_expr(pair[1], false)?;
            let slot = self.add_local(name);
            self.emit_op(Op::SetLocal, line, slot);
        }

        self.compile_sequence(&body_exprs, is_tail)?;

        self.end_scope();
        Ok(())
    }

    fn compile_begin(
        &mut self,
        elements: &[&Expr],
        line: u32,
        is_tail: bool,
    ) -> Result<(), CompileError> {
        let body_exprs: Vec<&Expr> = elements[1..].to_vec();
        if body_exprs.is_empty() {
            self.emit_constant(ConstantValue::Nil, line);
            return Ok(());
        }
        self.compile_sequence(&body_exprs, is_tail)
    }

    fn compile_cond(
        &mut self,
        elements: &[&Expr],
        line: u32,
        is_tail: bool,
    ) -> Result<(), CompileError> {
        let clauses = &elements[1..];
        if clauses.is_empty() {
            self.emit_constant(ConstantValue::Nil, line);
            return Ok(());
        }

        let mut end_jumps = Vec::new();

        for clause_expr in clauses {
            let clause = ast::list_to_array(clause_expr);
            if clause.len() != 2 {
                return Err(CompileError::new("cond clause must be (test expr)", line));
            }

            let test_expr = clause[0];
            let result_expr = clause[1];

            // Check for else clause
            if let Some(sym) = Self::get_symbol(test_expr) {
                if sym == self.known_symbols.else_ {
                    self.compile_expr(result_expr, is_tail)?;
                    end_jumps.push(self.emit_jump(Op::Jump, line));
                    break;
                }
            }

            self.compile_expr(test_expr, false)?;
            let jump_to_next = self.emit_jump(Op::JumpIfFalse, line);
            self.compile_expr(result_expr, is_tail)?;
            end_jumps.push(self.emit_jump(Op::Jump, line));
            self.patch_jump(jump_to_next);
        }

        // Default: nil if no else
        self.emit_constant(ConstantValue::Nil, line);

        for jump in end_jumps {
            self.patch_jump(jump);
        }
        Ok(())
    }

    fn compile_match(
        &mut self,
        elements: &[&Expr],
        line: u32,
        is_tail: bool,
    ) -> Result<(), CompileError> {
        if elements.len() < 3 {
            return Err(CompileError::new(
                "match expects scrutinee and at least one branch",
                line,
            ));
        }

        // Compile scrutinee
        self.compile_expr(elements[1], false)?;

        let branches = &elements[2..];
        let mut end_jumps = Vec::new();

        for branch_expr in branches {
            let branch_elements = ast::list_to_array(branch_expr);
            if branch_elements.len() != 2 {
                return Err(CompileError::new(
                    "match branch must be (pattern body)",
                    line,
                ));
            }

            let pattern_expr = branch_elements[0];
            let body_expr = branch_elements[1];

            // DUP scrutinee so pattern can consume the copy
            self.emit(Op::Dup, line);

            // compilePattern
            let fail_patches = self.compile_pattern(pattern_expr, line)?;

            // Success path: pop original scrutinee
            self.emit(Op::Pop, line);
            self.compile_expr(body_expr, is_tail)?;
            end_jumps.push(self.emit_jump(Op::Jump, line));

            // Fail path: pop DUP'd copy
            let fail_target = self.current_pos();
            self.emit(Op::Pop, line);

            // Patch all fail jumps
            for fp in &fail_patches {
                match fp.kind {
                    FailPatchKind::Jump => {
                        self.chunk.code[fp.idx].operand = fail_target as u32;
                    }
                    FailPatchKind::MatchTag => {
                        self.chunk.code[fp.idx].operand2 = fail_target as u32;
                    }
                }
            }
        }

        // No branch matched
        self.emit(Op::Pop, line);
        self.emit_constant(
            ConstantValue::String("No pattern matched in match expression".to_string()),
            line,
        );
        self.emit(Op::Halt, line);

        for jump in end_jumps {
            self.patch_jump(jump);
        }
        Ok(())
    }

    fn compile_pattern(
        &mut self,
        pattern: &Expr,
        line: u32,
    ) -> Result<Vec<FailPatch>, CompileError> {
        match pattern {
            // Wildcard _
            Expr::Atom(LispValue::Symbol(sym), _) if *sym == self.known_symbols.wildcard => {
                self.emit(Op::Pop, line);
                Ok(vec![])
            }

            // Variable or nullary constructor
            Expr::Atom(LispValue::Symbol(sym), _) => {
                if self.is_known_constructor(*sym) {
                    // Nullary constructor pattern — use stripped name for the tag
                    let name_str = self.resolve_constructor_name(*sym);
                    let name_idx = self.add_constant(ConstantValue::String(name_str));
                    let idx = self.emit_match_tag(name_idx, line);
                    self.emit(Op::Pop, line);
                    Ok(vec![FailPatch {
                        idx,
                        kind: FailPatchKind::MatchTag,
                    }])
                } else {
                    // Variable binding
                    let slot = self.add_local(*sym);
                    self.emit_op(Op::SetLocal, line, slot);
                    Ok(vec![])
                }
            }

            // Number literal
            Expr::Atom(LispValue::Number(n, _), _) => {
                self.emit(Op::Dup, line);
                self.emit_constant(ConstantValue::Number(*n), line);
                self.emit(Op::Eq, line);
                let idx = self.emit_jump(Op::JumpIfFalse, line);
                self.emit(Op::Pop, line);
                Ok(vec![FailPatch {
                    idx,
                    kind: FailPatchKind::Jump,
                }])
            }

            // String literal
            Expr::Atom(LispValue::String(s), _) => {
                self.emit(Op::Dup, line);
                self.emit_constant(ConstantValue::String(s.to_string()), line);
                self.emit(Op::Eq, line);
                let idx = self.emit_jump(Op::JumpIfFalse, line);
                self.emit(Op::Pop, line);
                Ok(vec![FailPatch {
                    idx,
                    kind: FailPatchKind::Jump,
                }])
            }

            // Nil pattern ()
            Expr::Nil(_) => {
                self.emit(Op::Dup, line);
                self.emit_constant(ConstantValue::Nil, line);
                self.emit(Op::Eq, line);
                let idx = self.emit_jump(Op::JumpIfFalse, line);
                self.emit(Op::Pop, line);
                Ok(vec![FailPatch {
                    idx,
                    kind: FailPatchKind::Jump,
                }])
            }

            // Constructor pattern: (CtorName args...)
            Expr::List { .. } => {
                let pattern_elements = ast::list_to_array(pattern);
                if pattern_elements.is_empty() {
                    // Empty list = nil pattern
                    self.emit(Op::Dup, line);
                    self.emit_constant(ConstantValue::Nil, line);
                    self.emit(Op::Eq, line);
                    let idx = self.emit_jump(Op::JumpIfFalse, line);
                    self.emit(Op::Pop, line);
                    return Ok(vec![FailPatch {
                        idx,
                        kind: FailPatchKind::Jump,
                    }]);
                }

                let ctor_expr = pattern_elements[0];
                let ctor_sym = Self::get_symbol(ctor_expr).ok_or_else(|| {
                    CompileError::new("Pattern constructor must be a symbol", line)
                })?;

                let ctor_name = self.resolve_constructor_name(ctor_sym);
                let arg_patterns = &pattern_elements[1..];

                // MATCH_TAG
                let name_idx = self.add_constant(ConstantValue::String(ctor_name));
                let tag_fail_idx = self.emit_match_tag(name_idx, line);

                if arg_patterns.is_empty() {
                    // Nullary constructor with parens: (None)
                    self.emit(Op::Pop, line);
                    return Ok(vec![FailPatch {
                        idx: tag_fail_idx,
                        kind: FailPatchKind::MatchTag,
                    }]);
                }

                // For each arg pattern: GET_FIELD, then match the field
                let mut sub_fail_patches = Vec::new();

                for (i, arg_pattern) in arg_patterns.iter().enumerate() {
                    self.emit_op(Op::GetField, line, i as u32);
                    let sub_fails = self.compile_pattern(arg_pattern, line)?;
                    sub_fail_patches.extend(sub_fails);
                }

                // Success: pop ADT
                self.emit(Op::Pop, line);
                let success_jump = self.emit_jump(Op::Jump, line);

                // Cleanup for sub-pattern failures: pop field value
                let cleanup_pos = self.current_pos();
                self.emit(Op::Pop, line);

                // Patch sub-fails to cleanup
                for sf in &sub_fail_patches {
                    match sf.kind {
                        FailPatchKind::Jump => {
                            self.chunk.code[sf.idx].operand = cleanup_pos as u32;
                        }
                        FailPatchKind::MatchTag => {
                            self.chunk.code[sf.idx].operand2 = cleanup_pos as u32;
                        }
                    }
                }

                // Emit cleanup fail jump
                let cleanup_fail_jump = self.emit_jump(Op::Jump, line);

                // Re-patch success jump to skip cleanup
                self.chunk.code[success_jump].operand = self.current_pos() as u32;

                Ok(vec![
                    FailPatch {
                        idx: tag_fail_idx,
                        kind: FailPatchKind::MatchTag,
                    },
                    FailPatch {
                        idx: cleanup_fail_jump,
                        kind: FailPatchKind::Jump,
                    },
                ])
            }
        }
    }

    fn compile_data(&mut self, elements: &[&Expr], line: u32) -> Result<(), CompileError> {
        if elements.len() < 3 {
            return Err(CompileError::new(
                "data expects type name and at least one constructor",
                line,
            ));
        }

        let ctor_exprs = &elements[2..];

        for ctor_expr in ctor_exprs {
            match ctor_expr {
                Expr::Atom(LispValue::Symbol(sym), _) => {
                    // Nullary constructor
                    self.known_constructors.insert(*sym);
                    let name_str = self.interner.resolve(*sym).to_string();
                    let name_idx = self.add_constant(ConstantValue::String(name_str));
                    self.emit_op2(Op::Construct, line, name_idx, 0);
                    self.emit_op(Op::SetGlobal, line, name_idx);
                }
                Expr::List { .. } => {
                    let ctor_elements = ast::list_to_array(ctor_expr);
                    let first = ctor_elements
                        .first()
                        .ok_or_else(|| CompileError::new("empty constructor", line))?;
                    let ctor_sym = Self::get_symbol(first).ok_or_else(|| {
                        CompileError::new("constructor name must be a symbol", line)
                    })?;
                    let ctor_name = self.interner.resolve(ctor_sym).to_string();
                    let arity = ctor_elements.len() - 1;

                    self.known_constructors.insert(ctor_sym);

                    let name_idx = self.add_constant(ConstantValue::String(ctor_name.clone()));
                    if arity == 0 {
                        self.emit_op2(Op::Construct, line, name_idx, 0);
                        self.emit_op(Op::SetGlobal, line, name_idx);
                    } else {
                        // N-ary: store special encoding
                        let ctor_const = format!("\0ctor:{}:{}", ctor_name, arity);
                        self.emit_constant(ConstantValue::String(ctor_const), line);
                        self.emit_op(Op::SetGlobal, line, name_idx);
                    }
                }
                _ => {
                    return Err(CompileError::new(
                        "constructor must be a symbol or list",
                        line,
                    ));
                }
            }
        }

        self.emit_constant(ConstantValue::Nil, line); // data returns nil
        Ok(())
    }

    fn compile_effect(&mut self, elements: &[&Expr], line: u32) -> Result<(), CompileError> {
        if elements.len() < 2 {
            return Err(CompileError::new("effect expects a name", line));
        }
        let name_sym = Self::get_symbol(elements[1])
            .ok_or_else(|| CompileError::new("effect name must be a symbol", line))?;

        // Register operations
        for element in &elements[2..] {
            if let Expr::List { .. } = element {
                let op_elements = ast::list_to_array(element);
                if let Some(&first) = op_elements.first() {
                    if let Some(op_sym) = Self::get_symbol(first) {
                        self.known_effect_ops.insert(op_sym, name_sym);
                    }
                }
            }
        }

        self.emit_constant(ConstantValue::Nil, line);
        Ok(())
    }

    fn compile_perform(&mut self, elements: &[&Expr], line: u32) -> Result<(), CompileError> {
        if elements.len() < 2 {
            return Err(CompileError::new("perform expects an operation name", line));
        }
        let op_sym = Self::get_symbol(elements[1])
            .ok_or_else(|| CompileError::new("perform expects an operation name symbol", line))?;

        // Compile arguments
        let arg_exprs = &elements[2..];
        for arg_expr in arg_exprs {
            self.compile_expr(arg_expr, false)?;
        }

        let op_name = self.interner.resolve(op_sym).to_string();
        let name_idx = self.add_constant(ConstantValue::String(op_name));
        self.emit_op2(Op::Perform, line, name_idx, arg_exprs.len() as u32);
        Ok(())
    }

    fn compile_handle(&mut self, elements: &[&Expr], line: u32) -> Result<(), CompileError> {
        if elements.len() < 3 {
            return Err(CompileError::new(
                "handle expects an expression and handler clauses",
                line,
            ));
        }

        let body_expr = elements[1];
        let handler_clauses = &elements[2..];

        // Parse and compile handler clauses
        struct HandlerInfo {
            op_name_idx: u32,
            proto_idx: u32,
            arity: u8,
        }
        let mut handler_infos: Vec<HandlerInfo> = Vec::new();
        let mut return_proto_idx: Option<u32> = None;

        for clause in handler_clauses {
            let clause_elements = ast::list_to_array(clause);
            if clause_elements.len() != 3 {
                return Err(CompileError::new(
                    "handle clause must be ((pattern) -> body)",
                    line,
                ));
            }
            let arrow = clause_elements[1];
            if Self::get_symbol(arrow) != Some(self.known_symbols.arrow) {
                return Err(CompileError::new("handle clause must use ->", line));
            }

            let pattern_expr = clause_elements[0];
            let handler_body = clause_elements[2];
            let pattern_elements = ast::list_to_array(pattern_expr);

            if pattern_elements.is_empty() {
                return Err(CompileError::new("handler pattern cannot be empty", line));
            }

            let op_sym = Self::get_symbol(pattern_elements[0]).ok_or_else(|| {
                CompileError::new("handler pattern must start with operation name", line)
            })?;

            if op_sym == self.known_symbols.return_ {
                // Return handler
                if pattern_elements.len() != 2 {
                    return Err(CompileError::new(
                        "return handler expects one parameter",
                        line,
                    ));
                }
                let param = self.get_param_name(pattern_elements[1])?;

                let parent_scope = self.snapshot_scope();
                let mut fn_compiler = Compiler::new(
                    "<return-handler>",
                    self.interner,
                    self.known_symbols,
                    Some(parent_scope),
                    self.known_constructors.clone(),
                    self.known_effect_ops.clone(),
                );
                fn_compiler.add_local(param);
                fn_compiler.compile_expr(handler_body, true)?;
                fn_compiler.emit(Op::Return, line);
                fn_compiler.chunk.local_count = fn_compiler.local_count;

                let proto = FunctionPrototype {
                    name: "<return-handler>".to_string(),
                    arity: 1,
                    is_variadic: false,
                    upvalues: fn_compiler.upvalues,
                    chunk: fn_compiler.chunk,
                };
                return_proto_idx = Some(self.add_constant(ConstantValue::Prototype(proto)));
            } else {
                // Effect handler
                let mut params = Vec::new();
                for pe in &pattern_elements[1..] {
                    params.push(self.get_param_name(pe)?);
                }
                let cont_param = if params.is_empty() {
                    self.interner.intern("k")
                } else {
                    params.pop().unwrap()
                };

                let parent_scope = self.snapshot_scope();
                let op_name_str = self.interner.resolve(op_sym).to_string();
                let handler_name = format!("<handler:{}>", op_name_str);
                let mut fn_compiler = Compiler::new(
                    &handler_name,
                    self.interner,
                    self.known_symbols,
                    Some(parent_scope),
                    self.known_constructors.clone(),
                    self.known_effect_ops.clone(),
                );
                for &p in &params {
                    fn_compiler.add_local(p);
                }
                fn_compiler.add_local(cont_param);
                fn_compiler.compile_expr(handler_body, true)?;
                fn_compiler.emit(Op::Return, line);
                fn_compiler.chunk.local_count = fn_compiler.local_count;

                let arity = (params.len() + 1) as u8; // params + continuation
                let proto = FunctionPrototype {
                    name: handler_name,
                    arity,
                    is_variadic: false,
                    upvalues: fn_compiler.upvalues,
                    chunk: fn_compiler.chunk,
                };

                let op_name_idx = self.add_constant(ConstantValue::String(op_name_str));
                let proto_idx = self.add_constant(ConstantValue::Prototype(proto));
                handler_infos.push(HandlerInfo {
                    op_name_idx,
                    proto_idx,
                    arity,
                });
            }
        }

        // Compile body as thunk
        let parent_scope = self.snapshot_scope();
        let mut body_compiler = Compiler::new(
            "<handle-body>",
            self.interner,
            self.known_symbols,
            Some(parent_scope),
            self.known_constructors.clone(),
            self.known_effect_ops.clone(),
        );
        body_compiler.compile_expr(body_expr, true)?;
        body_compiler.emit(Op::Return, line);
        body_compiler.chunk.local_count = body_compiler.local_count;

        let body_proto = FunctionPrototype {
            name: "<handle-body>".to_string(),
            arity: 0,
            is_variadic: false,
            upvalues: body_compiler.upvalues,
            chunk: body_compiler.chunk,
        };
        let body_proto_idx = self.add_constant(ConstantValue::Prototype(body_proto));

        // Encode handler descriptor as JSON string constant
        let handlers_json: Vec<String> = handler_infos
            .iter()
            .map(|h| {
                format!(
                    "{{\"opNameIdx\":{},\"protoIdx\":{},\"arity\":{}}}",
                    h.op_name_idx, h.proto_idx, h.arity
                )
            })
            .collect();
        let return_idx_str = match return_proto_idx {
            Some(idx) => idx.to_string(),
            None => "-1".to_string(),
        };
        let descriptor = format!(
            "{{\"bodyProtoIdx\":{},\"handlers\":[{}],\"returnProtoIdx\":{}}}",
            body_proto_idx,
            handlers_json.join(","),
            return_idx_str
        );
        let descriptor_const = format!("\0handler:{}", descriptor);
        let descriptor_idx = self.add_constant(ConstantValue::String(descriptor_const));

        self.emit_op(Op::PushHandler, line, descriptor_idx);
        Ok(())
    }

    fn compile_resume(&mut self, elements: &[&Expr], line: u32) -> Result<(), CompileError> {
        if elements.len() != 3 {
            return Err(CompileError::new("resume expects 2 arguments", line));
        }
        self.compile_expr(elements[1], false)?; // continuation
        self.compile_expr(elements[2], false)?; // value
        self.emit(Op::Resume, line);
        Ok(())
    }

    // --- Quote and Quasiquote ---

    fn compile_quote(&mut self, elements: &[&Expr], line: u32) -> Result<(), CompileError> {
        if elements.len() != 2 {
            return Err(CompileError::new("quote expects exactly 1 argument", line));
        }
        self.compile_quoted_datum(elements[1], line)
    }

    fn compile_quoted_datum(&mut self, expr: &Expr, line: u32) -> Result<(), CompileError> {
        match expr {
            Expr::Atom(LispValue::Number(n, _), _) => {
                self.emit_constant(ConstantValue::Number(*n), line);
            }
            Expr::Atom(LispValue::String(s), _) => {
                self.emit_constant(ConstantValue::String(s.to_string()), line);
            }
            Expr::Atom(LispValue::Symbol(sym), _) => {
                // Push make-symbol global, push name string, call with 1 arg
                let make_sym_name =
                    self.add_constant(ConstantValue::String("make-symbol".to_string()));
                self.emit_op(Op::PushGlobal, line, make_sym_name);
                let name_str = self.interner.resolve(*sym).to_string();
                self.emit_constant(ConstantValue::String(name_str), line);
                self.emit_op(Op::Call, line, 1);
            }
            Expr::Nil(_) => {
                // Nil ADT value
                let tag_idx = self.add_constant(ConstantValue::String("Nil".to_string()));
                self.emit_op2(Op::Construct, line, tag_idx, 0);
            }
            Expr::List { head, tail, .. } => {
                self.compile_quoted_datum(head, line)?;
                self.compile_quoted_datum(tail, line)?;
                let tag_idx = self.add_constant(ConstantValue::String("Cons".to_string()));
                self.emit_op2(Op::Construct, line, tag_idx, 2);
            }
        }
        Ok(())
    }

    fn compile_quasiquote(&mut self, elements: &[&Expr], line: u32) -> Result<(), CompileError> {
        if elements.len() != 2 {
            return Err(CompileError::new(
                "quasiquote expects exactly 1 argument",
                line,
            ));
        }
        self.compile_qq(elements[1], line)
    }

    fn compile_qq(&mut self, expr: &Expr, line: u32) -> Result<(), CompileError> {
        match expr {
            Expr::List { .. } => {
                let elems = ast::list_to_array(expr);
                // Check if this is (unquote x)
                if !elems.is_empty() {
                    if let Some(sym) = Self::get_symbol(elems[0]) {
                        if sym == self.known_symbols.unquote {
                            if elems.len() != 2 {
                                return Err(CompileError::new("unquote expects 1 argument", line));
                            }
                            return self.compile_expr(elems[1], false);
                        }
                    }
                }
                // It's a list — walk the cons structure
                self.compile_qq_list(expr, line)
            }
            _ => self.compile_quoted_datum(expr, line),
        }
    }

    fn compile_qq_list(&mut self, expr: &Expr, line: u32) -> Result<(), CompileError> {
        match expr {
            Expr::Nil(_) => {
                let tag_idx = self.add_constant(ConstantValue::String("Nil".to_string()));
                self.emit_op2(Op::Construct, line, tag_idx, 0);
            }
            Expr::List { head, tail, .. } => {
                // Check if head is (unquote-splicing x)
                if let Expr::List { .. } = head.as_ref() {
                    let head_elems = ast::list_to_array(head);
                    if !head_elems.is_empty() {
                        if let Some(sym) = Self::get_symbol(head_elems[0]) {
                            if sym == self.known_symbols.unquote_splicing {
                                if head_elems.len() != 2 {
                                    return Err(CompileError::new(
                                        "unquote-splicing expects 1 argument",
                                        line,
                                    ));
                                }
                                // Push append global, compile spliced expr, compile rest, call append
                                let append_name =
                                    self.add_constant(ConstantValue::String("append".to_string()));
                                self.emit_op(Op::PushGlobal, line, append_name);
                                self.compile_expr(head_elems[1], false)?;
                                self.compile_qq_list(tail, line)?;
                                self.emit_op(Op::Call, line, 2);
                                return Ok(());
                            }
                        }
                    }
                }
                // Normal element: compile_qq(head), compile_qq_list(tail), Construct Cons
                self.compile_qq(head, line)?;
                self.compile_qq_list(tail, line)?;
                let tag_idx = self.add_constant(ConstantValue::String("Cons".to_string()));
                self.emit_op2(Op::Construct, line, tag_idx, 2);
            }
            _ => {
                // Improper tail — just quote it
                self.compile_quoted_datum(expr, line)?;
            }
        }
        Ok(())
    }

    fn compile_ffi_lib(&mut self, elements: &[&Expr], line: u32) -> Result<(), CompileError> {
        // (foreign-lib <name> <path> (<FnName> (-> <Types>...)) ...)
        if elements.len() < 3 {
            return Err(CompileError::new(
                "foreign-lib expects at least name and path",
                line,
            ));
        }

        let name_sym = Self::get_symbol(elements[1])
            .ok_or_else(|| CompileError::new("foreign-lib name must be a symbol", line))?;
        let lib_name = self.interner.resolve(name_sym).to_string();

        let path_str = match elements[2] {
            Expr::Atom(LispValue::String(s), _) => s.to_string(),
            _ => {
                return Err(CompileError::new("foreign-lib path must be a string", line));
            }
        };

        // Parse function entries and build FfiSigEntry list
        let mut sigs = Vec::new();
        let mut fn_names = Vec::new();

        for e in &elements[3..] {
            let entry_elements = ast::list_to_array(e);
            if entry_elements.len() != 2 {
                return Err(CompileError::new(
                    "foreign-lib function entry must be (FnName ArrowType)",
                    line,
                ));
            }

            let fn_sym = Self::get_symbol(entry_elements[0]).ok_or_else(|| {
                CompileError::new("foreign-lib function name must be a symbol", line)
            })?;
            let fn_name = self.interner.resolve(fn_sym).to_string();

            // Parse the arrow type from AST to extract C type strings
            let type_elements = ast::list_to_array(entry_elements[1]);
            if type_elements.is_empty() {
                return Err(CompileError::new(
                    "foreign-lib function type must be (-> ...)",
                    line,
                ));
            }

            // First element should be ->
            let first = Self::get_symbol(type_elements[0]);
            if first != Some(self.known_symbols.arrow) {
                return Err(CompileError::new(
                    "foreign-lib function type must start with ->",
                    line,
                ));
            }

            let type_names: Vec<String> = type_elements[1..]
                .iter()
                .map(|te| {
                    Self::get_symbol(te)
                        .map(|s| self.interner.resolve(s).to_string())
                        .unwrap_or_else(|| "unknown".to_string())
                })
                .collect();

            if type_names.is_empty() {
                return Err(CompileError::new(
                    "foreign-lib arrow type must have at least a return type",
                    line,
                ));
            }

            let (arg_type_names, ret_type_name) = type_names.split_at(type_names.len() - 1);

            // Convert FFI type names to C type strings
            let arg_types: Vec<String> = arg_type_names
                .iter()
                .filter(|t| t.as_str() != "Void") // Void as param means no args
                .map(|t| ffi_type_to_c_string(t))
                .collect();
            let ret_type = ffi_type_to_c_string(&ret_type_name[0]);

            sigs.push(FfiSigEntry {
                name: fn_name.clone(),
                arg_types,
                ret_type,
            });
            fn_names.push(fn_name);
        }

        // Emit: ffi-open(path, sigs) → library value
        let ffi_open_name_idx = self.add_constant(ConstantValue::String("ffi-open".to_string()));
        self.emit_op(Op::PushGlobal, line, ffi_open_name_idx);

        let path_idx = self.add_constant(ConstantValue::String(path_str));
        self.emit_op(Op::PushConst, line, path_idx);

        let sigs_idx = self.add_constant(ConstantValue::FfiSigs(sigs));
        self.emit_op(Op::PushConst, line, sigs_idx);

        self.emit_op(Op::Call, line, 2); // call ffi-open with 2 args

        // Store library as global
        let lib_name_idx = self.add_constant(ConstantValue::String(lib_name.clone()));
        self.emit_op(Op::SetGlobal, line, lib_name_idx);

        // For each function, emit FfiBound as global
        for fn_name in &fn_names {
            let bound = ConstantValue::FfiBound {
                lib_name: lib_name.clone(),
                fn_name: fn_name.clone(),
            };
            let bound_idx = self.add_constant(bound);
            self.emit_op(Op::PushConst, line, bound_idx);
            let qualified_name = format!("{}/{}", lib_name, fn_name);
            let qualified_idx = self.add_constant(ConstantValue::String(qualified_name));
            self.emit_op(Op::SetGlobal, line, qualified_idx);
        }

        // foreign-lib returns nil
        self.emit_constant(ConstantValue::Nil, line);
        Ok(())
    }

    fn compile_call(
        &mut self,
        elements: &[&Expr],
        line: u32,
        is_tail: bool,
    ) -> Result<(), CompileError> {
        // Compile function expression
        self.compile_expr(elements[0], false)?;

        // Compile arguments
        let arity = elements.len() - 1;
        for element in &elements[1..] {
            self.compile_expr(element, false)?;
        }

        if is_tail {
            self.emit_op(Op::TailCall, line, arity as u32);
        } else {
            self.emit_op(Op::Call, line, arity as u32);
        }
        Ok(())
    }

    fn compile_sequence(&mut self, exprs: &[&Expr], is_tail: bool) -> Result<(), CompileError> {
        if exprs.is_empty() {
            self.emit_constant(ConstantValue::Nil, 0);
            return Ok(());
        }
        for (i, expr) in exprs.iter().enumerate() {
            let is_last = i == exprs.len() - 1;
            if is_last {
                self.compile_expr(expr, is_tail)?;
            } else {
                self.compile_expr(expr, false)?;
                self.emit(Op::Pop, Self::get_line(expr));
            }
        }
        Ok(())
    }
}

// --- Public API ---

/// Result of compiling a single expression with external context.
/// Returns the chunk along with updated constructor/effect knowledge.
pub struct CompileResult {
    pub chunk: Chunk,
    pub known_constructors: HashSet<Symbol>,
    pub known_effect_ops: HashMap<Symbol, Symbol>,
}

/// Compile a single expression with external knowledge of constructors and effect ops.
/// Used by the module system to compile expressions one at a time while accumulating
/// knowledge across expressions and imports.
pub fn compile_expr_with_context(
    expr: &Expr,
    interner: &Interner,
    known_constructors: &HashSet<Symbol>,
    known_effect_ops: &HashMap<Symbol, Symbol>,
) -> Result<CompileResult, CompileError> {
    let ks = KnownSymbols::new(interner);
    let mut compiler = Compiler::new(
        "<script>",
        interner,
        &ks,
        None,
        known_constructors.clone(),
        known_effect_ops.clone(),
    );
    compiler.compile_expr(expr, false)?;
    compiler.emit(Op::Return, Compiler::get_line(expr));
    compiler.chunk.local_count = compiler.local_count;
    Ok(CompileResult {
        chunk: compiler.chunk,
        known_constructors: compiler.known_constructors,
        known_effect_ops: compiler.known_effect_ops,
    })
}

/// Compile a single expression to bytecode.
pub fn compile(expr: &Expr, interner: &Interner) -> Result<Chunk, CompileError> {
    let ks = KnownSymbols::new(interner);
    let default_ctors: HashSet<Symbol> = HashSet::new();

    let mut compiler = Compiler::new(
        "<script>",
        interner,
        &ks,
        None,
        default_ctors,
        HashMap::new(),
    );
    compiler.compile_expr(expr, false)?;
    compiler.emit(Op::Return, Compiler::get_line(expr));
    compiler.chunk.local_count = compiler.local_count;
    Ok(compiler.chunk)
}

/// Compile a sequence of top-level expressions to bytecode.
pub fn compile_program(exprs: &[Expr], interner: &Interner) -> Result<Chunk, CompileError> {
    let ks = KnownSymbols::new(interner);
    let default_ctors: HashSet<Symbol> = HashSet::new();

    let mut compiler = Compiler::new(
        "<script>",
        interner,
        &ks,
        None,
        default_ctors,
        HashMap::new(),
    );

    if exprs.is_empty() {
        compiler.emit_constant(ConstantValue::Nil, 0);
    } else {
        for (i, expr) in exprs.iter().enumerate() {
            let is_last = i == exprs.len() - 1;
            if is_last {
                compiler.compile_expr(expr, false)?;
            } else {
                compiler.compile_expr(expr, false)?;
                compiler.emit(Op::Pop, Compiler::get_line(expr));
            }
        }
    }

    compiler.emit(Op::Return, 0);
    compiler.chunk.local_count = compiler.local_count;
    Ok(compiler.chunk)
}
