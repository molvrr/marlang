use std::path::{Path, PathBuf};

use marlang_core::intern::Interner;
use marlang_core::lexer::lex;
use marlang_core::parser::parse;

use crate::builtins::register_builtins;
use crate::bytecode::{Chunk, ConstantValue, Op};
use crate::compiler::{compile, compile_program};
use crate::debug::disassemble;
use crate::errors::VMError;
use crate::expand::MacroExpander;
use crate::modules::{expand_macros, run_program, run_program_from_exprs, ModuleLoader};
use crate::object::VMObject;
use crate::value::{format_value, Value};
use crate::vm::VM;

// --- Test Helpers ---

fn compile_source(source: &str) -> (Chunk, Interner) {
    let interner = Interner::new();
    let tokens = lex(source, &interner).expect("lex failed");
    let exprs = parse(&tokens).expect("parse failed");
    let chunk = compile_program(&exprs, &interner).expect("compile failed");
    (chunk, interner)
}

fn compile_and_disasm(source: &str) -> String {
    let (chunk, _) = compile_source(source);
    disassemble(&chunk, None)
}

fn find_op(chunk: &Chunk, op: Op) -> bool {
    chunk.code.iter().any(|inst| inst.op == op)
}

fn count_op(chunk: &Chunk, op: Op) -> usize {
    chunk.code.iter().filter(|inst| inst.op == op).count()
}

fn find_constant_number(chunk: &Chunk, n: f64) -> bool {
    chunk
        .constants
        .iter()
        .any(|c| matches!(c, ConstantValue::Number(v) if *v == n))
}

fn find_constant_string(chunk: &Chunk, s: &str) -> bool {
    chunk
        .constants
        .iter()
        .any(|c| matches!(c, ConstantValue::String(v) if v == s))
}

// ===== Literals =====

#[test]
fn literal_number() {
    let (chunk, _) = compile_source("42");
    assert!(find_op(&chunk, Op::PushConst));
    assert!(find_constant_number(&chunk, 42.0));
}

#[test]
fn literal_float() {
    let (chunk, _) = compile_source("3.14");
    assert!(find_constant_number(&chunk, 3.14));
}

#[test]
fn literal_string() {
    let (chunk, _) = compile_source("\"hello\"");
    assert!(find_op(&chunk, Op::PushConst));
    assert!(find_constant_string(&chunk, "hello"));
}

#[test]
fn literal_boolean_true() {
    let (chunk, _) = compile_source("#t");
    assert!(chunk
        .constants
        .iter()
        .any(|c| matches!(c, ConstantValue::Boolean(true))));
}

#[test]
fn literal_boolean_false() {
    let (chunk, _) = compile_source("#f");
    assert!(chunk
        .constants
        .iter()
        .any(|c| matches!(c, ConstantValue::Boolean(false))));
}

#[test]
fn literal_nil() {
    let (chunk, _) = compile_source("nil");
    assert!(chunk
        .constants
        .iter()
        .any(|c| matches!(c, ConstantValue::Nil)));
}

#[test]
fn constant_deduplication() {
    let (chunk, _) = compile_source("(begin 42 42 42)");
    let num_count = chunk
        .constants
        .iter()
        .filter(|c| matches!(c, ConstantValue::Number(n) if *n == 42.0))
        .count();
    assert_eq!(num_count, 1, "42 should be deduplicated in constant pool");
}

// ===== Arithmetic =====

#[test]
fn binary_add() {
    let (chunk, _) = compile_source("(+ 1 2)");
    assert!(find_op(&chunk, Op::Add));
    assert!(find_constant_number(&chunk, 1.0));
    assert!(find_constant_number(&chunk, 2.0));
}

#[test]
fn binary_sub() {
    let (chunk, _) = compile_source("(- 3 1)");
    assert!(find_op(&chunk, Op::Sub));
}

#[test]
fn binary_mul() {
    let (chunk, _) = compile_source("(* 2 3)");
    assert!(find_op(&chunk, Op::Mul));
}

#[test]
fn binary_div() {
    let (chunk, _) = compile_source("(/ 10 2)");
    assert!(find_op(&chunk, Op::Div));
}

#[test]
fn unary_negate() {
    let (chunk, _) = compile_source("(- 5)");
    assert!(find_op(&chunk, Op::Neg));
    // Should NOT have Sub
    assert!(!find_op(&chunk, Op::Sub));
}

#[test]
fn nested_arithmetic() {
    let (chunk, _) = compile_source("(+ (* 2 3) (- 10 4))");
    assert!(find_op(&chunk, Op::Add));
    assert!(find_op(&chunk, Op::Mul));
    assert!(find_op(&chunk, Op::Sub));
}

// ===== Comparisons =====

#[test]
fn comparison_eq() {
    let (chunk, _) = compile_source("(= 1 1)");
    assert!(find_op(&chunk, Op::Eq));
}

#[test]
fn comparison_lt() {
    let (chunk, _) = compile_source("(< 1 2)");
    assert!(find_op(&chunk, Op::Lt));
}

#[test]
fn comparison_gt() {
    let (chunk, _) = compile_source("(> 2 1)");
    assert!(find_op(&chunk, Op::Gt));
}

#[test]
fn comparison_le() {
    let (chunk, _) = compile_source("(<= 1 2)");
    assert!(find_op(&chunk, Op::Le));
}

#[test]
fn comparison_ge() {
    let (chunk, _) = compile_source("(>= 2 1)");
    assert!(find_op(&chunk, Op::Ge));
}

// ===== Control Flow =====

#[test]
fn if_then_else() {
    let (chunk, _) = compile_source("(if #t 1 2)");
    assert!(find_op(&chunk, Op::JumpIfFalse));
    assert!(find_op(&chunk, Op::Jump));
    assert!(find_constant_number(&chunk, 1.0));
    assert!(find_constant_number(&chunk, 2.0));
}

#[test]
fn if_no_else() {
    let (chunk, _) = compile_source("(if #t 1)");
    assert!(find_op(&chunk, Op::JumpIfFalse));
    // Should have nil for the else branch
    assert!(chunk
        .constants
        .iter()
        .any(|c| matches!(c, ConstantValue::Nil)));
}

#[test]
fn cond_basic() {
    let (chunk, _) = compile_source("(cond (#t 1) (else 2))");
    assert!(find_op(&chunk, Op::JumpIfFalse));
    assert!(find_constant_number(&chunk, 1.0));
    assert!(find_constant_number(&chunk, 2.0));
}

#[test]
fn begin_sequence() {
    let (chunk, _) = compile_source("(begin 1 2 3)");
    // Intermediate values are popped
    assert!(count_op(&chunk, Op::Pop) >= 2);
    assert!(find_constant_number(&chunk, 3.0));
}

#[test]
fn nested_if() {
    let (chunk, _) = compile_source("(if #t (if #f 1 2) 3)");
    // Two JumpIfFalse instructions
    assert_eq!(count_op(&chunk, Op::JumpIfFalse), 2);
}

// ===== Variables =====

#[test]
fn define_global() {
    let (chunk, _) = compile_source("(define x 42)");
    assert!(find_op(&chunk, Op::SetGlobal));
    assert!(find_constant_string(&chunk, "x"));
    assert!(find_constant_number(&chunk, 42.0));
}

#[test]
fn define_with_type_annotation() {
    let (chunk, _) = compile_source("(define (x : Int) 42)");
    assert!(find_op(&chunk, Op::SetGlobal));
    assert!(find_constant_string(&chunk, "x"));
}

#[test]
fn global_reference() {
    let (chunk, _) = compile_source("(begin (define x 42) x)");
    assert!(find_op(&chunk, Op::PushGlobal));
}

#[test]
fn let_binding() {
    let (chunk, _) = compile_source("(let ((x 1)) x)");
    assert!(find_op(&chunk, Op::SetLocal));
    assert!(find_op(&chunk, Op::PushLocal));
}

#[test]
fn let_multiple_bindings() {
    let (chunk, _) = compile_source("(let ((x 1) (y 2)) (+ x y))");
    assert_eq!(count_op(&chunk, Op::SetLocal), 2);
    assert!(find_op(&chunk, Op::Add));
}

#[test]
fn let_with_type_annotation() {
    let (chunk, _) = compile_source("(let (((x : Int) 42)) x)");
    assert!(find_op(&chunk, Op::SetLocal));
    assert!(find_op(&chunk, Op::PushLocal));
}

// ===== Functions =====

#[test]
fn lambda_basic() {
    let (chunk, _) = compile_source("(lambda (x) x)");
    assert!(find_op(&chunk, Op::Closure));
    // Should have a prototype in constants
    assert!(chunk
        .constants
        .iter()
        .any(|c| matches!(c, ConstantValue::Prototype(_))));
}

#[test]
fn lambda_arity() {
    let (chunk, _) = compile_source("(lambda (x y) (+ x y))");
    let proto = chunk.constants.iter().find_map(|c| match c {
        ConstantValue::Prototype(p) => Some(p),
        _ => None,
    });
    assert!(proto.is_some());
    assert_eq!(proto.unwrap().arity, 2);
}

#[test]
fn function_call() {
    let (chunk, _) = compile_source("(f 1 2)");
    assert!(find_op(&chunk, Op::Call));
    // Call with arity 2
    let call = chunk.code.iter().find(|i| i.op == Op::Call).unwrap();
    assert_eq!(call.operand, 2);
}

#[test]
fn tail_call() {
    // In a lambda body (tail position), calls become tail calls
    let (chunk, _) = compile_source("(lambda (x) (f x))");
    let proto = chunk
        .constants
        .iter()
        .find_map(|c| match c {
            ConstantValue::Prototype(p) => Some(p),
            _ => None,
        })
        .unwrap();
    assert!(proto.chunk.code.iter().any(|i| i.op == Op::TailCall));
}

#[test]
fn lambda_with_typed_param() {
    let (chunk, _) = compile_source("(lambda ((x : Int)) x)");
    assert!(find_op(&chunk, Op::Closure));
    let proto = chunk
        .constants
        .iter()
        .find_map(|c| match c {
            ConstantValue::Prototype(p) => Some(p),
            _ => None,
        })
        .unwrap();
    assert_eq!(proto.arity, 1);
}

#[test]
fn upvalue_capture() {
    let (chunk, _) = compile_source("(let ((x 1)) (lambda () x))");
    // The inner lambda should have upvalues
    let proto = chunk
        .constants
        .iter()
        .find_map(|c| match c {
            ConstantValue::Prototype(p) => Some(p),
            _ => None,
        })
        .unwrap();
    assert!(!proto.upvalues.is_empty());
    // The inner lambda should emit PUSH_UPVALUE
    assert!(proto.chunk.code.iter().any(|i| i.op == Op::PushUpvalue));
}

// ===== ADTs + Patterns =====

#[test]
fn data_nullary_constructor() {
    let (chunk, _) = compile_source("(data Bool True False)");
    assert!(find_op(&chunk, Op::Construct));
    assert!(find_op(&chunk, Op::SetGlobal));
    assert!(find_constant_string(&chunk, "True"));
    assert!(find_constant_string(&chunk, "False"));
}

#[test]
fn data_nary_constructor() {
    let (chunk, _) = compile_source("(data Option (Some a) None)");
    // Some is N-ary (arity 1), encoded as special string
    assert!(chunk
        .constants
        .iter()
        .any(|c| { matches!(c, ConstantValue::String(s) if s == "\0ctor:Some:1") }));
    // None is nullary
    assert!(find_op(&chunk, Op::Construct));
}

#[test]
fn match_wildcard() {
    let (chunk, _) = compile_source("(match 42 (_ 1))");
    assert!(find_op(&chunk, Op::Dup));
    assert!(find_op(&chunk, Op::Pop));
}

#[test]
fn match_variable() {
    let (chunk, _) = compile_source("(match 42 (x x))");
    assert!(find_op(&chunk, Op::Dup));
    assert!(find_op(&chunk, Op::SetLocal));
    assert!(find_op(&chunk, Op::PushLocal));
}

#[test]
fn match_literal() {
    let (chunk, _) = compile_source("(match 42 (1 \"one\") (2 \"two\") (_ \"other\"))");
    // Should have EQ checks for literals
    assert!(count_op(&chunk, Op::Eq) >= 2);
    assert!(count_op(&chunk, Op::JumpIfFalse) >= 2);
}

#[test]
fn match_nullary_constructor() {
    let (chunk, _) =
        compile_source("(begin (data Option (Some a) None) (match None (None 1) (_ 0)))");
    assert!(find_op(&chunk, Op::MatchTag));
}

#[test]
fn match_constructor_with_args() {
    let (chunk, _) = compile_source(
        "(begin (data Option (Some a) None) (match (Some 1) ((Some x) x) (None 0)))",
    );
    assert!(find_op(&chunk, Op::MatchTag));
    assert!(find_op(&chunk, Op::GetField));
}

#[test]
fn match_no_match_halt() {
    let (chunk, _) = compile_source("(match 1 (2 \"two\"))");
    assert!(find_op(&chunk, Op::Halt));
    assert!(find_constant_string(
        &chunk,
        "No pattern matched in match expression"
    ));
}

// ===== Effects =====

#[test]
fn effect_declaration_emits_nil() {
    let (chunk, _) = compile_source("(effect IO (print (-> String Unit)))");
    // effect declarations just produce nil
    assert!(chunk
        .constants
        .iter()
        .any(|c| matches!(c, ConstantValue::Nil)));
}

#[test]
fn perform_opcode() {
    let (chunk, _) = compile_source("(perform print \"hello\")");
    assert!(find_op(&chunk, Op::Perform));
    assert!(find_constant_string(&chunk, "print"));
    // One argument
    let perform = chunk.code.iter().find(|i| i.op == Op::Perform).unwrap();
    assert_eq!(perform.operand2, 1);
}

#[test]
fn handle_produces_push_handler() {
    let source = r#"
        (handle 42
            ((return v) -> v))
    "#;
    let (chunk, _) = compile_source(source);
    assert!(find_op(&chunk, Op::PushHandler));
    // Should have handler descriptor
    assert!(chunk
        .constants
        .iter()
        .any(|c| { matches!(c, ConstantValue::String(s) if s.starts_with("\0handler:")) }));
}

#[test]
fn handle_with_effect_handler() {
    let source = r#"
        (handle (perform ask)
            ((ask k) -> (resume k 42))
            ((return v) -> v))
    "#;
    let (chunk, _) = compile_source(source);
    assert!(find_op(&chunk, Op::PushHandler));
    // Should have prototypes for body, handler, and return handler
    let proto_count = chunk
        .constants
        .iter()
        .filter(|c| matches!(c, ConstantValue::Prototype(_)))
        .count();
    assert!(
        proto_count >= 3,
        "expected at least 3 prototypes (body, handler, return), got {}",
        proto_count
    );
}

#[test]
fn resume_opcode() {
    let source = "(resume k 42)";
    let (chunk, _) = compile_source(source);
    assert!(find_op(&chunk, Op::Resume));
}

// ===== Type Forms =====

#[test]
fn class_compile_error() {
    let interner = Interner::new();
    let tokens = lex("(class Eq (a) (eq (-> a a Bool)))", &interner).expect("lex failed");
    let exprs = parse(&tokens).expect("parse failed");
    let result = compile_program(&exprs, &interner);
    assert!(result.is_err(), "class should produce a compile error");
    assert!(
        result.unwrap_err().message.contains("not supported"),
        "error should mention 'not supported'"
    );
}

#[test]
fn type_emits_nil() {
    let (chunk, _) = compile_source("(type Pair (a b) (MkPair a b))");
    // type declarations emit nil
    assert!(chunk
        .constants
        .iter()
        .any(|c| matches!(c, ConstantValue::Nil)));
}

// ===== Disassembler =====

#[test]
fn disasm_output_format() {
    let output = compile_and_disasm("42");
    assert!(output.contains("=== <script> ==="));
    assert!(output.contains("PUSH_CONST"));
    assert!(output.contains("RETURN"));
}

#[test]
fn disasm_annotations() {
    let output = compile_and_disasm("(+ 1 2)");
    // Should show constant values as annotations
    assert!(output.contains("; 1"));
    assert!(output.contains("; 2"));
}

#[test]
fn disasm_nested_prototype() {
    let output = compile_and_disasm("(lambda (x) x)");
    // Should show the nested function
    assert!(output.contains("=== <lambda> ==="));
}

// ===== Import Error =====

#[test]
fn import_fails() {
    let interner = Interner::new();
    let tokens = lex("(import \"std:io\" as io)", &interner).expect("lex failed");
    let exprs = parse(&tokens).expect("parse failed");
    let result = compile_program(&exprs, &interner);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.message.contains("import"));
}

// ===== Compile Program =====

#[test]
fn compile_program_multiple_exprs() {
    let (chunk, _) = compile_source("(define x 1) (define y 2) (+ x y)");
    // Should have POP between non-last expressions
    assert!(count_op(&chunk, Op::Pop) >= 2);
    assert!(find_op(&chunk, Op::Add));
}

#[test]
fn compile_program_empty() {
    let interner = Interner::new();
    let chunk = compile_program(&[], &interner).expect("compile failed");
    // Empty program emits nil + return
    assert!(find_op(&chunk, Op::PushConst));
    assert!(find_op(&chunk, Op::Return));
}

#[test]
fn compile_single_expr() {
    let interner = Interner::new();
    let tokens = lex("42", &interner).expect("lex failed");
    let exprs = parse(&tokens).expect("parse failed");
    let chunk = compile(&exprs[0], &interner).expect("compile failed");
    assert!(find_op(&chunk, Op::PushConst));
    assert!(find_op(&chunk, Op::Return));
    assert!(find_constant_number(&chunk, 42.0));
}

// =====================================================================
// ===== Execution Tests (Phase 4) =====================================
// =====================================================================

fn eval(source: &str) -> Result<Value, VMError> {
    let interner = Interner::new();
    let tokens = lex(source, &interner).expect("lex failed");
    let exprs = parse(&tokens).expect("parse failed");
    let loader = ModuleLoader::new();
    let expanded = expand_macros(&interner, &exprs, None, &loader).expect("expand failed");
    let mut vm = VM::new();
    register_builtins(&mut vm);
    let mut loader = ModuleLoader::new();
    run_program_from_exprs(
        &mut vm,
        &expanded.exprs,
        expanded.prelude_count,
        None,
        &mut loader,
        &interner,
    )
}

fn eval_to_string(source: &str) -> String {
    format_value(&eval(source).expect("eval failed"))
}

fn eval_with_output(source: &str) -> (Result<Value, VMError>, String) {
    let interner = Interner::new();
    let tokens = lex(source, &interner).expect("lex failed");
    let exprs = parse(&tokens).expect("parse failed");
    let loader = ModuleLoader::new();
    let expanded = expand_macros(&interner, &exprs, None, &loader).expect("expand failed");
    let mut vm = VM::new();
    register_builtins(&mut vm);
    vm.captured_output = Some(String::new());
    let mut loader = ModuleLoader::new();
    let result = run_program_from_exprs(
        &mut vm,
        &expanded.exprs,
        expanded.prelude_count,
        None,
        &mut loader,
        &interner,
    );
    let output = vm.captured_output.take().unwrap_or_default();
    (result, output)
}

fn assert_num(source: &str, expected: f64) {
    let val = eval(source).expect(&format!("eval failed for: {}", source));
    let n = val.as_number().expect(&format!(
        "expected number, got {:?} for: {}",
        format_value(&val),
        source
    ));
    assert!(
        (n - expected).abs() < 1e-10,
        "expected {} but got {} for: {}",
        expected,
        n,
        source
    );
}

fn assert_bool(source: &str, expected: bool) {
    let val = eval(source).expect(&format!("eval failed for: {}", source));
    if expected {
        assert!(val.is_true(), "expected #t for: {}", source);
    } else {
        assert!(val.is_false(), "expected #f for: {}", source);
    }
}

fn assert_str(source: &str, expected: &str) {
    let val = eval(source).expect(&format!("eval failed for: {}", source));
    let s = val.as_string().expect(&format!(
        "expected string, got {:?} for: {}",
        format_value(&val),
        source
    ));
    assert_eq!(s, expected, "for: {}", source);
}

fn assert_nil(source: &str) {
    let val = eval(source).expect(&format!("eval failed for: {}", source));
    assert!(
        val.is_nil(),
        "expected nil, got {:?} for: {}",
        format_value(&val),
        source
    );
}

fn assert_formatted(source: &str, expected: &str) {
    let result = eval_to_string(source);
    assert_eq!(result, expected, "for: {}", source);
}

fn assert_output(source: &str, expected_output: &str) {
    let (result, output) = eval_with_output(source);
    result.expect(&format!("eval failed for: {}", source));
    assert_eq!(output, expected_output, "output mismatch for: {}", source);
}

fn assert_error(source: &str, substring: &str) {
    let result = eval(source);
    match result {
        Err(e) => assert!(
            e.message.contains(substring),
            "expected error containing '{}', got: {}",
            substring,
            e.message
        ),
        Ok(v) => panic!(
            "expected error containing '{}', but got: {:?}",
            substring,
            format_value(&v)
        ),
    }
}

// ===== Execution: Literals =====

#[test]
fn exec_literal_number() {
    assert_num("42", 42.0);
}

#[test]
fn exec_literal_float() {
    assert_num("3.14", 3.14);
}

#[test]
fn exec_literal_zero() {
    assert_num("0", 0.0);
}

#[test]
fn exec_literal_string() {
    assert_str("\"hello\"", "hello");
}

#[test]
fn exec_literal_true() {
    assert_bool("#t", true);
}

#[test]
fn exec_literal_false() {
    assert_bool("#f", false);
}

#[test]
fn exec_literal_nil() {
    assert_nil("nil");
}

// ===== Execution: Arithmetic =====

#[test]
fn exec_add() {
    assert_num("(+ 1 2)", 3.0);
}

#[test]
fn exec_sub() {
    assert_num("(- 10 3)", 7.0);
}

#[test]
fn exec_mul() {
    assert_num("(* 4 5)", 20.0);
}

#[test]
fn exec_div() {
    assert_num("(/ 10 2)", 5.0);
}

#[test]
fn exec_neg() {
    assert_num("(- 5)", -5.0);
}

#[test]
fn exec_nested_arithmetic() {
    assert_num("(+ (* 2 3) (- 10 4))", 12.0);
}

#[test]
fn exec_variadic_add() {
    assert_num("(+ 1 2 3 4 5)", 15.0);
}

#[test]
fn exec_negative_subtraction() {
    assert_num("(- 3 10)", -7.0);
}

#[test]
fn exec_neg_in_arithmetic() {
    assert_num("(+ (- 5) 10)", 5.0);
}

#[test]
fn exec_neg_multiply() {
    assert_num("(* (- 3) 4)", -12.0);
}

// ===== Execution: String concatenation =====

#[test]
fn exec_string_concat() {
    assert_str("(+ \"hello\" \" world\")", "hello world");
}

// ===== Execution: Comparison =====

#[test]
fn exec_lt_true() {
    assert_bool("(< 1 2)", true);
}

#[test]
fn exec_lt_false() {
    assert_bool("(< 2 1)", false);
}

#[test]
fn exec_gt() {
    assert_bool("(> 2 1)", true);
}

#[test]
fn exec_eq_true() {
    assert_bool("(= 5 5)", true);
}

#[test]
fn exec_eq_false() {
    assert_bool("(= 5 6)", false);
}

#[test]
fn exec_le_equal() {
    assert_bool("(<= 1 1)", true);
}

#[test]
fn exec_le_less() {
    assert_bool("(<= 1 2)", true);
}

#[test]
fn exec_le_false() {
    assert_bool("(<= 2 1)", false);
}

#[test]
fn exec_ge() {
    assert_bool("(>= 2 2)", true);
}

// ===== Execution: Control Flow =====

#[test]
fn exec_if_true_branch() {
    assert_num("(if #t 1 2)", 1.0);
}

#[test]
fn exec_if_false_branch() {
    assert_num("(if #f 1 2)", 2.0);
}

#[test]
fn exec_if_no_else() {
    assert_nil("(if #f 1)");
}

#[test]
fn exec_if_complex_condition() {
    assert_str("(if (< 1 2) \"yes\" \"no\")", "yes");
}

#[test]
fn exec_cond() {
    assert_num("(cond (#f 1) (#t 2) (else 3))", 2.0);
}

#[test]
fn exec_cond_else() {
    assert_num("(cond (#f 1) (#f 2) (else 3))", 3.0);
}

#[test]
fn exec_cond_first_match() {
    assert_num("(cond ((< 1 2) 10) (else 20))", 10.0);
}

#[test]
fn exec_cond_else_fallback() {
    assert_num("(cond ((> 1 2) 10) (else 20))", 20.0);
}

#[test]
fn exec_cond_no_else_nil() {
    assert_nil("(cond ((> 1 2) 10))");
}

#[test]
fn exec_cond_multiple_false() {
    assert_str(
        "(cond ((> 1 2) \"first\") ((> 2 3) \"second\") ((< 1 2) \"third\") (else \"fourth\"))",
        "third",
    );
}

#[test]
fn exec_begin_single() {
    assert_num("(begin 42)", 42.0);
}

#[test]
fn exec_begin_multiple() {
    assert_num("(begin 1 2 3)", 3.0);
}

#[test]
fn exec_begin_with_defines() {
    assert_num("(begin (define x 10) (define y 20) (+ x y))", 30.0);
}

#[test]
fn exec_begin_returns_last() {
    assert_str("(begin \"first\" \"second\" \"third\")", "third");
}

#[test]
fn exec_begin_nested() {
    assert_num("(begin (begin 1 2) (begin 3 4))", 4.0);
}

#[test]
fn exec_begin_empty() {
    assert_nil("(begin)");
}

// ===== Execution: Variables =====

#[test]
fn exec_define_and_use() {
    assert_num("(define x 42) x", 42.0);
}

#[test]
fn exec_define_with_expr() {
    assert_num("(define x (+ 1 2)) x", 3.0);
}

#[test]
fn exec_define_multiple() {
    assert_num("(define x 10) (+ x 5)", 15.0);
}

#[test]
fn exec_let_basic() {
    assert_num("(let ((x 10)) x)", 10.0);
}

#[test]
fn exec_let_multiple() {
    assert_num("(let ((x 10) (y 20)) (+ x y))", 30.0);
}

#[test]
fn exec_let_sequential() {
    assert_num("(let ((x 10) (y (+ x 5))) y)", 15.0);
}

#[test]
fn exec_let_nested() {
    assert_num("(let ((x 1)) (let ((y 2)) (+ x y)))", 3.0);
}

#[test]
fn exec_let_shadowing() {
    assert_num("(let ((x 1)) (let ((x 2)) x))", 2.0);
}

#[test]
fn exec_let_with_function() {
    assert_num("(let ((f (lambda (x) (* x 2)))) (f 5))", 10.0);
}

#[test]
fn exec_let_basic_add() {
    assert_num("(let ((a 1) (b 2)) (+ a b))", 3.0);
}

// ===== Execution: Functions =====

#[test]
fn exec_simple_lambda() {
    assert_num("((lambda (x) x) 42)", 42.0);
}

#[test]
fn exec_lambda_with_body() {
    assert_num("((lambda (x y) (+ x y)) 3 4)", 7.0);
}

#[test]
fn exec_define_function() {
    assert_num("(define add (lambda (a b) (+ a b))) (add 10 20)", 30.0);
}

#[test]
fn exec_closure() {
    assert_num(
        "(define make-adder (lambda (n) (lambda (x) (+ n x)))) (define add5 (make-adder 5)) (add5 3)",
        8.0,
    );
}

#[test]
fn exec_recursive_fib() {
    assert_num(
        "(define fib (lambda (n)
           (if (< n 2)
             n
             (+ (fib (- n 1)) (fib (- n 2))))))
         (fib 10)",
        55.0,
    );
}

#[test]
fn exec_factorial() {
    assert_num(
        "(define fact (lambda (n) (if (= n 0) 1 (* n (fact (- n 1)))))) (fact 5)",
        120.0,
    );
}

#[test]
fn exec_define_square() {
    assert_num("(define square (lambda (n) (* n n))) (square 4)", 16.0);
}

#[test]
fn exec_closures_make_adder() {
    assert_num(
        "(define make-adder (lambda (n) (lambda (x) (+ n x)))) ((make-adder 5) 10)",
        15.0,
    );
}

#[test]
fn exec_begin_in_function_body() {
    assert_num(
        "(define test (lambda () (begin (define a 1) (define b 2) (+ a b)))) (test)",
        3.0,
    );
}

// ===== Execution: Lexically-Scoped Internal Defines =====

#[test]
fn exec_recursive_local_define() {
    // Recursive define inside a lambda body (the motivating example)
    assert_num(
        "(define outer (lambda ()
           (define count-down (lambda (n)
             (if (= n 0) 0 (count-down (- n 1)))))
           (count-down 5)))
         (outer)",
        0.0,
    );
}

#[test]
fn exec_mutually_recursive_local_defines() {
    // Mutually recursive defines inside a lambda body
    assert_bool(
        "(define test (lambda ()
           (define is-even (lambda (n)
             (if (= n 0) #t (is-odd (- n 1)))))
           (define is-odd (lambda (n)
             (if (= n 0) #f (is-even (- n 1)))))
           (is-even 10)))
         (test)",
        true,
    );
}

#[test]
fn exec_mutually_recursive_local_defines_odd() {
    assert_bool(
        "(define test (lambda ()
           (define is-even (lambda (n)
             (if (= n 0) #t (is-odd (- n 1)))))
           (define is-odd (lambda (n)
             (if (= n 0) #f (is-even (- n 1)))))
           (is-odd 7)))
         (test)",
        true,
    );
}

#[test]
fn exec_simple_local_defines() {
    // Non-recursive defines inside a lambda body
    assert_num(
        "(define f (lambda ()
           (define x 10)
           (define y 20)
           (+ x y)))
         (f)",
        30.0,
    );
}

#[test]
fn exec_local_defines_dont_pollute_globals() {
    // Two functions with same-named local defines should be independent
    assert_num(
        "(define f (lambda ()
           (define local-x 42)
           local-x))
         (define g (lambda ()
           (define local-x 99)
           local-x))
         (+ (f) (g))",
        141.0,
    );
}

#[test]
fn exec_nested_lambda_local_defines() {
    // Nested lambdas each with their own local defines
    assert_num(
        "(define outer (lambda ()
           (define x 10)
           (define inner (lambda ()
             (define y 20)
             (+ x y)))
           (inner)))
         (outer)",
        30.0,
    );
}

#[test]
fn exec_recursive_local_define_in_begin() {
    // Recursive define inside begin inside lambda
    assert_num(
        "(define test (lambda ()
           (begin
             (define count-down (lambda (n)
               (if (= n 0) 0 (count-down (- n 1)))))
             (count-down 3))))
         (test)",
        0.0,
    );
}

#[test]
fn exec_local_define_fibonacci() {
    // Local recursive fibonacci
    assert_num(
        "(define test (lambda ()
           (define fib (lambda (n)
             (if (< n 2) n (+ (fib (- n 1)) (fib (- n 2))))))
           (fib 10)))
         (test)",
        55.0,
    );
}

#[test]
fn exec_local_define_with_closure_capture() {
    // Local define that captures from the enclosing lambda's params
    assert_num(
        "(define make-counter (lambda (start)
           (define step (lambda (n)
             (if (= n 0) start (step (- n 1)))))
           (step 5)))
         (make-counter 42)",
        42.0,
    );
}

#[test]
fn exec_local_define_after_expr_errors() {
    // A define that appears after a non-define expression should be rejected
    assert_error(
        "(define test (lambda ()
           (+ 1 2)
           (define x 10)
           x))
         (test)",
        "define 'x' is not allowed here",
    );
}

#[test]
fn exec_local_define_after_expr_in_begin_errors() {
    // Same rule applies inside begin blocks within lambdas
    assert_error(
        "(define test (lambda ()
           (begin
             (+ 1 2)
             (define y 20)
             y)))
         (test)",
        "define 'y' is not allowed here",
    );
}

// ===== Execution: Tail Call Optimization =====

#[test]
fn exec_tco_loop() {
    assert_num(
        "(define loop (lambda (n acc)
           (if (= n 0)
             acc
             (loop (- n 1) (+ acc 1)))))
         (loop 100000 0)",
        100000.0,
    );
}

#[test]
fn exec_tco_fib() {
    assert_num(
        "(define fib-tail (lambda (n a b)
           (if (= n 0)
             a
             (fib-tail (- n 1) b (+ a b)))))
         (fib-tail 30 0 1)",
        832040.0,
    );
}

// ===== Execution: ADTs =====

#[test]
fn exec_adt_none() {
    assert_formatted("None", "None");
}

#[test]
fn exec_adt_some() {
    assert_formatted("(Some 42)", "(Some 42)");
}

#[test]
fn exec_match_wildcard() {
    assert_num("(match 42 (_ 1))", 1.0);
}

#[test]
fn exec_match_variable() {
    assert_num("(match 42 (x x))", 42.0);
}

#[test]
fn exec_match_number_literal_1() {
    assert_str("(match 1 (1 \"one\") (2 \"two\") (_ \"other\"))", "one");
}

#[test]
fn exec_match_number_literal_2() {
    assert_str("(match 2 (1 \"one\") (2 \"two\") (_ \"other\"))", "two");
}

#[test]
fn exec_match_number_literal_3() {
    assert_str("(match 3 (1 \"one\") (2 \"two\") (_ \"other\"))", "other");
}

#[test]
fn exec_match_some() {
    assert_num("(match (Some 42) ((Some x) x) (None 0))", 42.0);
}

#[test]
fn exec_match_none() {
    assert_num("(match None ((Some x) x) (None 0))", 0.0);
}

#[test]
fn exec_user_data_type() {
    assert_str(
        "(data Color Red Green Blue)
         (match Green
           (Red \"red\")
           (Green \"green\")
           (Blue \"blue\"))",
        "green",
    );
}

#[test]
fn exec_list() {
    assert_formatted("(list 1 2 3)", "(1 2 3)");
}

#[test]
fn exec_list_strings() {
    assert_formatted("(list \"a\" \"b\" \"c\")", "(\"a\" \"b\" \"c\")");
}

// ===== Execution: Show =====

#[test]
fn exec_show_integer() {
    assert_str("(show 42)", "42");
}

#[test]
fn exec_show_true() {
    assert_str("(show (< 1 2))", "#t");
}

#[test]
fn exec_show_false() {
    assert_str("(show (> 1 2))", "#f");
}

#[test]
fn exec_show_string() {
    assert_str("(show \"hello\")", "\"hello\"");
}

#[test]
fn exec_show_list() {
    assert_str("(show (list 1 2 3))", "(1 2 3)");
}

#[test]
fn exec_show_nil() {
    assert_str("(show nil)", "nil");
}

// ===== Execution: Not =====

#[test]
fn exec_not_true() {
    assert_bool("(not #t)", false);
}

#[test]
fn exec_not_false() {
    assert_bool("(not #f)", true);
}

#[test]
fn exec_not_nil() {
    assert_bool("(not nil)", true);
}

#[test]
fn exec_not_number() {
    assert_bool("(not 42)", false);
}

// ===== Execution: String Operations =====

#[test]
fn exec_string_length() {
    assert_num("(string-length \"hello\")", 5.0);
}

#[test]
fn exec_string_append() {
    assert_str("(string-append \"hello\" \" \" \"world\")", "hello world");
}

#[test]
fn exec_number_to_string() {
    assert_str("(number->string 42)", "42");
}

#[test]
fn exec_string_to_number() {
    assert_num("(string->number \"42\")", 42.0);
}

#[test]
fn exec_string_to_number_invalid() {
    // string->number now throws on invalid input via (perform throw ...)
    // With CPS dispatch, unhandled throw gives "Uncaught exception: ..." instead of "Unhandled effect: throw"
    assert_error(
        "(string->number \"abc\")",
        "Uncaught exception: string->number: invalid number: abc",
    );
}

#[test]
fn exec_string_to_number_caught() {
    // string->number throws can be caught with handle
    assert_str(
        r#"(handle (begin (string->number "abc") "ok")
             ((throw msg k) -> msg))"#,
        "string->number: invalid number: abc",
    );
}

// ===== Execution: Effects — Handle/Perform =====

#[test]
fn exec_handle_returns_body() {
    assert_num(
        "(handle 42
           ((op k) -> 0))",
        42.0,
    );
}

#[test]
fn exec_handle_with_return_handler() {
    assert_num(
        "(handle 42
           ((return x) -> (+ x 1)))",
        43.0,
    );
}

#[test]
fn exec_simple_perform_and_handle() {
    assert_num(
        "(handle
           (+ 1 (perform get))
           ((get k) -> (resume k 10)))",
        11.0,
    );
}

#[test]
fn exec_perform_with_argument() {
    assert_num(
        "(handle
           (perform put 42)
           ((put x k) -> x))",
        42.0,
    );
}

#[test]
fn exec_handler_not_resuming() {
    assert_num(
        "(handle
           (begin
             (perform abort)
             100)
           ((abort k) -> 0))",
        0.0,
    );
}

#[test]
fn exec_multiple_performs() {
    assert_num(
        "(handle
           (+ (perform get) (perform get))
           ((get k) -> (resume k 5)))",
        10.0,
    );
}

#[test]
fn exec_perform_in_function() {
    assert_num(
        "(define double (lambda (x) (* x 2)))
         (handle
           (double (perform get))
           ((get k) -> (resume k 7)))",
        14.0,
    );
}

#[test]
fn exec_conditional_with_perform() {
    assert_num(
        "(handle
           (if (perform ask)
             100
             0)
           ((ask k) -> (resume k #t)))",
        100.0,
    );
}

#[test]
fn exec_perform_in_let() {
    assert_num(
        "(handle
           (let ((x (perform get)))
             (* x x))
           ((get k) -> (resume k 4)))",
        16.0,
    );
}

#[test]
fn exec_return_handler_with_perform() {
    assert_num(
        "(handle
           (perform get)
           ((get k) -> (resume k 10))
           ((return x) -> (* x 2)))",
        20.0,
    );
}

#[test]
fn exec_nested_handlers() {
    assert_num(
        "(handle
           (handle
             (+ (perform get) (perform inc))
             ((get k) -> (resume k 5)))
           ((inc k) -> (resume k 1)))",
        6.0,
    );
}

#[test]
fn exec_handler_no_continuation_param() {
    assert_num(
        "(handle
           (begin (perform log) 42)
           ((log k) -> 0))",
        0.0,
    );
}

#[test]
fn exec_effect_declaration_nil() {
    assert_nil("(effect Console (print (-> String Unit)))");
}

// ===== Execution: Default IO Handlers =====

#[test]
fn exec_print_returns_nil() {
    let (result, output) = eval_with_output("(perform print \"hello\")");
    assert!(result.unwrap().is_nil());
    assert_eq!(output, "hello");
}

#[test]
fn exec_println_returns_nil() {
    let (result, output) = eval_with_output("(perform println \"world\")");
    assert!(result.unwrap().is_nil());
    assert_eq!(output, "world\n");
}

// ===== Execution: Error handling =====

#[test]
fn exec_error_builtin() {
    assert_error("(error \"something went wrong\")", "something went wrong");
}

#[test]
fn exec_undefined_variable() {
    assert_error("nonexistent", "Undefined variable");
}

#[test]
fn exec_no_match_error() {
    assert_error("(match 1 (2 \"two\"))", "No pattern matched");
}

#[test]
fn exec_arity_mismatch() {
    assert_error(
        "(define f (lambda (x) x)) (f 1 2)",
        "Expected 1 arguments, got 2",
    );
}

// ===== Execution: Type forms (no-ops) =====

#[test]
fn exec_type_nil() {
    assert_nil("(type Pair (a b) (MkPair a b))");
}

// ===== Execution: Float operators =====

#[test]
fn exec_float_add() {
    assert_num("(+. 2.5 3.0)", 5.5);
}

#[test]
fn exec_float_mul() {
    assert_num("(*. 2.0 3.0)", 6.0);
}

#[test]
fn exec_float_sub() {
    assert_num("(-. 10.0 3.0)", 7.0);
}

#[test]
fn exec_float_div() {
    assert_num("(/. 10.0 2.0)", 5.0);
}

// ===== Execution: Polymorphic equality =====

#[test]
fn exec_equal_true() {
    assert_bool("(equal? 1 1)", true);
}

#[test]
fn exec_equal_false() {
    assert_bool("(equal? 1 2)", false);
}

// ===== Execution: Conversion =====

#[test]
fn exec_int_to_float() {
    assert_num("(int->float 42)", 42.0);
}

// ===== Execution: String comparison =====

#[test]
fn exec_string_eq() {
    assert_bool("(string= \"hello\" \"hello\")", true);
}

#[test]
fn exec_string_lt() {
    assert_bool("(string< \"a\" \"b\")", true);
}

#[test]
fn exec_string_gt() {
    assert_bool("(string> \"b\" \"a\")", true);
}

#[test]
fn exec_string_le() {
    assert_bool("(string<= \"a\" \"a\")", true);
}

#[test]
fn exec_string_ge() {
    assert_bool("(string>= \"b\" \"a\")", true);
}

// ===== Execution: Complex programs =====

#[test]
fn exec_map_function() {
    assert_formatted(
        "(define map (lambda (f lst)
           (match lst
             (Nil Nil)
             ((Cons x rest) (Cons (f x) (map f rest))))))
         (map (lambda (x) (* x 2)) (Cons 1 (Cons 2 (Cons 3 Nil))))",
        "(Cons 2 (Cons 4 (Cons 6 Nil)))",
    );
}

#[test]
fn exec_fold_function() {
    assert_num(
        "(define fold (lambda (f acc lst)
           (match lst
             (Nil acc)
             ((Cons x rest) (fold f (f acc x) rest)))))
         (fold (lambda (acc x) (+ acc x)) 0 (Cons 1 (Cons 2 (Cons 3 Nil))))",
        6.0,
    );
}

#[test]
fn exec_mutual_recursion() {
    assert_bool(
        "(define even? (lambda (n) (if (= n 0) #t (odd? (- n 1)))))
         (define odd? (lambda (n) (if (= n 0) #f (even? (- n 1)))))
         (even? 10)",
        true,
    );
}

#[test]
fn exec_higher_order() {
    assert_num(
        "(define apply-twice (lambda (f x) (f (f x))))
         (apply-twice (lambda (x) (+ x 1)) 5)",
        7.0,
    );
}

// ===== Execution: Effect — Stateful counter =====

#[test]
fn exec_stateful_counter() {
    // A counter that increments on each `tick` and returns final count
    assert_num(
        "(handle
           (begin
             (perform tick)
             (perform tick)
             (perform tick)
             (perform done))
           ((tick k) -> (+ 1 (resume k nil)))
           ((done k) -> 0))",
        3.0,
    );
}

// ===== Execution: Effect — Exception-like abort =====

#[test]
fn exec_effect_abort() {
    assert_str(
        "(handle
           (begin
             (perform abort \"early exit\")
             \"unreachable\")
           ((abort msg k) -> msg))",
        "early exit",
    );
}

// ===== Execution: Effect — State =====

#[test]
fn exec_effect_state() {
    assert_num(
        "(handle
           (begin
             (perform put 10)
             (+ (perform get) 5))
           ((get k) -> (resume k 10))
           ((put v k) -> (resume k nil)))",
        15.0,
    );
}

// ===== Execution: MkPair =====

#[test]
fn exec_mkpair() {
    assert_formatted("(MkPair 1 2)", "(MkPair 1 2)");
}

#[test]
fn exec_match_mkpair() {
    assert_num("(match (MkPair 3 4) ((MkPair a b) (+ a b)))", 7.0);
}

// ===== Execution: Ok/Err =====

#[test]
fn exec_ok() {
    assert_formatted("(Ok 42)", "(Ok 42)");
}

#[test]
fn exec_err() {
    assert_formatted("(Err \"fail\")", "(Err \"fail\")");
}

#[test]
fn exec_match_result() {
    assert_num("(match (Ok 10) ((Ok v) v) ((Err e) 0))", 10.0);
}

#[test]
fn exec_match_err() {
    assert_num("(match (Err \"fail\") ((Ok v) v) ((Err e) 0))", 0.0);
}

// ===== One-shot continuation enforcement =====

#[test]
fn exec_continuation_one_shot_enforced() {
    // Store continuation in a variable and try to resume it twice — second should fail
    let source = r#"
(effect Ask (ask (-> Unit)))
(define saved nil)
(handle
  (begin
    (perform ask)
    (perform ask)
    42)
  ((ask k) -> (if (= saved nil)
      (begin
        (define saved k)
        (resume k nil))
      (resume saved nil))))
"#;
    assert_error(source, "one-shot");
}

// =====================================================================
// ===== CPS Handler Dispatch Tests ====================================
// =====================================================================

#[test]
fn cps_handle_throw_with_println() {
    // Issue #4 repro: handle/throw where handler clause does (perform println ...)
    // Previously this failed because the Rust call stack was unwound by the scheduler
    // when println suspended, losing the handler dispatch context.
    assert_output(
        r#"(handle
             (begin
               (perform println "before throw")
               (perform throw "oops")
               "unreachable")
             ((throw msg k) ->
               (begin
                 (perform println (string-append "caught: " msg))
                 "handled")))"#,
        "before throw\ncaught: oops\n",
    );
}

#[test]
fn cps_deep_handler_recatch() {
    // Deep handler semantics: handler re-catches effects from resumed continuations
    assert_num(
        "(handle
           (+ (perform get) (perform get))
           ((get k) -> (resume k 10)))",
        20.0,
    );
}

#[test]
fn cps_throw_unhandled_crashes_with_message() {
    // Unhandled throw gives "Uncaught exception: ..." not "Unhandled effect: throw"
    assert_error(
        r#"(perform throw "something went wrong")"#,
        "Uncaught exception: something went wrong",
    );
}

#[test]
fn cps_throw_caught_by_user_handler() {
    // throw can be caught by a user handle block
    assert_str(
        r#"(handle
             (begin (perform throw "err") "unreachable")
             ((throw msg k) -> (string-append "caught: " msg)))"#,
        "caught: err",
    );
}

#[test]
fn cps_nested_handlers_inner_catches() {
    // Nested handlers: inner handler catches its effect, outer is unaffected
    assert_num(
        "(handle
           (handle
             (+ (perform get) 1)
             ((get k) -> (resume k 100)))
           ((get k) -> (resume k 0)))",
        101.0,
    );
}

#[test]
fn cps_nested_handlers_outer_catches() {
    // Nested handlers: inner doesn't handle the effect, outer catches it
    assert_str(
        r#"(handle
           (handle
             (perform ask)
             ((put v k) -> "inner"))
           ((ask k) -> (resume k "outer")))"#,
        "outer",
    );
}

#[test]
fn cps_nested_handlers_propagation() {
    // Effect not handled by inner handler propagates to outer
    assert_num(
        "(handle
           (handle
             (perform get)
             ((put v k) -> (resume k nil)))
           ((get k) -> (resume k 42)))",
        42.0,
    );
}

#[test]
fn cps_handler_with_io_in_clause() {
    // Handler clause that uses IO effects (println) - the core Issue #4 scenario
    assert_output(
        r#"(handle
             (begin
               (perform throw "test error")
               "unreachable")
             ((throw msg k) ->
               (begin
                 (perform println (string-append "Error: " msg))
                 "done")))"#,
        "Error: test error\n",
    );
}

#[test]
fn cps_stateful_counter() {
    // Accumulation via handler wrapping (key correctness test for deep semantics)
    assert_num(
        "(handle
           (begin
             (perform tick)
             (perform tick)
             (perform tick)
             (perform get-count))
           ((tick k) -> (resume k nil))
           ((get-count k) -> (resume k 3)))",
        3.0,
    );
}

// =====================================================================
// ===== Module Tests (Phase 5a) =======================================
// =====================================================================

fn fixtures_dir() -> PathBuf {
    let manifest = env!("CARGO_MANIFEST_DIR");
    Path::new(manifest)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tests")
        .join("fixtures")
        .join("modules")
}

fn eval_with_modules(source: &str, file_path: &str) -> Result<Value, VMError> {
    let interner = Interner::new();
    let mut vm = VM::new();
    register_builtins(&mut vm);
    let mut loader = ModuleLoader::new();

    let path = fixtures_dir().join(file_path);
    run_program(&mut vm, source, Some(&path), &mut loader, &interner)
}

fn eval_with_virtual_modules(
    source: &str,
    file_path: &str,
    virtual_modules: Vec<(&str, &str)>,
) -> Result<Value, VMError> {
    let interner = Interner::new();
    let mut vm = VM::new();
    register_builtins(&mut vm);
    let mut loader = ModuleLoader::new();

    let base = fixtures_dir();
    for (vpath, vsource) in virtual_modules {
        loader.register_virtual(base.join(vpath), vsource);
    }

    let path = base.join(file_path);
    run_program(&mut vm, source, Some(&path), &mut loader, &interner)
}

#[allow(dead_code)]
fn eval_modules_with_output(source: &str, file_path: &str) -> (Result<Value, VMError>, String) {
    let interner = Interner::new();
    let mut vm = VM::new();
    register_builtins(&mut vm);
    vm.captured_output = Some(String::new());
    let mut loader = ModuleLoader::new();

    let path = fixtures_dir().join(file_path);
    let result = run_program(&mut vm, source, Some(&path), &mut loader, &interner);
    let output = vm.captured_output.take().unwrap_or_default();
    (result, output)
}

fn assert_module_num(source: &str, file_path: &str, expected: f64) {
    let val = eval_with_modules(source, file_path)
        .expect(&format!("eval failed for module test: {}", source));
    let n = val
        .as_number()
        .expect(&format!("expected number, got {:?}", format_value(&val)));
    assert!(
        (n - expected).abs() < 1e-10,
        "expected {} but got {}",
        expected,
        n
    );
}

fn assert_module_formatted(source: &str, file_path: &str, expected: &str) {
    let val = eval_with_modules(source, file_path)
        .expect(&format!("eval failed for module test: {}", source));
    let formatted = format_value(&val);
    assert_eq!(formatted, expected, "for: {}", source);
}

fn assert_module_error(source: &str, file_path: &str, substring: &str) {
    let result = eval_with_modules(source, file_path);
    match result {
        Err(e) => assert!(
            e.message.contains(substring),
            "expected error containing '{}', got: {}",
            substring,
            e.message
        ),
        Ok(v) => panic!(
            "expected error containing '{}', but got: {:?}",
            substring,
            format_value(&v)
        ),
    }
}

// ===== Import All =====

#[test]
fn module_import_all_bindings() {
    assert_module_num(
        r#"(import "./math.mar")
           (square 5)"#,
        "test.mar",
        25.0,
    );
}

#[test]
fn module_import_all_multiple() {
    assert_module_num(
        r#"(import "./math.mar")
           (add (square 2) (double 3))"#,
        "test.mar",
        10.0,
    );
}

// ===== Selective Import =====

#[test]
fn module_import_selective() {
    assert_module_num(
        r#"(import "./math.mar" (square))
           (square 6)"#,
        "test.mar",
        36.0,
    );
}

#[test]
fn module_import_selective_multiple() {
    assert_module_num(
        r#"(import "./math.mar" (square double))
           (+ (square 3) (double 4))"#,
        "test.mar",
        17.0,
    );
}

#[test]
fn module_import_selective_nonexistent() {
    assert_module_error(
        r#"(import "./math.mar" (nonexistent))"#,
        "test.mar",
        "does not export",
    );
}

// ===== Qualified Import =====

#[test]
fn module_import_qualified() {
    assert_module_num(
        r#"(import "./math.mar" as m)
           (m/square 7)"#,
        "test.mar",
        49.0,
    );
}

#[test]
fn module_import_qualified_multiple() {
    assert_module_num(
        r#"(import "./math.mar" as math)
           (math/add (math/square 2) (math/double 5))"#,
        "test.mar",
        14.0,
    );
}

// ===== Qualified Import — Namespace Isolation =====

#[test]
fn module_qualified_no_namespace_collision() {
    // Two modules defining the same name should be independently accessible
    // via qualified names. This was the Known Issue #2 bug: first-imported wins.
    let val = eval_with_virtual_modules(
        r#"(import "./a.mar" as a)
           (import "./b.mar" as b)
           (+ (a/compute 3) (b/compute 3))"#,
        "test.mar",
        vec![
            ("a.mar", "(define compute (lambda (x) (* x x)))"), // square: 9
            ("b.mar", "(define compute (lambda (x) (* x 2)))"), // double: 6
        ],
    )
    .expect("namespace collision test failed");
    let n = val.as_number().expect("expected number");
    assert!((n - 15.0).abs() < 1e-10, "expected 15 (9 + 6), got {}", n);
}

#[test]
fn module_qualified_internal_cross_reference() {
    // Module functions calling other functions from the same module should still work
    // since PushGlobal resolves against the closure's home module.
    let val = eval_with_virtual_modules(
        r#"(import "./mod.mar" as m)
           (m/cube 3)"#,
        "test.mar",
        vec![(
            "mod.mar",
            r#"(define square (lambda (x) (* x x)))
                           (define cube (lambda (x) (* x (square x))))"#,
        )],
    )
    .expect("cross-reference test failed");
    let n = val.as_number().expect("expected number");
    assert!((n - 27.0).abs() < 1e-10, "expected 27, got {}", n);
}

#[test]
fn module_qualified_no_unqualified_leak() {
    // After a qualified import, the unqualified name should NOT be available
    // in the importing module (the old workaround installed them).
    let result = eval_with_virtual_modules(
        r#"(import "./mod.mar" as m)
           (compute 3)"#,
        "test.mar",
        vec![("mod.mar", "(define compute (lambda (x) (* x x)))")],
    );
    match result {
        Err(e) => assert!(
            e.message.contains("Undefined variable"),
            "expected 'Undefined variable' error, got: {}",
            e.message
        ),
        Ok(v) => panic!(
            "expected error (unqualified 'compute' should not be visible), got: {:?}",
            format_value(&v)
        ),
    }
}

// ===== Path Resolution =====

#[test]
fn module_import_auto_extension() {
    assert_module_num(
        r#"(import "./math")
           (square 3)"#,
        "test.mar",
        9.0,
    );
}

// ===== Circular Dependency =====

#[test]
fn module_import_circular_detection() {
    assert_module_error(
        r#"(import "./circular-a.mar")"#,
        "test.mar",
        "Circular dependency",
    );
}

// ===== Module Caching (Diamond) =====

#[test]
fn module_import_caching_diamond() {
    // Both test.mar and uses-math.mar import math.mar — should only load once
    assert_module_num(
        r#"(import "./math.mar")
           (import "./uses-math.mar")
           (+ (square 2) (quad 2))"#,
        "test.mar",
        20.0,
    );
}

// ===== Data Types =====

#[test]
fn module_import_data_types() {
    assert_module_formatted(
        r#"(import "./types.mar")
           (map-result (lambda (x) (* x 2)) (Ok 3))"#,
        "test.mar",
        "(Ok 6)",
    );
}

// ===== Effects =====

#[test]
fn module_import_effects() {
    assert_module_num(
        r#"(import "./effects.mar")
           (define counter 0)
           (handle
             (count-up 3)
             ((inc k) -> (begin
               (define counter (+ counter 1))
               (resume k counter)))
             ((get k) -> (resume k counter)))"#,
        "test.mar",
        3.0,
    );
}

// ===== Error Cases =====

#[test]
fn module_import_nonexistent() {
    assert_module_error(r#"(import "./nonexistent.mar")"#, "test.mar", "not found");
}

#[test]
fn module_import_no_file_context() {
    let interner = Interner::new();
    let mut vm = VM::new();
    register_builtins(&mut vm);
    let mut loader = ModuleLoader::new();
    let result = run_program(
        &mut vm,
        r#"(import "./math.mar")"#,
        None,
        &mut loader,
        &interner,
    );
    match result {
        Err(e) => assert!(
            e.message.contains("file context"),
            "expected 'file context' error, got: {}",
            e.message
        ),
        Ok(v) => panic!("expected error, got: {:?}", format_value(&v)),
    }
}

// ===== run_program without imports =====

#[test]
fn module_run_program_basic() {
    let interner = Interner::new();
    let mut vm = VM::new();
    register_builtins(&mut vm);
    let mut loader = ModuleLoader::new();
    let result =
        run_program(&mut vm, "(+ 1 2)", None, &mut loader, &interner).expect("run_program failed");
    assert_eq!(result.as_number(), Some(3.0));
}

#[test]
fn module_run_program_multiple_exprs() {
    let interner = Interner::new();
    let mut vm = VM::new();
    register_builtins(&mut vm);
    let mut loader = ModuleLoader::new();
    let result = run_program(
        &mut vm,
        "(define x 10) (define y 20) (+ x y)",
        None,
        &mut loader,
        &interner,
    )
    .expect("run_program failed");
    assert_eq!(result.as_number(), Some(30.0));
}

#[test]
fn module_run_program_effects() {
    let interner = Interner::new();
    let mut vm = VM::new();
    register_builtins(&mut vm);
    vm.captured_output = Some(String::new());
    let mut loader = ModuleLoader::new();
    let _result = run_program(
        &mut vm,
        r#"(perform println "hello from run_program")"#,
        None,
        &mut loader,
        &interner,
    )
    .expect("run_program failed");
    let output = vm.captured_output.take().unwrap_or_default();
    assert_eq!(output, "hello from run_program\n");
}

// ===== Stdlib Tests =====

#[test]
fn stdlib_option_map_some() {
    let interner = Interner::new();
    let mut vm = VM::new();
    register_builtins(&mut vm);
    let mut loader = ModuleLoader::new();
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let test_file = workspace.join("stdlib").join("_test.mar");
    let result = run_program(
        &mut vm,
        r#"(import "std:option" as opt)
           (opt/map (lambda (x) (* x 2)) (Some 5))"#,
        Some(&test_file),
        &mut loader,
        &interner,
    )
    .expect("stdlib option test failed");
    assert_eq!(format_value(&result), "(Some 10)");
}

#[test]
fn stdlib_option_map_none() {
    let interner = Interner::new();
    let mut vm = VM::new();
    register_builtins(&mut vm);
    let mut loader = ModuleLoader::new();
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let test_file = workspace.join("stdlib").join("_test.mar");
    let result = run_program(
        &mut vm,
        r#"(import "std:option" as opt)
           (opt/map (lambda (x) (* x 2)) None)"#,
        Some(&test_file),
        &mut loader,
        &interner,
    )
    .expect("stdlib option test failed");
    assert_eq!(format_value(&result), "None");
}

#[test]
fn stdlib_option_unwrap_or() {
    let interner = Interner::new();
    let mut vm = VM::new();
    register_builtins(&mut vm);
    let mut loader = ModuleLoader::new();
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let test_file = workspace.join("stdlib").join("_test.mar");
    let result = run_program(
        &mut vm,
        r#"(import "std:option" as opt)
           (+ (opt/unwrap-or 0 (Some 42)) (opt/unwrap-or 99 None))"#,
        Some(&test_file),
        &mut loader,
        &interner,
    )
    .expect("stdlib option test failed");
    assert_eq!(result.as_number(), Some(141.0));
}

#[test]
fn stdlib_option_is_some_is_none() {
    let interner = Interner::new();
    let mut vm = VM::new();
    register_builtins(&mut vm);
    let mut loader = ModuleLoader::new();
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let test_file = workspace.join("stdlib").join("_test.mar");
    let result = run_program(
        &mut vm,
        r#"(import "std:option" as opt)
           (list (opt/is-some (Some 1)) (opt/is-none (Some 1)) (opt/is-some None) (opt/is-none None))"#,
        Some(&test_file),
        &mut loader,
        &interner,
    )
    .expect("stdlib option test failed");
    assert_eq!(format_value(&result), "(#t #f #f #t)");
}

#[test]
fn stdlib_result_map_ok() {
    let interner = Interner::new();
    let mut vm = VM::new();
    register_builtins(&mut vm);
    let mut loader = ModuleLoader::new();
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let test_file = workspace.join("stdlib").join("_test.mar");
    let result = run_program(
        &mut vm,
        r#"(import "std:result" as res)
           (res/map (lambda (x) (* x 3)) (res/Ok 5))"#,
        Some(&test_file),
        &mut loader,
        &interner,
    )
    .expect("stdlib result test failed");
    assert_eq!(format_value(&result), "(Ok 15)");
}

#[test]
fn stdlib_result_map_err() {
    let interner = Interner::new();
    let mut vm = VM::new();
    register_builtins(&mut vm);
    let mut loader = ModuleLoader::new();
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let test_file = workspace.join("stdlib").join("_test.mar");
    let result = run_program(
        &mut vm,
        r#"(import "std:result" as res)
           (res/map (lambda (x) (* x 3)) (res/Err "oops"))"#,
        Some(&test_file),
        &mut loader,
        &interner,
    )
    .expect("stdlib result test failed");
    assert_eq!(format_value(&result), "(Err \"oops\")");
}

#[test]
fn stdlib_result_is_ok_is_err() {
    let interner = Interner::new();
    let mut vm = VM::new();
    register_builtins(&mut vm);
    let mut loader = ModuleLoader::new();
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let test_file = workspace.join("stdlib").join("_test.mar");
    let result = run_program(
        &mut vm,
        r#"(import "std:result" as res)
           (list (res/is-ok (res/Ok 1)) (res/is-err (res/Ok 1)) (res/is-ok (res/Err "e")) (res/is-err (res/Err "e")))"#,
        Some(&test_file),
        &mut loader,
        &interner,
    )
    .expect("stdlib result test failed");
    assert_eq!(format_value(&result), "(#t #f #f #t)");
}

#[test]
fn stdlib_result_unwrap_or() {
    let interner = Interner::new();
    let mut vm = VM::new();
    register_builtins(&mut vm);
    let mut loader = ModuleLoader::new();
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let test_file = workspace.join("stdlib").join("_test.mar");
    let result = run_program(
        &mut vm,
        r#"(import "std:result" as res)
           (+ (res/unwrap-or 0 (res/Ok 10)) (res/unwrap-or 99 (res/Err "fail")))"#,
        Some(&test_file),
        &mut loader,
        &interner,
    )
    .expect("stdlib result test failed");
    assert_eq!(result.as_number(), Some(109.0));
}

#[test]
fn stdlib_io_println_effect() {
    let interner = Interner::new();
    let mut vm = VM::new();
    register_builtins(&mut vm);
    vm.captured_output = Some(String::new());
    let mut loader = ModuleLoader::new();
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let test_file = workspace.join("stdlib").join("_test.mar");
    let _result = run_program(
        &mut vm,
        r#"(import "std:io" as io)
           (io/println "hello from stdlib")"#,
        Some(&test_file),
        &mut loader,
        &interner,
    )
    .expect("stdlib io test failed");
    let output = vm.captured_output.take().unwrap_or_default();
    assert_eq!(output, "hello from stdlib\n");
}

// =====================================================================
// ===== FFI Tests (Phase 5b) ==========================================
// =====================================================================

#[cfg(feature = "ffi")]
#[test]
fn ffi_call_cos() {
    // cos(0.0) → 1.0
    let source = r#"
(define lib (ffi-open "m" (list (list "cos" (list "f64") "f64"))))
(ffi-call lib "cos" 0.0)
"#;
    assert_num(source, 1.0);
}

#[cfg(feature = "ffi")]
#[test]
fn ffi_call_sqrt() {
    // sqrt(16.0) → 4.0
    let source = r#"
(define lib (ffi-open "m" (list (list "sqrt" (list "f64") "f64"))))
(ffi-call lib "sqrt" 16.0)
"#;
    assert_num(source, 4.0);
}

#[cfg(feature = "ffi")]
#[test]
fn ffi_multiple_sigs() {
    // sin(0) + cos(0) + pow(2,3) → 0 + 1 + 8 = 9
    let source = r#"
(define lib (ffi-open "m" (list
  (list "sin" (list "f64") "f64")
  (list "cos" (list "f64") "f64")
  (list "pow" (list "f64" "f64") "f64"))))
(+ (+ (ffi-call lib "sin" 0.0) (ffi-call lib "cos" 0.0)) (ffi-call lib "pow" 2.0 3.0))
"#;
    assert_num(source, 9.0);
}

#[cfg(feature = "ffi")]
#[test]
fn ffi_struct_roundtrip() {
    let source = r#"
(define point-def (ffi-struct "Point" (list (list "x" "f64") (list "y" "f64"))))
(define p (struct-new point-def 3.0 4.0))
(+ (struct-get p "x") (struct-get p "y"))
"#;
    assert_num(source, 7.0);
}

#[cfg(feature = "ffi")]
#[test]
fn ffi_struct_set() {
    let source = r#"
(define point-def (ffi-struct "Point" (list (list "x" "f64") (list "y" "f64"))))
(define p (struct-new point-def 1.0 2.0))
(define p2 (struct-set p "x" 10.0))
(+ (struct-get p2 "x") (struct-get p2 "y"))
"#;
    assert_num(source, 12.0);
}

#[cfg(feature = "ffi")]
#[test]
fn ffi_close_library() {
    let source = r#"
(define lib (ffi-open "m" (list (list "cos" (list "f64") "f64"))))
(ffi-close lib)
"#;
    assert_nil(source);
}

#[cfg(feature = "ffi")]
#[test]
fn ffi_call_three_args() {
    // fma(2.0, 3.0, 4.0) → 2*3+4 = 10.0
    let source = r#"
(define lib (ffi-open "m" (list (list "fma" (list "f64" "f64" "f64") "f64"))))
(ffi-call lib "fma" 2.0 3.0 4.0)
"#;
    assert_num(source, 10.0);
}

#[cfg(feature = "ffi")]
#[test]
fn ffi_void_with_args() {
    // Call a void-returning function that takes args (srand takes u32)
    // On macOS/Linux, srand is in libc
    let source = r#"
(define lib (ffi-open "c" (list (list "srand" (list "u32") "void"))))
(ffi-call lib "srand" 42)
"#;
    assert_nil(source);
}

#[cfg(feature = "ffi")]
#[test]
fn ffi_cstring_arg() {
    // strlen("hello") → 5
    let source = r#"
(define lib (ffi-open "c" (list (list "strlen" (list "cstring") "u64"))))
(ffi-call lib "strlen" "hello")
"#;
    assert_num(source, 5.0);
}

#[cfg(feature = "ffi")]
#[test]
fn ffi_void_arg_skipped() {
    // (list "void") should be treated as no args, matching TS behavior
    let source = r#"
(define lib (ffi-open "m" (list (list "cos" (list "f64") "f64")
                                (list "drand48" (list "void") "f64"))))
(define r (ffi-call lib "drand48"))
(>= r 0.0)
"#;
    assert_bool(source, true);
}

#[cfg(feature = "ffi")]
#[test]
fn ffi_f32_roundtrip() {
    // floorf(3.7) → 3.0 (tests f32 arg and f32 return)
    let source = r#"
(define lib (ffi-open "m" (list (list "floorf" (list "f32") "f32"))))
(ffi-call lib "floorf" 3.7)
"#;
    assert_num(source, 3.0);
}

#[cfg(feature = "ffi")]
#[test]
fn ffi_i32_return() {
    // abs(-42) → 42
    let source = r#"
(define lib (ffi-open "c" (list (list "abs" (list "i32") "i32"))))
(ffi-call lib "abs" -42)
"#;
    assert_num(source, 42.0);
}

#[cfg(feature = "ffi")]
#[test]
fn ffi_struct_type_in_parse() {
    // Unknown type names should be treated as struct types (not error)
    let source = r#"
(define point-def (ffi-struct "Point" (list (list "x" "f32") (list "y" "f32"))))
(define lib (ffi-open "m" (list (list "cos" (list "f64") "f64"))))
(ffi-call lib "cos" 0.0)
"#;
    assert_num(source, 1.0);
}

// ===== Concurrency - Fibers =====

#[test]
fn fiber_spawn_returns_pid() {
    let (result, output) = eval_with_output(
        r#"
(run-fibers (lambda ()
  (let ((pid (perform fiber-spawn (lambda () nil))))
    (perform println (show pid)))))
"#,
    );
    assert!(result.is_ok());
    assert!(output.contains("#<pid"));
}

#[test]
fn fiber_self_returns_pid() {
    let (result, output) = eval_with_output(
        r#"
(run-fibers (lambda ()
  (let ((me (perform fiber-self)))
    (perform println (show me)))))
"#,
    );
    assert!(result.is_ok());
    assert!(output.contains("#<pid 0>"));
}

#[test]
fn fiber_spawn_runs_to_completion() {
    let (result, output) = eval_with_output(
        r#"
(run-fibers (lambda ()
  (perform fiber-spawn (lambda ()
    (perform println "child")))
  (perform println "main")))
"#,
    );
    assert!(result.is_ok());
    assert!(output.contains("main"));
    assert!(output.contains("child"));
}

#[test]
fn fiber_yield_interleaves() {
    let (result, output) = eval_with_output(
        r#"
(run-fibers (lambda ()
  (perform fiber-spawn (lambda ()
    (perform println "B1")
    (perform fiber-yield)
    (perform println "B2")))
  (perform println "A1")
  (perform fiber-yield)
  (perform println "A2")))
"#,
    );
    assert!(result.is_ok());
    assert!(output.find("A1").unwrap() < output.find("A2").unwrap());
    assert!(output.find("B1").unwrap() < output.find("B2").unwrap());
}

#[test]
fn fiber_multiple_run_to_completion() {
    let (result, output) = eval_with_output(
        r#"
(run-fibers (lambda ()
  (perform fiber-spawn (lambda () (perform println "f1")))
  (perform fiber-spawn (lambda () (perform println "f2")))
  (perform fiber-spawn (lambda () (perform println "f3")))
  (perform println "main")))
"#,
    );
    assert!(result.is_ok());
    assert!(output.contains("f1"));
    assert!(output.contains("f2"));
    assert!(output.contains("f3"));
    assert!(output.contains("main"));
}

#[test]
fn fiber_crash_does_not_kill_main() {
    let (result, output) = eval_with_output(
        r#"
(run-fibers (lambda ()
  (perform fiber-spawn (lambda () (error "boom")))
  (perform fiber-yield)
  (perform println "main survived")))
"#,
    );
    assert!(result.is_ok());
    assert!(output.contains("main survived"));
}

// ===== Concurrency - Channels =====

#[test]
fn channel_buffered_send_recv() {
    let (result, output) = eval_with_output(
        r#"
(define unwrap-some (lambda (opt)
  (match opt ((Some v) v) (None (error "unwrap-some: got None")))))

(run-fibers (lambda ()
  (let ((ch (perform chan-new 10)))
    (perform chan-send ch 42)
    (let ((v (unwrap-some (perform chan-recv ch))))
      (perform println v)))))
"#,
    );
    assert!(result.is_ok());
    assert_eq!(output.trim(), "42");
}

#[test]
fn channel_unbuffered_synchronizes() {
    let (result, output) = eval_with_output(
        r#"
(define unwrap-some (lambda (opt)
  (match opt ((Some v) v) (None (error "unwrap-some: got None")))))

(run-fibers (lambda ()
  (let ((ch (perform chan-new 0)))
    (perform fiber-spawn (lambda ()
      (perform chan-send ch 99)))
    (let ((v (unwrap-some (perform chan-recv ch))))
      (perform println v)))))
"#,
    );
    assert!(result.is_ok());
    assert_eq!(output.trim(), "99");
}

#[test]
fn channel_producer_consumer() {
    let (result, output) = eval_with_output(
        r#"
(run-fibers (lambda ()
  (let ((ch (perform chan-new 0)))
    (perform fiber-spawn (lambda ()
      (perform chan-send ch 1)
      (perform chan-send ch 2)
      (perform chan-send ch 3)
      (perform chan-close ch)))
    (define consume (lambda ()
      (match (perform chan-recv ch)
        (None nil)
        ((Some v)
          (begin
            (perform println v)
            (consume))))))
    (consume))))
"#,
    );
    assert!(result.is_ok());
    assert_eq!(output.trim(), "1\n2\n3");
}

#[test]
fn channel_close_wakes_receivers() {
    let (result, output) = eval_with_output(
        r#"
(run-fibers (lambda ()
  (let ((ch (perform chan-new 0)))
    (perform fiber-spawn (lambda ()
      (let ((v (perform chan-recv ch)))
        (perform println (match v (None "got-none") ((Some _) "got-val"))))))
    (perform fiber-yield)
    (perform chan-close ch)
    (perform fiber-yield))))
"#,
    );
    assert!(result.is_ok());
    assert_eq!(output.trim(), "got-none");
}

#[test]
fn channel_send_closed_returns_err() {
    let (result, output) = eval_with_output(
        r#"
(run-fibers (lambda ()
  (let ((ch (perform chan-new 0)))
    (perform chan-close ch)
    (let ((r (perform chan-send ch 42)))
      (perform println (match r
        ((Ok _) "ok")
        ((Err msg) (string-append "err:" msg))))))))
"#,
    );
    assert!(result.is_ok(), "chan-send closed test failed: {:?}", result);
    assert_eq!(output.trim(), "err:channel is closed");
}

#[test]
fn channel_fan_out() {
    let (result, output) = eval_with_output(
        r#"
(define unwrap-some (lambda (opt)
  (match opt ((Some v) v) (None (error "unwrap-some: got None")))))

(run-fibers (lambda ()
  (let ((ch (perform chan-new 10)))
    (perform fiber-spawn (lambda () (perform chan-send ch (+ 1 1))))
    (perform fiber-spawn (lambda () (perform chan-send ch (+ 2 2))))
    (perform fiber-spawn (lambda () (perform chan-send ch (+ 3 3))))
    (let ((a (unwrap-some (perform chan-recv ch)))
          (b (unwrap-some (perform chan-recv ch)))
          (c (unwrap-some (perform chan-recv ch))))
      (perform println (+ a (+ b c)))))))
"#,
    );
    assert!(result.is_ok());
    assert_eq!(output.trim(), "12");
}

// ===== Concurrency - Actors =====

#[test]
fn actor_send_receive_basic() {
    let (result, output) = eval_with_output(
        r#"
(run-fibers (lambda ()
  (let ((me (perform fiber-self)))
    (perform fiber-spawn (lambda ()
      (perform actor-send me "hello")))
    (let ((msg (perform actor-receive)))
      (perform println msg)))))
"#,
    );
    assert!(result.is_ok());
    assert_eq!(output.trim(), "hello");
}

#[test]
fn actor_receive_blocks_until_message() {
    let (result, output) = eval_with_output(
        r#"
(run-fibers (lambda ()
  (let ((me (perform fiber-self)))
    (perform fiber-spawn (lambda ()
      (perform fiber-yield)
      (perform actor-send me "delayed")))
    (let ((msg (perform actor-receive)))
      (perform println msg)))))
"#,
    );
    assert!(result.is_ok());
    assert_eq!(output.trim(), "delayed");
}

#[test]
fn actor_selective_receive() {
    let (result, output) = eval_with_output(
        r#"
(data Msg (A Int) (B Int))
(run-fibers (lambda ()
  (let ((me (perform fiber-self)))
    (perform fiber-spawn (lambda ()
      (perform actor-send me (A 1))
      (perform actor-send me (B 2))
      (perform actor-send me (A 3))))
    (perform fiber-yield)
    (perform fiber-yield)
    (let ((msg (perform actor-receive-match
                 (lambda (m)
                   (match m
                     ((B _) #t)
                     (_ #f))))))
      (match msg
        ((B n) (perform println n)))))))
"#,
    );
    assert!(result.is_ok());
    assert_eq!(output.trim(), "2");
}

// ===== Concurrency - Supervision =====

#[test]
fn monitor_delivers_down_on_crash() {
    let (result, output) = eval_with_output(
        r#"
(data Signal (Down Pid String) (Exit Pid String))
(run-fibers (lambda ()
  (let ((worker (perform fiber-spawn (lambda ()
                  (perform fiber-yield)
                  (error "boom")))))
    (perform actor-monitor worker)
    (perform fiber-yield)
    (perform fiber-yield)
    (perform fiber-yield)
    (let ((msg (perform actor-receive)))
      (match msg
        ((Down pid reason) (perform println reason)))))))
"#,
    );
    assert!(result.is_ok());
    assert_eq!(output.trim(), "boom");
}

#[test]
fn monitor_delivers_down_normal_on_clean_exit() {
    let (result, output) = eval_with_output(
        r#"
(data Signal (Down Pid String) (Exit Pid String))
(run-fibers (lambda ()
  (let ((worker (perform fiber-spawn (lambda () nil))))
    (perform actor-monitor worker)
    (perform fiber-yield)
    (perform fiber-yield)
    (let ((msg (perform actor-receive)))
      (match msg
        ((Down pid reason) (perform println reason)))))))
"#,
    );
    assert!(result.is_ok());
    assert_eq!(output.trim(), "normal");
}

#[test]
fn trap_exit_converts_crash_to_message() {
    let (result, output) = eval_with_output(
        r#"
(data Signal (Down Pid String) (Exit Pid String))
(run-fibers (lambda ()
  (perform actor-trap-exit #t)
  (let ((worker (perform fiber-spawn (lambda ()
                  (perform fiber-yield)
                  (error "crash")))))
    (perform actor-link worker)
    (perform fiber-yield)
    (perform fiber-yield)
    (perform fiber-yield)
    (let ((msg (perform actor-receive)))
      (match msg
        ((Exit pid reason) (perform println (+ "caught: " reason))))))))
"#,
    );
    assert!(result.is_ok());
    assert_eq!(output.trim(), "caught: crash");
}

// ===== Concurrency - IO inside run-fibers =====

#[test]
fn io_println_inside_run_fibers() {
    let (result, output) = eval_with_output(
        r#"
(run-fibers (lambda ()
  (perform println "hello from fibers")))
"#,
    );
    assert!(result.is_ok());
    assert_eq!(output.trim(), "hello from fibers");
}

#[test]
fn io_multiple_fibers_printing() {
    let (result, output) = eval_with_output(
        r#"
(run-fibers (lambda ()
  (perform fiber-spawn (lambda () (perform println "fiber1")))
  (perform fiber-spawn (lambda () (perform println "fiber2")))
  (perform println "main")))
"#,
    );
    assert!(result.is_ok());
    assert!(output.contains("main"));
    assert!(output.contains("fiber1"));
    assert!(output.contains("fiber2"));
}

#[test]
fn io_file_read_inside_fibers() {
    let tmp_path = "/tmp/marlang-rust-test-read.txt";
    std::fs::write(tmp_path, "test content").unwrap();
    let source = format!(
        r#"
(define unwrap-ok (lambda (r)
  (match r ((Ok v) v) ((Err msg) (error msg)))))

(run-fibers (lambda ()
  (let ((content (unwrap-ok (perform read-file "{}"))))
    (perform println content))))
"#,
        tmp_path
    );
    let (result, output) = eval_with_output(&source);
    std::fs::remove_file(tmp_path).ok();
    assert!(result.is_ok());
    assert_eq!(output.trim(), "test content");
}

#[test]
fn io_concurrent_file_reads() {
    let tmp1 = "/tmp/marlang-rust-cr1.txt";
    let tmp2 = "/tmp/marlang-rust-cr2.txt";
    std::fs::write(tmp1, "file1").unwrap();
    std::fs::write(tmp2, "file2").unwrap();
    let source = format!(
        r#"
(define unwrap-some (lambda (opt)
  (match opt ((Some v) v) (None (error "unwrap-some: got None")))))
(define unwrap-ok (lambda (r)
  (match r ((Ok v) v) ((Err msg) (error msg)))))

(run-fibers (lambda ()
  (let ((ch (perform chan-new 10)))
    (perform fiber-spawn (lambda ()
      (perform chan-send ch (unwrap-ok (perform read-file "{}")))))
    (perform fiber-spawn (lambda ()
      (perform chan-send ch (unwrap-ok (perform read-file "{}")))))
    (let ((a (unwrap-some (perform chan-recv ch)))
          (b (unwrap-some (perform chan-recv ch))))
      (perform println a)
      (perform println b)))))
"#,
        tmp1, tmp2
    );
    let (result, output) = eval_with_output(&source);
    std::fs::remove_file(tmp1).ok();
    std::fs::remove_file(tmp2).ok();
    assert!(result.is_ok());
    assert!(output.contains("file1"));
    assert!(output.contains("file2"));
}

// ===== Concurrency - Edge Cases =====

#[test]
fn concurrency_main_result_returned() {
    assert_num(
        r#"
(run-fibers (lambda () 42))
"#,
        42.0,
    );
}

#[test]
fn io_outside_run_fibers_still_works() {
    let (result, output) = eval_with_output(
        r#"
(perform println "sync io")
"#,
    );
    assert!(result.is_ok());
    assert!(output.contains("sync io"));
}

#[test]
fn counter_actor_pattern() {
    let (result, output) = eval_with_output(
        r#"
(data CounterMsg
  (Increment Int)
  (GetCount Pid))
(data CounterReply
  (Count Int))

(define counter (lambda (count)
  (let ((msg (perform actor-receive)))
    (match msg
      ((Increment n) (counter (+ count n)))
      ((GetCount sender)
        (begin
          (perform actor-send sender (Count count))
          (counter count)))))))

(run-fibers (lambda ()
  (let ((c (perform fiber-spawn (lambda () (counter 0))))
        (me (perform fiber-self)))
    (perform actor-send c (Increment 5))
    (perform actor-send c (Increment 3))
    (perform actor-send c (GetCount me))
    (let ((reply (perform actor-receive)))
      (match reply
        ((Count n) (perform println n)))))))
"#,
    );
    assert!(result.is_ok());
    assert_eq!(output.trim(), "8");
}

#[test]
fn ping_pong_pattern() {
    let (result, output) = eval_with_output(
        r#"
(data PingPong (Ping Pid) (Pong Pid))

(define pong (lambda ()
  (let ((msg (perform actor-receive)))
    (match msg
      ((Ping sender)
        (let ((me (perform fiber-self)))
          (perform actor-send sender (Pong me))
          (pong)))))))

(define ping (lambda (partner n)
  (if (<= n 0)
    (perform println "done")
    (let ((me (perform fiber-self)))
      (perform actor-send partner (Ping me))
      (let ((msg (perform actor-receive)))
        (match msg
          ((Pong _) (ping partner (- n 1)))))))))

(run-fibers (lambda ()
  (let ((p (perform fiber-spawn pong)))
    (ping p 100))))
"#,
    );
    assert!(result.is_ok());
    assert_eq!(output.trim(), "done");
}

#[test]
fn fiber_crash_isolation() {
    // Multiple child fibers crash, main survives
    let (result, output) = eval_with_output(
        r#"
(run-fibers (lambda ()
  (perform fiber-spawn (lambda () (error "crash1")))
  (perform fiber-spawn (lambda () (error "crash2")))
  (perform fiber-yield)
  (perform fiber-yield)
  (perform println "survived")))
"#,
    );
    assert!(result.is_ok());
    assert!(output.contains("survived"));
}

// ===== Parallel Scheduler (multi-threaded) =====

#[test]
fn parallel_main_result_returned() {
    assert_num(
        r#"
(run-fibers-parallel (lambda () 42) 2)
"#,
        42.0,
    );
}

#[test]
fn parallel_spawn_returns_pid() {
    let val = eval(
        r#"
(run-fibers-parallel (lambda ()
  (let ((pid (perform fiber-spawn (lambda () 99))))
    pid)) 2)
"#,
    )
    .expect("eval failed");
    // Should be a pid
    assert!(matches!(val.as_object(), Some(VMObject::Pid(_))));
}

#[test]
fn parallel_basic_println() {
    let (result, output) = eval_with_output(
        r#"
(run-fibers-parallel (lambda ()
  (perform println "hello from parallel")) 2)
"#,
    );
    assert!(result.is_ok());
    assert_eq!(output.trim(), "hello from parallel");
}

#[test]
fn parallel_multiple_fibers_printing() {
    let (result, output) = eval_with_output(
        r#"
(run-fibers-parallel (lambda ()
  (perform fiber-spawn (lambda () (perform println "fiber1")))
  (perform fiber-spawn (lambda () (perform println "fiber2")))
  (perform println "main")) 2)
"#,
    );
    assert!(result.is_ok());
    assert!(output.contains("main"));
    assert!(output.contains("fiber1"));
    assert!(output.contains("fiber2"));
}

#[test]
fn parallel_fib_two_fibers() {
    // Spawn two fib(20) computations, verify both complete
    let (result, output) = eval_with_output(
        r#"
(define fib (lambda (n)
  (if (< n 2) n
    (+ (fib (- n 1)) (fib (- n 2))))))

(define unwrap-some (lambda (opt)
  (match opt ((Some v) v) (None (error "unwrap-some: got None")))))

(run-fibers-parallel (lambda ()
  (let ((ch (perform chan-new 10)))
    (perform fiber-spawn (lambda ()
      (perform chan-send ch (fib 20))))
    (perform fiber-spawn (lambda ()
      (perform chan-send ch (fib 20))))
    (let ((a (unwrap-some (perform chan-recv ch)))
          (b (unwrap-some (perform chan-recv ch))))
      (perform println (string-append "results: " (show a) " " (show b)))))) 2)
"#,
    );
    assert!(result.is_ok());
    assert!(output.contains("results: 6765 6765"));
}

#[test]
fn parallel_cross_thread_channel() {
    // Producer/consumer across threads
    let (result, output) = eval_with_output(
        r#"
(define unwrap-some (lambda (opt)
  (match opt ((Some v) v) (None (error "unwrap-some: got None")))))

(run-fibers-parallel (lambda ()
  (let ((ch (perform chan-new 0)))
    (perform fiber-spawn (lambda ()
      (perform chan-send ch "hello")
      (perform chan-send ch "world")))
    (let ((a (unwrap-some (perform chan-recv ch)))
          (b (unwrap-some (perform chan-recv ch))))
      (perform println (string-append a " " b))))) 2)
"#,
    );
    assert!(result.is_ok());
    assert_eq!(output.trim(), "hello world");
}

#[test]
fn parallel_actor_send_receive() {
    let (result, output) = eval_with_output(
        r#"
(run-fibers-parallel (lambda ()
  (let ((worker (perform fiber-spawn (lambda ()
                  (let ((msg (perform actor-receive)))
                    (perform println (+ "got: " msg)))))))
    (perform actor-send worker "ping")
    (perform fiber-yield)
    (perform fiber-yield)
    (perform fiber-yield))) 2)
"#,
    );
    assert!(result.is_ok());
    assert_eq!(output.trim(), "got: ping");
}

#[test]
fn parallel_many_fibers() {
    // Spawn several fibers via recursive helper, collect results through channel
    let (result, output) = eval_with_output(
        r#"
(define unwrap-some (lambda (opt)
  (match opt ((Some v) v) (None (error "unwrap-some: got None")))))

(define spawn-workers (lambda (ch n)
  (if (<= n 0) nil
    (begin
      (perform fiber-spawn (lambda () (perform chan-send ch 1)))
      (spawn-workers ch (- n 1))))))

(define collect (lambda (ch n acc)
  (if (<= n 0) acc
    (let ((v (unwrap-some (perform chan-recv ch))))
      (collect ch (- n 1) (+ acc v))))))

(run-fibers-parallel (lambda ()
  (let ((ch (perform chan-new 100)))
    (spawn-workers ch 10)
    (let ((total (collect ch 10 0)))
      (perform println total)))) 2)
"#,
    );
    assert!(result.is_ok());
    assert_eq!(output.trim(), "10");
}

#[test]
fn parallel_actor_ping_pong() {
    let (result, output) = eval_with_output(
        r#"
(data PingPong (Ping Pid) (Pong Pid))

(define pong-loop (lambda ()
  (let ((msg (perform actor-receive)))
    (match msg
      ((Ping sender)
        (let ((me (perform fiber-self)))
          (perform actor-send sender (Pong me))
          (pong-loop)))))))

(define ping-loop (lambda (partner n)
  (if (<= n 0)
    (perform println "done")
    (let ((me (perform fiber-self)))
      (perform actor-send partner (Ping me))
      (let ((msg (perform actor-receive)))
        (match msg
          ((Pong _) (ping-loop partner (- n 1)))))))))

(run-fibers-parallel (lambda ()
  (let ((p (perform fiber-spawn pong-loop)))
    (ping-loop p 50))) 2)
"#,
    );
    assert!(result.is_ok());
    assert_eq!(output.trim(), "done");
}

#[test]
fn parallel_monitor_cross_thread() {
    let (result, output) = eval_with_output(
        r#"
(data Signal (Down Pid String))

(run-fibers-parallel (lambda ()
  (let ((worker (perform fiber-spawn (lambda ()
                  (perform fiber-yield)
                  (error "boom")))))
    (perform actor-monitor worker)
    (perform fiber-yield)
    (perform fiber-yield)
    (perform fiber-yield)
    (let ((msg (perform actor-receive)))
      (match msg
        ((Down pid reason) (perform println (+ "down: " reason))))))) 2)
"#,
    );
    assert!(result.is_ok());
    assert_eq!(output.trim(), "down: boom");
}

#[test]
fn parallel_fiber_crash_isolation() {
    let (result, output) = eval_with_output(
        r#"
(run-fibers-parallel (lambda ()
  (perform fiber-spawn (lambda () (error "crash1")))
  (perform fiber-spawn (lambda () (error "crash2")))
  (perform fiber-yield)
  (perform fiber-yield)
  (perform println "survived")) 2)
"#,
    );
    assert!(result.is_ok());
    assert!(output.contains("survived"));
}

// ===== Async IO (tokio integration) =====

#[test]
fn async_read_file() {
    let tmp_path = "/tmp/marlang-async-read-test.txt";
    std::fs::write(tmp_path, "async content").unwrap();
    let source = format!(
        r#"
(define unwrap-ok (lambda (r)
  (match r ((Ok v) v) ((Err msg) (error msg)))))

(run-fibers-parallel (lambda ()
  (let ((content (unwrap-ok (perform read-file "{}"))))
    (perform println content))) 2)
"#,
        tmp_path
    );
    let (result, output) = eval_with_output(&source);
    std::fs::remove_file(tmp_path).ok();
    assert!(result.is_ok());
    assert_eq!(output.trim(), "async content");
}

#[test]
fn async_write_file() {
    let tmp_path = "/tmp/marlang-async-write-test.txt";
    let _ = std::fs::remove_file(tmp_path);
    let source = format!(
        r#"
(run-fibers-parallel (lambda ()
  (perform write-file "{}" "written by tokio")) 2)
"#,
        tmp_path
    );
    let result = eval(&source);
    assert!(result.is_ok());
    let content = std::fs::read_to_string(tmp_path).unwrap();
    std::fs::remove_file(tmp_path).ok();
    assert_eq!(content, "written by tokio");
}

#[test]
fn async_io_concurrent() {
    // Two fibers: one reads a file, the other counts. Both complete.
    let tmp_path = "/tmp/marlang-async-concurrent-test.txt";
    std::fs::write(tmp_path, "from-file").unwrap();
    let source = format!(
        r#"
(define unwrap-some (lambda (opt)
  (match opt ((Some v) v) (None (error "unwrap-some: got None")))))
(define unwrap-ok (lambda (r)
  (match r ((Ok v) v) ((Err msg) (error msg)))))

(run-fibers-parallel (lambda ()
  (let ((ch (perform chan-new 10)))
    (perform fiber-spawn (lambda ()
      (let ((content (unwrap-ok (perform read-file "{}"))))
        (perform chan-send ch content))))
    (perform fiber-spawn (lambda ()
      (perform chan-send ch (show (+ 1 2)))))
    (let ((a (unwrap-some (perform chan-recv ch)))
          (b (unwrap-some (perform chan-recv ch))))
      (perform println a)
      (perform println b)))) 2)
"#,
        tmp_path
    );
    let (result, output) = eval_with_output(&source);
    std::fs::remove_file(tmp_path).ok();
    assert!(result.is_ok());
    assert!(output.contains("from-file"));
    assert!(output.contains("3"));
}

#[test]
fn async_io_error() {
    // read-file on nonexistent path → fiber crashes cleanly, main survives
    let (result, output) = eval_with_output(
        r#"
(run-fibers-parallel (lambda ()
  (perform fiber-spawn (lambda ()
    (perform read-file "/tmp/marlang-no-such-file-ever.txt")))
  (perform fiber-yield)
  (perform fiber-yield)
  (perform fiber-yield)
  (perform println "main ok")) 2)
"#,
    );
    assert!(result.is_ok());
    assert!(output.contains("main ok"));
}

// ===== Quote / Symbol Tests =====

#[test]
fn quote_number() {
    assert_num("(quote 42)", 42.0);
}

#[test]
fn quote_symbol() {
    let val = eval("(quote foo)").expect("eval failed");
    match val.as_object() {
        Some(VMObject::Symbol(s)) => assert_eq!(s.as_str(), "foo"),
        other => panic!("expected symbol, got {:?}", other),
    }
}

#[test]
fn quote_list() {
    let val = eval("(quote (a b c))").expect("eval failed");
    let s = format_value(&val);
    assert_eq!(s, "(Cons a (Cons b (Cons c Nil)))");
}

#[test]
fn quote_empty_list() {
    let val = eval("(quote ())").expect("eval failed");
    let s = format_value(&val);
    assert_eq!(s, "Nil");
}

#[test]
fn quote_nested() {
    let val = eval("(quote (a (b c)))").expect("eval failed");
    // Should create nested Cons structure
    assert!(val.as_object().is_some());
}

#[test]
fn quote_sugar_number() {
    assert_num("'42", 42.0);
}

#[test]
fn quote_sugar_symbol() {
    let val = eval("'foo").expect("eval failed");
    match val.as_object() {
        Some(VMObject::Symbol(s)) => assert_eq!(s.as_str(), "foo"),
        other => panic!("expected symbol, got {:?}", other),
    }
}

#[test]
fn quote_sugar_list() {
    let val = eval("'(a b c)").expect("eval failed");
    let s = format_value(&val);
    assert_eq!(s, "(Cons a (Cons b (Cons c Nil)))");
}

// ===== Quasiquote Tests =====

#[test]
fn quasiquote_no_unquote() {
    let val = eval("`(a b c)").expect("eval failed");
    let s = format_value(&val);
    assert_eq!(s, "(Cons a (Cons b (Cons c Nil)))");
}

#[test]
fn quasiquote_unquote() {
    let val = eval("(define x 42) `(a ,x c)").expect("eval failed");
    let s = format_value(&val);
    assert_eq!(s, "(Cons a (Cons 42 (Cons c Nil)))");
}

#[test]
fn quasiquote_splicing() {
    let val = eval(
        r#"
(define xs (Cons 1 (Cons 2 Nil)))
`(a ,@xs c)
"#,
    )
    .expect("eval failed");
    let s = format_value(&val);
    assert_eq!(s, "(Cons a (Cons 1 (Cons 2 (Cons c Nil))))");
}

#[test]
fn quasiquote_nested_unquote() {
    let val = eval("(define x 10) (define y 20) `(,x ,y)").expect("eval failed");
    let s = format_value(&val);
    assert_eq!(s, "(Cons 10 (Cons 20 Nil))");
}

// ===== Builtin Tests =====

#[test]
fn cons_car_cdr() {
    assert_num("(car (cons 1 (cons 2 Nil)))", 1.0);
    assert_num("(car (cdr (cons 1 (cons 2 Nil))))", 2.0);
}

#[test]
fn append_lists() {
    let val = eval("(append (Cons 1 (Cons 2 Nil)) (Cons 3 Nil))").expect("eval failed");
    let s = format_value(&val);
    assert_eq!(s, "(Cons 1 (Cons 2 (Cons 3 Nil)))");
}

#[test]
fn symbol_predicates() {
    assert_eq!(eval_to_string("(symbol? (make-symbol \"x\"))"), "#t");
    assert_eq!(eval_to_string("(symbol? 42)"), "#f");
    assert_eq!(eval_to_string("(pair? (cons 1 Nil))"), "#t");
    assert_eq!(eval_to_string("(nil? Nil)"), "#t");
    assert_eq!(eval_to_string("(number? 3)"), "#t");
    assert_eq!(eval_to_string("(string? \"hi\")"), "#t");
}

#[test]
fn gensym_unique() {
    let val1 = eval("(gensym)").expect("eval failed");
    let val2 = eval("(gensym)").expect("eval failed");
    let s1 = format_value(&val1);
    let s2 = format_value(&val2);
    assert_ne!(s1, s2);
}

#[test]
fn symbol_string_conversion() {
    assert_eq!(
        eval_to_string("(symbol->string (make-symbol \"hello\"))"),
        "\"hello\""
    );
    let val = eval("(string->symbol \"world\")").expect("eval failed");
    match val.as_object() {
        Some(VMObject::Symbol(s)) => assert_eq!(s.as_str(), "world"),
        other => panic!("expected symbol, got {:?}", other),
    }
}

// ===== Macro Basic Tests =====

#[test]
fn defmacro_when() {
    assert_num(
        r#"
(defmacro when (cond body)
  `(if ,cond ,body nil))
(when #t 42)
"#,
        42.0,
    );
}

#[test]
fn defmacro_when_false() {
    assert_eq!(
        eval_to_string(
            r#"
(defmacro when (cond body)
  `(if ,cond ,body nil))
(when #f 42)
"#
        ),
        "nil"
    );
}

#[test]
fn defmacro_unless() {
    assert_num(
        r#"
(defmacro unless (cond body)
  `(if ,cond nil ,body))
(unless #f 99)
"#,
        99.0,
    );
}

#[test]
fn defmacro_nested_expansion() {
    assert_num(
        r#"
(defmacro when (cond body)
  `(if ,cond ,body nil))
(defmacro unless (cond body)
  `(when (not ,cond) ,body))
(unless #f 77)
"#,
        77.0,
    );
}

#[test]
fn defmacro_with_splicing() {
    assert_num(
        r#"
(defmacro my-begin (first rest)
  `(let ((unused ,first)) ,rest))
(my-begin (+ 1 2) (+ 3 4))
"#,
        7.0,
    );
}

#[test]
fn defmacro_with_data_types() {
    // Macro generates match expressions using user-defined ADTs.
    // The body doesn't reference the pattern variable — it's self-contained.
    assert_num(
        r#"
(data (MyOpt a) MyNone (MySome a))
(defmacro is-some (opt)
  `(match ,opt
    ((MySome _) 1)
    (MyNone 0)))
(is-some (MySome 42))
"#,
        1.0,
    );
}

// ===== Hygiene Tests =====

#[test]
fn macro_hygiene_no_capture() {
    // Macro introduces 'tmp' binding. Caller also has 'tmp'.
    // With hygiene, macro's tmp is distinct from caller's tmp.
    assert_num(
        r#"
(defmacro my-or (a b)
  `(let ((tmp ,a))
     (if tmp tmp ,b)))
(let ((tmp 42))
  (my-or #f tmp))
"#,
        42.0,
    );
}

#[test]
fn macro_hygiene_nested() {
    // Two expansions of the same macro should have distinct marks
    assert_num(
        r#"
(defmacro my-or (a b)
  `(let ((tmp ,a))
     (if tmp tmp ,b)))
(my-or #f (my-or #f 99))
"#,
        99.0,
    );
}

#[test]
fn macro_hygiene_global_ref() {
    // Macro template references a global function — should still resolve
    assert_num(
        r#"
(define double (lambda (x) (* x 2)))
(defmacro apply-double (x)
  `(double ,x))
(apply-double 21)
"#,
        42.0,
    );
}

#[test]
fn macro_hygiene_constructor() {
    // Macro generates match expressions with constructor patterns
    assert_num(
        r#"
(defmacro unwrap-or (opt default)
  `(match ,opt
    ((Some v) v)
    (None ,default)))
(unwrap-or (Some 42) 0)
"#,
        42.0,
    );
}

#[test]
fn macro_hygiene_constructor_none() {
    assert_num(
        r#"
(defmacro unwrap-or (opt default)
  `(match ,opt
    ((Some v) v)
    (None ,default)))
(unwrap-or None 99)
"#,
        99.0,
    );
}

// ===== Cross-Module Macro Tests =====

#[test]
fn macro_import_basic() {
    let val = eval_with_virtual_modules(
        r#"
(import "./macros.mar" as m)
(m/when #t 42)
"#,
        "test.mar",
        vec![(
            "macros.mar",
            r#"
(defmacro when (cond body)
  `(if ,cond ,body nil))
(defmacro unless (cond body)
  `(if ,cond nil ,body))
"#,
        )],
    )
    .expect("eval failed");
    assert_eq!(val.as_number(), Some(42.0));
}

#[test]
fn macro_import_hygiene() {
    // Imported macro should maintain hygiene in the importing file
    let val = eval_with_virtual_modules(
        r#"
(import "./macros.mar" as m)
(let ((tmp 42))
  (m/my-or #f tmp))
"#,
        "test.mar",
        vec![(
            "macros.mar",
            r#"
(defmacro my-or (a b)
  `(let ((tmp ,a))
     (if tmp tmp ,b)))
"#,
        )],
    )
    .expect("eval failed");
    assert_eq!(val.as_number(), Some(42.0));
}

#[test]
fn macro_import_chain() {
    // Module A defines macro, module B imports A and defines derived macro, file imports B
    let val = eval_with_virtual_modules(
        r#"
(import "./b.mar" as b)
(b/when-true 42)
"#,
        "test.mar",
        vec![
            (
                "a.mar",
                r#"
(defmacro when (cond body)
  `(if ,cond ,body nil))
"#,
            ),
            (
                "b.mar",
                r#"
(import "./a.mar" as a)
(defmacro when-true (body)
  `(a/when #t ,body))
"#,
            ),
        ],
    )
    .expect("eval failed");
    assert_eq!(val.as_number(), Some(42.0));
}

// ===== Error Cases =====

#[test]
fn defmacro_expansion_error() {
    // Macro transformer that calls error should produce an expansion-time error
    let interner = Interner::new();
    let tokens = lex(
        r#"
(defmacro bad-macro (x)
  (error "macro went wrong"))
(bad-macro 1)
"#,
        &interner,
    )
    .unwrap();
    let exprs = parse(&tokens).unwrap();
    let mut expander = MacroExpander::new(&interner);
    let result = expander.expand_all(exprs);
    assert!(result.is_err());
}

#[test]
fn defmacro_result_not_ast() {
    // If macro returns a non-convertible value, should error
    let interner = Interner::new();
    let tokens = lex(
        r#"
(defmacro bad-macro (x)
  (lambda (y) y))
(bad-macro 1)
"#,
        &interner,
    )
    .unwrap();
    let exprs = parse(&tokens).unwrap();
    let mut expander = MacroExpander::new(&interner);
    let result = expander.expand_all(exprs);
    assert!(result.is_err());
}

#[test]
fn defmacro_arity_error() {
    // Calling macro transformer with wrong arity
    let interner = Interner::new();
    let tokens = lex(
        r#"
(defmacro needs-two (a b)
  `(+ ,a ,b))
(needs-two 1)
"#,
        &interner,
    )
    .unwrap();
    let exprs = parse(&tokens).unwrap();
    let mut expander = MacroExpander::new(&interner);
    let result = expander.expand_all(exprs);
    assert!(result.is_err());
}

// ============================================================================
// TCP Networking Tests (Layer 4 — multi-threaded scheduler only)
// ============================================================================

#[test]
fn tcp_listen_and_close() {
    // Verify tcp-listen with port 0 (ephemeral) works and close cleans up
    let (result, output) = eval_with_output(
        r#"
(run-fibers-parallel (lambda ()
  (let ((listener (match (perform tcp-listen 0)
                    ((Ok l) l)
                    ((Err msg) (perform throw msg)))))
    (perform tcp-close listener)
    (perform println "listen-ok")))
  2)
"#,
    );
    assert!(result.is_ok(), "tcp listen/close failed: {:?}", result);
    assert_eq!(output.trim(), "listen-ok");
}

#[test]
fn tcp_echo_server_fixed_port() {
    // Use a high random-ish port to avoid conflicts
    let port = 19384;
    let code = format!(
        r#"
(define unwrap-ok (lambda (r label)
  (match r
    ((Ok v) v)
    ((Err msg) (perform throw (string-append label ": " msg))))))

(define unwrap-some (lambda (opt)
  (match opt
    ((Some v) v)
    (None (perform throw "unwrap-some: got None")))))

(run-fibers-parallel (lambda ()
  (let ((listener (unwrap-ok (perform tcp-listen {port}) "listen")))
    ;; Server fiber: accept one connection, echo data, close
    (perform fiber-spawn (lambda ()
      (let ((client (unwrap-some (perform tcp-accept listener))))
        (let ((data (unwrap-some (perform tcp-recv client))))
          (unwrap-ok (perform tcp-send client data) "send")
          (perform tcp-close client)))))
    ;; Client: connect, send, recv, print
    (perform fiber-yield)
    (perform fiber-yield)
    (let ((sock (unwrap-ok (perform tcp-connect "127.0.0.1" {port}) "connect")))
      (unwrap-ok (perform tcp-send sock "hello") "send")
      (let ((resp (unwrap-some (perform tcp-recv sock))))
        (perform println resp)
        (perform tcp-close sock)))
    (perform tcp-close listener)))
  2)
"#
    );
    let (result, output) = eval_with_output(&code);
    assert!(result.is_ok(), "tcp echo failed: {:?}", result);
    assert_eq!(output.trim(), "hello");
}

#[test]
fn tcp_recv_returns_none_on_close() {
    let port = 19385;
    let code = format!(
        r#"
(define unwrap-ok (lambda (r label)
  (match r
    ((Ok v) v)
    ((Err msg) (perform throw (string-append label ": " msg))))))

(define unwrap-some (lambda (opt)
  (match opt
    ((Some v) v)
    (None (perform throw "unwrap-some: got None")))))

(run-fibers-parallel (lambda ()
  (let ((listener (unwrap-ok (perform tcp-listen {port}) "listen")))
    ;; Server: accept and immediately close
    (perform fiber-spawn (lambda ()
      (let ((client (unwrap-some (perform tcp-accept listener))))
        (perform tcp-close client))))
    (perform fiber-yield)
    (perform fiber-yield)
    ;; Client: connect, recv should get None
    (let ((sock (unwrap-ok (perform tcp-connect "127.0.0.1" {port}) "connect")))
      (let ((data (perform tcp-recv sock)))
        (perform println (match data (None "closed") ((Some _) "data"))))
      (perform tcp-close sock))
    (perform tcp-close listener)))
  2)
"#
    );
    let (result, output) = eval_with_output(&code);
    assert!(result.is_ok(), "tcp close test failed: {:?}", result);
    assert_eq!(output.trim(), "closed");
}

#[test]
fn tcp_send_and_receive_data() {
    let port = 19386;
    let code = format!(
        r#"
(define unwrap-ok (lambda (r label)
  (match r
    ((Ok v) v)
    ((Err msg) (perform throw (string-append label ": " msg))))))

(define unwrap-some (lambda (opt)
  (match opt
    ((Some v) v)
    (None (perform throw "unwrap-some: got None")))))

(run-fibers-parallel (lambda ()
  (let ((listener (unwrap-ok (perform tcp-listen {port}) "listen")))
    ;; Server: accept, receive, echo with prefix
    (perform fiber-spawn (lambda ()
      (let ((client (unwrap-some (perform tcp-accept listener))))
        (let ((d1 (unwrap-some (perform tcp-recv client))))
          (unwrap-ok (perform tcp-send client (string-append "echo:" d1)) "send")
          (perform tcp-close client)))))
    (perform fiber-yield)
    (perform fiber-yield)
    ;; Client
    (let ((sock (unwrap-ok (perform tcp-connect "127.0.0.1" {port}) "connect")))
      (unwrap-ok (perform tcp-send sock "foobar") "send")
      (let ((resp (unwrap-some (perform tcp-recv sock))))
        (perform println resp)
        (perform tcp-close sock)))
    (perform tcp-close listener)))
  2)
"#
    );
    let (result, output) = eval_with_output(&code);
    assert!(result.is_ok(), "tcp send/recv failed: {:?}", result);
    assert_eq!(output.trim(), "echo:foobar");
}

#[test]
fn tcp_listener_close_wakes_acceptor() {
    let port = 19387;
    let code = format!(
        r#"
(define unwrap-ok (lambda (r label)
  (match r
    ((Ok v) v)
    ((Err msg) (perform throw (string-append label ": " msg))))))

(run-fibers-parallel (lambda ()
  (let ((listener (unwrap-ok (perform tcp-listen {port}) "listen")))
    ;; Fiber waiting to accept
    (perform fiber-spawn (lambda ()
      (let ((result (perform tcp-accept listener)))
        (perform println (match result (None "woke-none") ((Some _) "got-conn"))))))
    (perform fiber-yield)
    (perform fiber-yield)
    ;; Close the listener while someone is waiting
    (perform tcp-close listener)
    ;; Give the acceptor time to wake
    (perform fiber-yield)
    (perform fiber-yield)
    (perform fiber-yield)))
  2)
"#
    );
    let (result, output) = eval_with_output(&code);
    assert!(
        result.is_ok(),
        "tcp listener close test failed: {:?}",
        result
    );
    assert_eq!(output.trim(), "woke-none");
}

#[test]
fn tcp_multiple_clients() {
    let port = 19388;
    let code = format!(
        r#"
(define unwrap-ok (lambda (r label)
  (match r
    ((Ok v) v)
    ((Err msg) (perform throw (string-append label ": " msg))))))

(define unwrap-some (lambda (opt)
  (match opt
    ((Some v) v)
    (None (perform throw "unwrap-some: got None")))))

(run-fibers-parallel (lambda ()
  (let ((listener (unwrap-ok (perform tcp-listen {port}) "listen")))
    ;; Server: handle two connections sequentially
    (perform fiber-spawn (lambda ()
      (let ((c1 (unwrap-some (perform tcp-accept listener))))
        (let ((d1 (unwrap-some (perform tcp-recv c1))))
          (unwrap-ok (perform tcp-send c1 (string-append "1:" d1)) "send")
          (perform tcp-close c1)))
      (let ((c2 (unwrap-some (perform tcp-accept listener))))
        (let ((d2 (unwrap-some (perform tcp-recv c2))))
          (unwrap-ok (perform tcp-send c2 (string-append "2:" d2)) "send")
          (perform tcp-close c2)))))
    (perform fiber-yield)
    (perform fiber-yield)
    ;; Client 1
    (let ((s1 (unwrap-ok (perform tcp-connect "127.0.0.1" {port}) "connect")))
      (unwrap-ok (perform tcp-send s1 "alpha") "send")
      (let ((r1 (unwrap-some (perform tcp-recv s1))))
        (perform println r1)
        (perform tcp-close s1)))
    ;; Client 2
    (let ((s2 (unwrap-ok (perform tcp-connect "127.0.0.1" {port}) "connect")))
      (unwrap-ok (perform tcp-send s2 "beta") "send")
      (let ((r2 (unwrap-some (perform tcp-recv s2))))
        (perform println r2)
        (perform tcp-close s2)))
    (perform tcp-close listener)))
  2)
"#
    );
    let (result, output) = eval_with_output(&code);
    assert!(result.is_ok(), "tcp multiple clients failed: {:?}", result);
    assert!(output.contains("1:alpha"), "output: {}", output);
    assert!(output.contains("2:beta"), "output: {}", output);
}

#[test]
fn tcp_bidirectional() {
    let port = 19389;
    let code = format!(
        r#"
(define unwrap-ok (lambda (r label)
  (match r
    ((Ok v) v)
    ((Err msg) (perform throw (string-append label ": " msg))))))

(define unwrap-some (lambda (opt)
  (match opt
    ((Some v) v)
    (None (perform throw "unwrap-some: got None")))))

(run-fibers-parallel (lambda ()
  (let ((listener (unwrap-ok (perform tcp-listen {port}) "listen")))
    ;; Server: recv then send
    (perform fiber-spawn (lambda ()
      (let ((client (unwrap-some (perform tcp-accept listener))))
        (let ((msg (unwrap-some (perform tcp-recv client))))
          (unwrap-ok (perform tcp-send client "pong") "send")
          (perform tcp-close client)))))
    (perform fiber-yield)
    (perform fiber-yield)
    ;; Client: send then recv
    (let ((sock (unwrap-ok (perform tcp-connect "127.0.0.1" {port}) "connect")))
      (unwrap-ok (perform tcp-send sock "ping") "send")
      (let ((resp (unwrap-some (perform tcp-recv sock))))
        (perform println resp)
        (perform tcp-close sock)))
    (perform tcp-close listener)))
  2)
"#
    );
    let (result, output) = eval_with_output(&code);
    assert!(result.is_ok(), "tcp bidirectional failed: {:?}", result);
    assert_eq!(output.trim(), "pong");
}

#[test]
fn tcp_connect_failure() {
    // Connect to a port that's not listening — returns (Err msg)
    let (result, output) = eval_with_output(
        r#"
(run-fibers-parallel (lambda ()
  (perform fiber-spawn (lambda ()
    (let ((r (perform tcp-connect "127.0.0.1" 19399)))
      (match r
        ((Err msg) (perform println (string-append "err:" msg)))
        ((Ok _) (perform println "unexpected-ok"))))))
  ;; Main fiber waits a bit for the child to attempt connection
  (perform fiber-yield)
  (perform fiber-yield)
  (perform fiber-yield)
  (perform fiber-yield)
  (perform println "main-ok"))
  2)
"#,
    );
    assert!(
        result.is_ok(),
        "tcp connect failure test failed: {:?}",
        result
    );
    assert!(output.contains("main-ok"), "output: {}", output);
    assert!(output.contains("err:tcp-connect:"), "output: {}", output);
}

#[test]
fn tcp_single_threaded_rejects() {
    // TCP effects should fail in the single-threaded scheduler
    let result = eval(
        r#"
(run-fibers (lambda ()
  (perform tcp-listen 19390)))
"#,
    );
    assert!(
        result.is_err(),
        "expected error for TCP in single-threaded scheduler"
    );
    let err_msg = result.unwrap_err().message;
    assert!(
        err_msg.contains("multi-threaded"),
        "error message: {}",
        err_msg
    );
}

// =========================================================================
// Exception (throw) effect tests
// =========================================================================

#[test]
fn throw_crashes_fiber_default() {
    // Unhandled throw should crash the fiber (default handler)
    let (result, output) = eval_with_output(
        r#"
(run-fibers (lambda ()
  (perform fiber-spawn (lambda ()
    (perform throw "boom")))
  (perform fiber-yield)
  (perform fiber-yield)
  (perform println "main-survived")))
"#,
    );
    // Main fiber should survive — the spawned fiber crashes
    assert!(result.is_ok(), "throw crash test failed: {:?}", result);
    assert_eq!(output.trim(), "main-survived");
}

#[test]
fn throw_caught_by_handle() {
    // User code can catch throw with a handle block
    let (result, output) = eval_with_output(
        r#"
(run-fibers (lambda ()
  (let ((r (handle
             (begin
               (perform throw "oops")
               "unreachable")
             ((throw msg k) -> (string-append "caught:" msg)))))
    (perform println r))))
"#,
    );
    assert!(result.is_ok(), "throw handle test failed: {:?}", result);
    assert_eq!(output.trim(), "caught:oops");
}

#[test]
fn throw_with_tcp_connect_and_handle() {
    // tcp-connect failure returns (Err msg), match on Result directly (recommended API)
    let (result, output) = eval_with_output(
        r#"
(run-fibers-parallel (lambda ()
  ;; Spawn a fiber that tries to connect and checks the Result
  (perform fiber-spawn (lambda ()
    (let ((result (perform tcp-connect "127.0.0.1" 19399)))
      (match result
        ((Ok v) (perform println "unexpected-ok"))
        ((Err msg) (perform println (string-append "err:" msg)))))))
  ;; Main fiber waits
  (perform fiber-yield)
  (perform fiber-yield)
  (perform fiber-yield)
  (perform fiber-yield)
  (perform fiber-yield)
  (perform println "done"))
  2)
"#,
    );
    assert!(result.is_ok(), "throw tcp test failed: {:?}", result);
    assert!(output.contains("err:tcp-connect:"), "output: {}", output);
    assert!(output.contains("done"), "output: {}", output);
}

#[test]
fn throw_with_tcp_listen_and_handle() {
    // tcp-listen returns Result: second bind on same port should fail with (Err msg)
    let (result, output) = eval_with_output(
        r#"
(define unwrap-ok (lambda (r label)
  (match r
    ((Ok v) v)
    ((Err msg) (perform throw (string-append label ": " msg))))))

(run-fibers-parallel (lambda ()
  ;; First bind the port
  (let ((listener (unwrap-ok (perform tcp-listen 19391) "listen")))
    ;; Try to bind the SAME port again — should fail with (Err msg)
    ;; tcp-listen is synchronous so Result matching works directly
    (let ((r2 (perform tcp-listen 19391)))
      (match r2
        ((Ok _) (perform println "unexpected-ok"))
        ((Err msg) (perform println (string-append "err:" msg)))))
    (perform tcp-close listener)))
  2)
"#,
    );
    assert!(result.is_ok(), "throw listen test failed: {:?}", result);
    assert!(output.contains("err:tcp-listen:"), "output: {}", output);
}

// ===== Variadic Functions (Rest Args) =====

#[test]
fn variadic_string_append_3_args() {
    let (_, output) = eval_with_output(r#"(perform println (string-append "a" "b" "c"))"#);
    assert_eq!(output.trim(), "abc");
}

#[test]
fn variadic_string_append_1_arg() {
    let (_, output) = eval_with_output(r#"(perform println (string-append "solo"))"#);
    assert_eq!(output.trim(), "solo");
}

#[test]
fn variadic_string_append_0_args() {
    let result = eval_to_string(r#"(string-append)"#);
    assert_eq!(result, r#""""#);
}

#[test]
fn variadic_list_0_args() {
    let result = eval_to_string("(list)");
    assert_eq!(result, "()");
}

#[test]
fn variadic_list_multiple_args() {
    let result = eval_to_string("(list 1 2 3)");
    assert_eq!(result, "(1 2 3)");
}

#[test]
fn variadic_lambda_all_rest() {
    // (lambda (. args) ...) captures all args as a list
    let result = eval_to_string(
        r#"
        (define f (lambda (. args) args))
        (f 1 2 3)
        "#,
    );
    assert_eq!(result, "(1 2 3)");
}

#[test]
fn variadic_lambda_all_rest_empty() {
    let result = eval_to_string(
        r#"
        (define f (lambda (. args) args))
        (f)
        "#,
    );
    assert_eq!(result, "()");
}

#[test]
fn variadic_lambda_required_plus_rest() {
    // (lambda (a b . rest) ...) — a and b required, rest captures extras
    let (_, output) = eval_with_output(
        r#"
        (define f (lambda (a b . rest)
          (begin
            (perform println (show a))
            (perform println (show b))
            (perform println (show rest)))))
        (f 1 2 3 4 5)
        "#,
    );
    let lines: Vec<&str> = output.trim().lines().collect();
    assert_eq!(lines[0], "1");
    assert_eq!(lines[1], "2");
    assert_eq!(lines[2], "(3 4 5)");
}

#[test]
fn variadic_lambda_required_plus_empty_rest() {
    // Calling with exactly the required params -> rest is empty list
    let result = eval_to_string(
        r#"
        (define f (lambda (a b . rest) rest))
        (f 1 2)
        "#,
    );
    assert_eq!(result, "()");
}

#[test]
fn variadic_lambda_tail_call() {
    // Variadic functions should work with tail calls
    let result = eval_to_string(
        r#"
        (define sum-list (lambda (lst acc)
          (match lst
            (Nil acc)
            ((Cons x rest) (sum-list rest (+ acc x))))))
        (define my-sum (lambda (. args)
          (sum-list args 0)))
        (my-sum 1 2 3 4 5)
        "#,
    );
    assert_eq!(result, "15");
}

#[test]
fn variadic_lambda_too_few_args_errors() {
    // Calling a variadic function with fewer than required args should error
    let result = eval(
        r#"
        (define f (lambda (a b . rest) a))
        (f 1)
        "#,
    );
    assert!(result.is_err());
    let err_msg = format!("{:?}", result.unwrap_err());
    assert!(err_msg.contains("at least 2"), "error: {}", err_msg);
}
