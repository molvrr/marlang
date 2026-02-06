use marlang_core::intern::Interner;
use marlang_core::lexer::lex;
use marlang_core::parser::parse;

use crate::builtins::register_builtins;
use crate::bytecode::{Chunk, ConstantValue, Op};
use crate::compiler::{compile, compile_program};
use crate::debug::disassemble;
use crate::errors::VMError;
use crate::value::{format_value, Value};
use crate::vm::{interpret, VM};

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
    chunk.constants.iter().any(|c| matches!(c, ConstantValue::Number(v) if *v == n))
}

fn find_constant_string(chunk: &Chunk, s: &str) -> bool {
    chunk.constants.iter().any(|c| matches!(c, ConstantValue::String(v) if v == s))
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
    assert!(chunk.constants.iter().any(|c| matches!(c, ConstantValue::Boolean(true))));
}

#[test]
fn literal_boolean_false() {
    let (chunk, _) = compile_source("#f");
    assert!(chunk.constants.iter().any(|c| matches!(c, ConstantValue::Boolean(false))));
}

#[test]
fn literal_nil() {
    let (chunk, _) = compile_source("nil");
    assert!(chunk.constants.iter().any(|c| matches!(c, ConstantValue::Nil)));
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
    assert!(chunk.constants.iter().any(|c| matches!(c, ConstantValue::Nil)));
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
    assert!(chunk.constants.iter().any(|c| matches!(c, ConstantValue::Prototype(_))));
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
    let proto = chunk.constants.iter().find_map(|c| match c {
        ConstantValue::Prototype(p) => Some(p),
        _ => None,
    }).unwrap();
    assert!(proto.chunk.code.iter().any(|i| i.op == Op::TailCall));
}

#[test]
fn lambda_with_typed_param() {
    let (chunk, _) = compile_source("(lambda ((x : Int)) x)");
    assert!(find_op(&chunk, Op::Closure));
    let proto = chunk.constants.iter().find_map(|c| match c {
        ConstantValue::Prototype(p) => Some(p),
        _ => None,
    }).unwrap();
    assert_eq!(proto.arity, 1);
}

#[test]
fn upvalue_capture() {
    let (chunk, _) = compile_source("(let ((x 1)) (lambda () x))");
    // The inner lambda should have upvalues
    let proto = chunk.constants.iter().find_map(|c| match c {
        ConstantValue::Prototype(p) => Some(p),
        _ => None,
    }).unwrap();
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
    assert!(chunk.constants.iter().any(|c| {
        matches!(c, ConstantValue::String(s) if s == "\0ctor:Some:1")
    }));
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
    let (chunk, _) = compile_source("(begin (data Option (Some a) None) (match None (None 1) (_ 0)))");
    assert!(find_op(&chunk, Op::MatchTag));
}

#[test]
fn match_constructor_with_args() {
    let (chunk, _) = compile_source("(begin (data Option (Some a) None) (match (Some 1) ((Some x) x) (None 0)))");
    assert!(find_op(&chunk, Op::MatchTag));
    assert!(find_op(&chunk, Op::GetField));
}

#[test]
fn match_no_match_halt() {
    let (chunk, _) = compile_source("(match 1 (2 \"two\"))");
    assert!(find_op(&chunk, Op::Halt));
    assert!(find_constant_string(&chunk, "No pattern matched in match expression"));
}

// ===== Effects =====

#[test]
fn effect_declaration_emits_nil() {
    let (chunk, _) = compile_source("(effect IO (print (-> String Unit)))");
    // effect declarations just produce nil
    assert!(chunk.constants.iter().any(|c| matches!(c, ConstantValue::Nil)));
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
    assert!(chunk.constants.iter().any(|c| {
        matches!(c, ConstantValue::String(s) if s.starts_with("\0handler:"))
    }));
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
    let proto_count = chunk.constants.iter().filter(|c| matches!(c, ConstantValue::Prototype(_))).count();
    assert!(proto_count >= 3, "expected at least 3 prototypes (body, handler, return), got {}", proto_count);
}

#[test]
fn resume_opcode() {
    let source = "(resume k 42)";
    let (chunk, _) = compile_source(source);
    assert!(find_op(&chunk, Op::Resume));
}

// ===== Type Forms =====

#[test]
fn class_emits_nil() {
    let (chunk, _) = compile_source("(class Eq (a) (eq (-> a a Bool)))");
    assert!(chunk.constants.iter().any(|c| matches!(c, ConstantValue::Nil)));
}

#[test]
fn type_emits_nil() {
    let (chunk, _) = compile_source("(type Pair (a b) (MkPair a b))");
    // type declarations emit nil
    assert!(chunk.constants.iter().any(|c| matches!(c, ConstantValue::Nil)));
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
    let chunk = compile_program(&exprs, &interner).expect("compile failed");
    let mut vm = VM::new();
    register_builtins(&mut vm);
    interpret(&mut vm, &chunk)
}

fn eval_to_string(source: &str) -> String {
    format_value(&eval(source).expect("eval failed"))
}

fn eval_with_output(source: &str) -> (Result<Value, VMError>, String) {
    let interner = Interner::new();
    let tokens = lex(source, &interner).expect("lex failed");
    let exprs = parse(&tokens).expect("parse failed");
    let chunk = compile_program(&exprs, &interner).expect("compile failed");
    let mut vm = VM::new();
    register_builtins(&mut vm);
    vm.captured_output = Some(String::new());
    let result = interpret(&mut vm, &chunk);
    let output = vm.captured_output.take().unwrap_or_default();
    (result, output)
}

fn assert_num(source: &str, expected: f64) {
    let val = eval(source).expect(&format!("eval failed for: {}", source));
    let n = val.as_number().expect(&format!("expected number, got {:?} for: {}", format_value(&val), source));
    assert!((n - expected).abs() < 1e-10, "expected {} but got {} for: {}", expected, n, source);
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
    let s = val.as_string().expect(&format!("expected string, got {:?} for: {}", format_value(&val), source));
    assert_eq!(s, expected, "for: {}", source);
}

fn assert_nil(source: &str) {
    let val = eval(source).expect(&format!("eval failed for: {}", source));
    assert!(val.is_nil(), "expected nil, got {:?} for: {}", format_value(&val), source);
}

fn assert_formatted(source: &str, expected: &str) {
    let result = eval_to_string(source);
    assert_eq!(result, expected, "for: {}", source);
}

fn assert_error(source: &str, substring: &str) {
    let result = eval(source);
    match result {
        Err(e) => assert!(e.message.contains(substring), "expected error containing '{}', got: {}", substring, e.message),
        Ok(v) => panic!("expected error containing '{}', but got: {:?}", substring, format_value(&v)),
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
    assert_num("(define make-adder (lambda (n) (lambda (x) (+ n x)))) ((make-adder 5) 10)", 15.0);
}

#[test]
fn exec_begin_in_function_body() {
    assert_num("(define test (lambda () (begin (define a 1) (define b 2) (+ a b)))) (test)", 3.0);
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
    assert_nil("(string->number \"abc\")");
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
    assert_error("(define f (lambda (x) x)) (f 1 2)", "Expected 1 arguments, got 2");
}

// ===== Execution: Type forms (no-ops) =====

#[test]
fn exec_class_nil() {
    assert_nil("(class Eq (a) (eq (-> a a Bool)))");
}

#[test]
fn exec_type_nil() {
    assert_nil("(type Pair (a b) (MkPair a b))");
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
    assert_num(
        "(match (MkPair 3 4) ((MkPair a b) (+ a b)))",
        7.0,
    );
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
    assert_num(
        "(match (Ok 10) ((Ok v) v) ((Err e) 0))",
        10.0,
    );
}

#[test]
fn exec_match_err() {
    assert_num(
        "(match (Err \"fail\") ((Ok v) v) ((Err e) 0))",
        0.0,
    );
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
