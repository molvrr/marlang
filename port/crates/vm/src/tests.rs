use marlang_core::intern::Interner;
use marlang_core::lexer::lex;
use marlang_core::parser::parse;

use crate::bytecode::{Chunk, ConstantValue, Op};
use crate::compiler::{compile, compile_program};
use crate::debug::disassemble;

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
