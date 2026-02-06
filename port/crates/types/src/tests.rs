use marlang_core::intern::Interner;
use marlang_core::lexer::lex;
use marlang_core::parser::parse;

use crate::ast::format_type;
use crate::checker::{type_check, TypeCheckResult};
use crate::errors::format_error;

fn check(source: &str) -> Result<Vec<crate::ast::Type>, String> {
    let interner = Interner::new();
    let tokens = lex(source, &interner).expect("lex failed");
    let exprs = parse(&tokens).expect("parse failed");
    match type_check(&exprs, &interner) {
        TypeCheckResult::Ok { types, .. } => Ok(types),
        TypeCheckResult::Err { error } => Err(format_error(&error)),
    }
}

fn check_ok(source: &str) -> Vec<crate::ast::Type> {
    match check(source) {
        Ok(types) => types,
        Err(e) => panic!("expected ok, got error: {}", e),
    }
}

fn check_err(source: &str) -> String {
    match check(source) {
        Ok(types) => panic!(
            "expected error, got ok: {:?}",
            types.iter().map(format_type).collect::<Vec<_>>()
        ),
        Err(e) => e,
    }
}

fn last_type(types: &[crate::ast::Type]) -> String {
    format_type(types.last().expect("no types"))
}

// ===== Basic Types =====

#[test]
fn basic_integer_literal() {
    let types = check_ok("42");
    assert_eq!(format_type(&types[0]), "Int");
}

#[test]
fn basic_float_literal() {
    let types = check_ok("3.14");
    assert_eq!(format_type(&types[0]), "Float");
}

#[test]
fn basic_string_literal() {
    let types = check_ok("\"hello\"");
    assert_eq!(format_type(&types[0]), "String");
}

#[test]
fn basic_boolean_literal() {
    let types = check_ok("#t");
    assert_eq!(format_type(&types[0]), "Bool");
}

// ===== Arithmetic =====

#[test]
fn addition_of_integers() {
    let types = check_ok("(+ 1 2)");
    assert_eq!(format_type(&types[0]), "Int");
}

#[test]
fn addition_of_floats() {
    let types = check_ok("(+ 1.0 2.0)");
    assert_eq!(format_type(&types[0]), "Float");
}

#[test]
fn comparison_returns_bool() {
    let types = check_ok("(< 1 2)");
    assert_eq!(format_type(&types[0]), "Bool");
}

// ===== Type Annotations =====

#[test]
fn annotated_define_int() {
    check_ok("(define (x : Int) 42)");
}

#[test]
fn annotated_define_float() {
    check_ok("(define (y : Float) 3.14)");
}

#[test]
fn type_mismatch_in_annotated_define() {
    let err = check_err("(define (x : Int) \"hello\")");
    assert!(err.contains("cannot unify"), "error was: {}", err);
}

#[test]
fn annotated_lambda_parameter() {
    check_ok("(lambda ((x : Int)) x)");
}

#[test]
fn function_type_annotation() {
    check_ok("(define (add : (-> Int Int Int)) (lambda (a b) (+ a b)))");
}

// ===== Functions =====

#[test]
fn identity_function_is_polymorphic() {
    check_ok("(define id (lambda (x) x))");
}

#[test]
fn function_application() {
    let types = check_ok(
        "(define (add : (-> Int Int Int)) (lambda (a b) (+ a b)))
         (add 1 2)",
    );
    assert_eq!(last_type(&types), "Int");
}

// ===== If Expressions =====

#[test]
fn if_branches_same_type() {
    let types = check_ok("(if #t 1 2)");
    assert_eq!(format_type(&types[0]), "Int");
}

#[test]
fn if_condition_must_be_bool() {
    let err = check_err("(if 1 2 3)");
    assert!(err.contains("cannot unify"), "error was: {}", err);
}

#[test]
fn if_branches_type_mismatch() {
    let err = check_err("(if #t 1 \"hello\")");
    assert!(err.contains("cannot unify"), "error was: {}", err);
}

// ===== Let Expressions =====

#[test]
fn basic_let_binding() {
    let types = check_ok("(let ((x 42)) x)");
    assert_eq!(format_type(&types[0]), "Int");
}

#[test]
fn let_with_type_annotation() {
    let types = check_ok("(let (((x : Int) 42)) x)");
    assert_eq!(format_type(&types[0]), "Int");
}

#[test]
fn let_generalization() {
    check_ok(
        "(let ((id (lambda (x) x)))
           (begin (id 1) (id \"hello\")))",
    );
}

// ===== ADTs (Type Checking) =====

#[test]
fn option_type_inference() {
    check_ok(
        "(data (Option a) None (Some a))
         (Some 42)",
    );
}

#[test]
fn match_expression_type_inference() {
    let types = check_ok(
        "(data (Option a) None (Some a))
         (define unwrap
           (lambda (opt)
             (match opt
               (None 0)
               ((Some v) v))))
         (unwrap (Some 42))",
    );
    assert_eq!(last_type(&types), "Int");
}

// ===== Typeclasses =====

#[test]
fn num_constraint_on_plus() {
    let types = check_ok(
        "(define double (lambda (x) (+ x x)))
         (double 5)",
    );
    assert_eq!(last_type(&types), "Int");
}

#[test]
fn num_constraint_with_float() {
    let types = check_ok(
        "(define double (lambda (x) (+ x x)))
         (double 2.5)",
    );
    assert_eq!(last_type(&types), "Float");
}

// ===== Recursive Types (mu-types) =====

#[test]
fn self_application_omega() {
    check_ok("(define omega (lambda (x) (x x)))");
}

#[test]
fn y_combinator_loop_pattern() {
    check_ok(
        "(define loop (lambda (f) (if #t nil (f f))))
         (loop loop)",
    );
}

#[test]
fn self_application_with_operation() {
    check_ok("(define selfapp (lambda (f) (f f)))");
}

#[test]
fn mu_type_annotation_parsing() {
    check_ok("(define (f : (mu a (-> a Int))) (lambda (x) 42))");
}

// ===== Exhaustiveness Checking =====

#[test]
fn missing_constructor_should_fail() {
    let err = check_err(
        "(data (Option a) None (Some a))
         (define id (match (Some 20) ((Some x) x)))",
    );
    assert!(
        err.contains("Non-exhaustive") || err.contains("non-exhaustive"),
        "error was: {}",
        err
    );
    assert!(err.contains("None"), "error was: {}", err);
}

#[test]
fn all_constructors_covered_should_pass() {
    check_ok(
        "(data (Option a) None (Some a))
         (match (Some 42)
           (None 0)
           ((Some x) x))",
    );
}

#[test]
fn wildcard_covers_all_should_pass() {
    check_ok(
        "(data (Option a) None (Some a))
         (match (Some 42)
           (_ 99))",
    );
}

#[test]
fn variable_pattern_covers_all_should_pass() {
    check_ok(
        "(data (Option a) None (Some a))
         (match (Some 42)
           (x 99))",
    );
}

#[test]
fn missing_nil_constructor_in_list_should_fail() {
    let err = check_err(
        "(data (List a) Nil (Cons a (List a)))
         (define head (lambda (xs)
           (match xs
             ((Cons h t) h))))",
    );
    assert!(
        err.contains("Non-exhaustive") || err.contains("non-exhaustive"),
        "error was: {}",
        err
    );
    assert!(err.contains("Nil"), "error was: {}", err);
}

#[test]
fn multiple_missing_constructors_should_list_all() {
    let err = check_err(
        "(data Color Red Green Blue)
         (match Red
           (Red 1))",
    );
    assert!(
        err.contains("Non-exhaustive") || err.contains("non-exhaustive"),
        "error was: {}",
        err
    );
    assert!(err.contains("Green"), "error was: {}", err);
    assert!(err.contains("Blue"), "error was: {}", err);
}

#[test]
fn complete_enum_coverage_should_pass() {
    check_ok(
        "(data Color Red Green Blue)
         (match Red
           (Red 1)
           (Green 2)
           (Blue 3))",
    );
}

// ===== Show Typeclass =====

#[test]
fn show_on_int() {
    let types = check_ok("(show 42)");
    assert_eq!(format_type(&types[0]), "String");
}

#[test]
fn show_on_float() {
    let types = check_ok("(show 3.14)");
    assert_eq!(format_type(&types[0]), "String");
}

#[test]
fn show_on_string() {
    let types = check_ok("(show \"hello\")");
    assert_eq!(format_type(&types[0]), "String");
}

#[test]
fn show_on_bool() {
    let types = check_ok("(show #t)");
    assert_eq!(format_type(&types[0]), "String");
}

#[test]
fn show_on_adt() {
    let types = check_ok("(data (Option a) None (Some a)) (show (Some 42))");
    assert_eq!(last_type(&types), "String");
}

#[test]
fn show_on_list() {
    let types = check_ok("(show (list 1 2 3))");
    assert_eq!(format_type(&types[0]), "String");
}

#[test]
fn show_in_function() {
    let types = check_ok(
        "(define stringify (lambda (x) (show x)))
         (stringify 42)",
    );
    assert_eq!(last_type(&types), "String");
}

// ===== Variadic Arithmetic Operators =====

#[test]
fn plus_with_zero_args() {
    check_ok("(+)");
}

#[test]
fn plus_with_multiple_args() {
    check_ok("(+ 1 2 3 4)");
}

#[test]
fn mul_with_zero_args() {
    check_ok("(*)");
}

#[test]
fn mul_with_multiple_args() {
    check_ok("(* 2 3 4)");
}

#[test]
fn minus_with_one_arg_negation() {
    check_ok("(- 5)");
}

#[test]
fn minus_with_multiple_args() {
    check_ok("(- 10 3 2)");
}

#[test]
fn minus_with_zero_args_fails() {
    check_err("(-)");
}

#[test]
fn div_with_two_args() {
    check_ok("(/ 10 2)");
}

#[test]
fn div_with_one_arg_fails() {
    check_err("(/ 10)");
}

// ===== Integration Tests =====

#[test]
fn factorial_with_type_annotation() {
    check_ok(
        "(define (factorial : (-> Int Int))
           (lambda (n)
             (if (= n 0)
               1
               (* n (factorial (- n 1))))))
         (factorial 5)",
    );
}

#[test]
fn map_function_on_adt_list() {
    check_ok(
        "(data (List a) Nil (Cons a (List a)))
         (define map
           (lambda (f lst)
             (match lst
               (Nil Nil)
               ((Cons h t) (Cons (f h) (map f t))))))
         (define double (lambda (x) (+ x x)))
         (map double (Cons 1 (Cons 2 (Cons 3 Nil))))",
    );
}

#[test]
fn fold_function_on_adt_list() {
    check_ok(
        "(data (List a) Nil (Cons a (List a)))
         (define foldl
           (lambda (f acc lst)
             (match lst
               (Nil acc)
               ((Cons h t) (foldl f (f acc h) t)))))
         (foldl + 0 (Cons 1 (Cons 2 (Cons 3 Nil))))",
    );
}
