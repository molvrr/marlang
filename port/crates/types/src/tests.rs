use crate::ast::format_type;
use crate::checker::{TypeModuleLoader, type_check, type_check_with_modules};
use crate::errors::{TypeError, format_error};
use marlang_core::ast::Expr;
use marlang_core::intern::Interner;
use marlang_core::lexer::lex;
use marlang_core::parser::parse_with_interner;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Minimal prelude for type-checker tests: data types, builtin declares, and effects.
/// Mirrors the declarations in stdlib/prelude.mar but omits macros and function definitions
/// (which require VM execution to expand/evaluate).
const TEST_PRELUDE: &str = r#"
(data (Option a) None (Some a))
(data (List a) Nil (Cons a (List a)))
(data (Pair a b) (MkPair a b))
(data (Result e a) (Err e) (Ok a))

(declare + (-> Int Int Int))
(declare - (-> Int Int Int))
(declare * (-> Int Int Int))
(declare / (-> Int Int Int))
(declare negate (-> Int Int))
(declare +. (-> Float Float Float))
(declare -. (-> Float Float Float))
(declare *. (-> Float Float Float))
(declare /. (-> Float Float Float))
(declare negate. (-> Float Float))
(declare = (-> Int Int Bool))
(declare < (-> Int Int Bool))
(declare > (-> Int Int Bool))
(declare <= (-> Int Int Bool))
(declare >= (-> Int Int Bool))
(declare =. (-> Float Float Bool))
(declare <. (-> Float Float Bool))
(declare >. (-> Float Float Bool))
(declare <=. (-> Float Float Bool))
(declare >=. (-> Float Float Bool))
(declare string= (-> String String Bool))
(declare string< (-> String String Bool))
(declare string> (-> String String Bool))
(declare string<= (-> String String Bool))
(declare string>= (-> String String Bool))
(declare equal? (-> a a Bool))
(declare show (-> a String))
(declare list (-> (... a) (List a)))
(declare error (-> String a))
(declare nil Unit)
(declare abs (-> Int Int))
(declare rem (-> Int Int Int))
(declare mod (-> Int Int Int))
(declare abs. (-> Float Float))
(declare rem. (-> Float Float Float))
(declare mod. (-> Float Float Float))
(declare sign (-> Int Int))
(declare sign. (-> Float Float))
(declare int->float (-> Int Float))
(declare floor (-> Float Int))
(declare ceil (-> Float Int))
(declare round (-> Float Int))
(declare truncate (-> Float Int))
(declare pow (-> Float Float Float))
(declare sqrt (-> Float Float))
(declare exp (-> Float Float))
(declare log (-> Float Float))
(declare log10 (-> Float Float))
(declare log2 (-> Float Float))
(declare sin (-> Float Float))
(declare cos (-> Float Float))
(declare tan (-> Float Float))
(declare asin (-> Float Float))
(declare acos (-> Float Float))
(declare atan (-> Float Float))
(declare sinh (-> Float Float))
(declare cosh (-> Float Float))
(declare tanh (-> Float Float))
(declare atan2 (-> Float Float Float))
(declare is-nan (-> Float Bool))
(declare is-infinite (-> Float Bool))
(declare is-finite (-> Float Bool))
(declare string-length (-> String Int))
(declare string-append (-> (... String) String))
(declare string-is-empty (-> String Bool))
(declare string-reverse (-> String String))
(declare string-repeat (-> String Int String))
(declare string-to-upper (-> String String))
(declare string-to-lower (-> String String))
(declare string-trim (-> String String))
(declare string-trim-start (-> String String))
(declare string-trim-end (-> String String))
(declare string-starts-with (-> String String Bool))
(declare string-ends-with (-> String String Bool))
(declare string-contains (-> String String Bool))
(declare string-index-of (-> String String (Option Int)))
(declare string-substring (-> String Int Int String))
(declare string-char-at (-> String Int (Option Char)))
(declare string-split (-> String String (List String)))
(declare string-join (-> String (List String) String))
(declare string-replace (-> String String String String))
(declare string-replace-all (-> String String String String))
(declare string-pad-start (-> String Int String String))
(declare string-pad-end (-> String Int String String))
(declare char-code (-> Char Int))
(declare char-from-code (-> Int Char))
(declare char->string (-> Char String))
(declare chars->string (-> (List Char) String))
(declare string->chars (-> String (List Char)))
(declare number->string (-> Float String))
(declare __try-string->int (-> String (Option Int)))
(declare __try-string->float (-> String (Option Float)))
(declare time-it (-> (-> Unit {| e} a) (Pair a Float)))
(declare ffi-open (-> String (List a) FFILibrary))
(declare ffi-call (-> FFILibrary String a))
(declare ffi-close (-> FFILibrary Unit))
(declare ffi-struct (-> String (List a) FFIStructDef))
(declare struct-new (-> FFIStructDef a FFIStruct))
(declare struct-get (-> FFIStruct String a))
(declare struct-set (-> FFIStruct String a Unit))
(declare bytes-new (-> Int Bytes))
(declare bytes-length (-> Bytes Int))
(declare bytes-get (-> Bytes Int Int))
(declare bytes-set (-> Bytes Int Int Bytes))
(declare bytes-slice (-> Bytes Int Int Bytes))
(declare bytes-concat (-> Bytes Bytes Bytes))
(declare bytes-from-string (-> String Bytes))
(declare bytes-to-string (-> Bytes (Result String String)))
(declare bytes-from-list (-> (List Int) Bytes))
(declare bytes-to-list (-> Bytes (List Int)))
(declare make-symbol (-> String Symbol))
(declare gensym (-> Unit Symbol))
(declare symbol->string (-> Symbol String))
(declare string->symbol (-> String Symbol))
(declare symbol? (-> a Bool))
(declare pair? (-> a Bool))
(declare nil? (-> a Bool))
(declare number? (-> a Bool))
(declare string? (-> a Bool))
(declare list? (-> a Bool))

(effect Exception
  (throw (-> e a)))
"#;

fn check(source: &str) -> Result<Vec<crate::ast::Type>, String> {
    let full_source = format!("{}\n{}", TEST_PRELUDE, source);
    let interner = Interner::new();
    let prelude_tokens = lex(TEST_PRELUDE, &interner).expect("prelude lex failed");
    let prelude_count = parse_with_interner(&prelude_tokens, &interner)
        .expect("prelude parse failed")
        .len();
    let tokens = lex(&full_source, &interner).expect("lex failed");
    let exprs = parse_with_interner(&tokens, &interner).expect("parse failed");
    match type_check(&exprs, &interner) {
        Ok(output) => Ok(output.types[prelude_count..].to_vec()),
        Err(error) => Err(format_error(&error)),
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

// ===== Nested Pattern Exhaustiveness =====

#[test]
fn nested_adt_missing_case_should_fail() {
    // The original bug: (Node _ (Node _ _)) + Leaf covers {Node, Leaf} at the
    // top level, but misses (Node _ Leaf).
    let err = check_err(
        "(data (Tree a) Leaf (Node a (Tree a)))
         (match (Node 20 (Node 19 Leaf))
           ((Node a (Node b c)) a)
           (Leaf 40))",
    );
    assert!(
        err.contains("Non-exhaustive") || err.contains("non-exhaustive"),
        "error was: {}",
        err
    );
    assert!(
        err.contains("Node") && err.contains("Leaf"),
        "expected witness like (Node _ Leaf), error was: {}",
        err
    );
}

#[test]
fn nested_adt_fully_covered_should_pass() {
    check_ok(
        "(data (Tree a) Leaf (Node a (Tree a)))
         (match (Node 20 (Node 19 Leaf))
           ((Node a (Node b c)) a)
           ((Node a Leaf) a)
           (Leaf 40))",
    );
}

#[test]
fn nested_wildcard_covers_subtree_should_pass() {
    check_ok(
        "(data (Tree a) Leaf (Node a (Tree a)))
         (match (Node 20 Leaf)
           ((Node a _) a)
           (Leaf 40))",
    );
}

#[test]
fn nested_variable_covers_subtree_should_pass() {
    check_ok(
        "(data (Tree a) Leaf (Node a (Tree a)))
         (match (Node 20 Leaf)
           ((Node a t) a)
           (Leaf 40))",
    );
}

#[test]
fn deeply_nested_missing_case_should_fail() {
    // Three levels: Node _ (Node _ (Node _ _)) + Node _ (Node _ Leaf) + Node _ Leaf + Leaf
    // covers everything EXCEPT nothing — wait, let's construct a real gap.
    // Missing: Node _ (Node _ Leaf) — the second-level Leaf case is absent.
    let err = check_err(
        "(data (Tree a) Leaf (Node a (Tree a)))
         (match (Node 1 (Node 2 (Node 3 Leaf)))
           ((Node a (Node b (Node c d))) a)
           ((Node a Leaf) a)
           (Leaf 0))",
    );
    assert!(
        err.contains("Non-exhaustive") || err.contains("non-exhaustive"),
        "error was: {}",
        err
    );
}

#[test]
fn deeply_nested_fully_covered_should_pass() {
    check_ok(
        "(data (Tree a) Leaf (Node a (Tree a)))
         (match (Node 1 (Node 2 (Node 3 Leaf)))
           ((Node a (Node b (Node c d))) a)
           ((Node a (Node b Leaf)) a)
           ((Node a Leaf) a)
           (Leaf 0))",
    );
}

#[test]
fn nested_option_in_list_missing_should_fail() {
    // Matching on (List (Option Int)): Cons (Some x) rest is present,
    // but Cons None rest is missing.
    let err = check_err(
        "(match (Cons (Some 1) Nil)
           ((Cons (Some x) rest) x)
           (Nil 0))",
    );
    assert!(
        err.contains("Non-exhaustive") || err.contains("non-exhaustive"),
        "error was: {}",
        err
    );
    assert!(
        err.contains("None"),
        "expected witness mentioning None, error was: {}",
        err
    );
}

#[test]
fn nested_option_in_list_fully_covered_should_pass() {
    check_ok(
        "(match (Cons (Some 1) Nil)
           ((Cons (Some x) rest) x)
           ((Cons None rest) 0)
           (Nil 0))",
    );
}

#[test]
fn single_wildcard_still_exhaustive() {
    check_ok(
        "(data (Tree a) Leaf (Node a (Tree a)))
         (match (Node 1 Leaf)
           (_ 42))",
    );
}

#[test]
fn single_variable_still_exhaustive() {
    check_ok(
        "(data (Tree a) Leaf (Node a (Tree a)))
         (match (Node 1 Leaf)
           (x 42))",
    );
}

#[test]
fn overlapping_patterns_still_exhaustive() {
    // Overlapping but exhaustive: first arm catches Node, second catches everything.
    check_ok(
        "(data (Tree a) Leaf (Node a (Tree a)))
         (match Leaf
           ((Node a b) a)
           (_ 0))",
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

// ===== Foreign Declarations =====

#[test]
fn foreign_basic_declaration() {
    let types = check_ok(
        "(define lib 42)
         (foreign lib
           (InitWindow (-> Int Int String Unit))
           (CloseWindow (-> Unit Unit)))
          (ffi-call lib \"InitWindow\" 800 600 \"hello\")
          (ffi-call lib \"CloseWindow\")",
    );
    assert_eq!(last_type(&types), "Unit");
}

#[test]
fn foreign_return_type_checking() {
    let types = check_ok(
        "(define lib 42)
         (foreign lib
           (GetValue (-> Unit Int))
           (IsReady (-> Unit Bool)))
          (+ (ffi-call lib \"GetValue\") 1)
          (if (ffi-call lib \"IsReady\") 1 0)",
    );
    assert_eq!(format_type(&types[2]), "Int");
    assert_eq!(format_type(&types[3]), "Int");
}

#[test]
fn foreign_arity_mismatch() {
    let err = check_err(
        "(define lib 42)
         (foreign lib
           (InitWindow (-> Int Int String Unit)))
         (ffi-call lib \"InitWindow\" 800)",
    );
    assert!(err.contains("expected 3"), "got: {err}");
    assert!(err.contains("got 1"), "got: {err}");
}

#[test]
fn foreign_type_mismatch() {
    let err = check_err(
        "(define lib 42)
         (foreign lib
           (InitWindow (-> Int Int String Unit)))
         (ffi-call lib \"InitWindow\" 800 600 42)",
    );
    assert!(err.contains("cannot unify"), "got: {err}");
}

#[test]
fn foreign_unknown_function_errors() {
    let err = check_err(
        "(define lib 42)
         (foreign lib
           (InitWindow (-> Int Int String Unit)))
         (ffi-call lib \"SomeOtherFn\" 1 2 3)",
    );
    assert!(err.contains("not declared"), "got: {err}");
}

#[test]
fn foreign_no_declaration_errors() {
    let err = check_err(
        "(define lib 42)
         (ffi-call lib \"anything\" 1 \"hello\" #t)",
    );
    assert!(err.contains("requires a (foreign"), "got: {err}");
}

#[test]
fn foreign_return_type_constrains_usage() {
    let err = check_err(
        "(define lib 42)
         (foreign lib
           (CloseWindow (-> Unit Unit)))
          (+ (ffi-call lib \"CloseWindow\") 1)",
    );
    assert!(err.contains("cannot unify"), "got: {err}");
}

#[test]
fn foreign_multiple_libraries() {
    let types = check_ok(
        "(define rl 1)
         (define gl 2)
         (foreign rl
           (InitWindow (-> Int Int String Unit)))
         (foreign gl
            (Init (-> Unit Bool)))
         (ffi-call rl \"InitWindow\" 800 600 \"test\")
         (ffi-call gl \"Init\")",
    );
    // define rl (0), define gl (1), foreign rl (2), foreign gl (3), ffi-call rl (4), ffi-call gl (5)
    assert_eq!(format_type(&types[4]), "Unit");
    assert_eq!(format_type(&types[5]), "Bool");
}

#[test]
fn foreign_with_ffi_types() {
    check_ok(
        "(define lib 42)
         (foreign lib
           (GetStruct (-> String FFIStruct))
            (GetPointer (-> Unit FFIPointer)))
         (ffi-call lib \"GetStruct\" \"test\")
         (ffi-call lib \"GetPointer\")",
    );
}

// ===== foreign-lib Declarations =====

#[test]
fn ffi_lib_basic() {
    let types = check_ok(
        "(foreign-lib rl \"libraylib.dylib\"
           (InitWindow (-> I32 I32 CString Void))
           (CloseWindow (-> Unit Void))
            (IsReady (-> Unit Bool)))
         (rl/InitWindow 800 600 \"hello\")
         (rl/CloseWindow)
         (rl/IsReady)",
    );
    // foreign-lib (0), call InitWindow (1), call CloseWindow (2), call IsReady (3)
    assert_eq!(format_type(&types[1]), "Unit");
    assert_eq!(format_type(&types[2]), "Unit");
    assert_eq!(format_type(&types[3]), "Bool");
}

#[test]
fn ffi_lib_wrong_args() {
    let err = check_err(
        "(foreign-lib rl \"libtest.dylib\"
           (InitWindow (-> I32 I32 CString Void)))
         (rl/InitWindow 800)",
    );
    assert!(
        err.contains("cannot unify") || err.contains("arity") || err.contains("expected"),
        "got: {err}"
    );
}

#[test]
fn ffi_lib_ffi_call_still_works() {
    let types = check_ok(
        "(foreign-lib rl \"libtest.dylib\"
           (InitWindow (-> I32 I32 CString Void)))
         (ffi-call rl \"InitWindow\" 800 600 \"hello\")",
    );
    assert_eq!(last_type(&types), "Unit");
}

#[test]
fn ffi_lib_void_return() {
    let types = check_ok(
        "(foreign-lib rl \"libtest.dylib\"
           (CloseWindow (-> Unit Void)))
          (rl/CloseWindow)",
    );
    assert_eq!(last_type(&types), "Unit");
}

#[test]
fn ffi_lib_numeric_types() {
    let types = check_ok(
        "(foreign-lib rl \"libtest.dylib\"
           (GetWidth (-> Unit I32))
            (GetTime (-> Unit F64))
            (GetFrameTime (-> Unit F32)))
         (+ (rl/GetWidth) 1)
         (+ (rl/GetTime) 1.0)
         (+ (rl/GetFrameTime) 1.0)",
    );
    assert_eq!(format_type(&types[1]), "Int");
    assert_eq!(format_type(&types[2]), "Float");
    assert_eq!(format_type(&types[3]), "Float");
}

#[test]
fn ffi_struct_creates_distinct_type() {
    let types = check_ok(
        "(define Color (ffi-struct \"Color\" (list (list \"r\" \"u8\") (list \"g\" \"u8\") (list \"b\" \"u8\") (list \"a\" \"u8\"))))
         (struct-new Color 255 0 0 255)",
    );
    assert_eq!(last_type(&types), "Color");
}

#[test]
fn ffi_lib_struct_type() {
    let types = check_ok(
        "(define Color (ffi-struct \"Color\" (list (list \"r\" \"u8\"))))
         (foreign-lib rl \"libtest.dylib\"
           (ClearBackground (-> Color Void)))
         (rl/ClearBackground (struct-new Color 255))",
    );
    assert_eq!(last_type(&types), "Unit");
}

// ===== FFI Struct Type Validation =====

#[test]
fn ffi_lib_undefined_struct_type_errors() {
    let err = check_err(
        "(foreign-lib rl \"libtest.dylib\"
           (ClearBackground (-> Color Void)))",
    );
    assert!(
        err.contains("Undefined type") || err.contains("not defined"),
        "got: {err}"
    );
    assert!(err.contains("Color"), "got: {err}");
}

#[test]
fn ffi_lib_defined_struct_type_ok() {
    check_ok(
        "(define Color (ffi-struct \"Color\" (list (list \"r\" \"u8\") (list \"g\" \"u8\") (list \"b\" \"u8\") (list \"a\" \"u8\"))))
         (foreign-lib rl \"libtest.dylib\"
           (ClearBackground (-> Color Void))
            (GetColor (-> Unit Color)))",
    );
}

#[test]
fn struct_new_wrong_arity_errors() {
    let err = check_err(
        "(define Color (ffi-struct \"Color\" (list (list \"r\" \"u8\") (list \"g\" \"u8\") (list \"b\" \"u8\") (list \"a\" \"u8\"))))
         (struct-new Color 255 0 0)",
    );
    assert!(err.contains("expected 4"), "got: {err}");
    assert!(err.contains("got 3"), "got: {err}");
}

#[test]
fn struct_new_wrong_field_type_errors() {
    let err = check_err(
        "(define Color (ffi-struct \"Color\" (list (list \"r\" \"u8\") (list \"g\" \"u8\"))))
         (struct-new Color 255 \"not-a-number\")",
    );
    assert!(err.contains("cannot unify"), "got: {err}");
}

#[test]
fn struct_get_returns_correct_field_type_int() {
    let types = check_ok(
        "(define Color (ffi-struct \"Color\" (list (list \"r\" \"u8\") (list \"g\" \"u8\") (list \"b\" \"u8\") (list \"a\" \"u8\"))))
         (define c (struct-new Color 255 0 0 255))
         (+ (struct-get c \"r\") 1)",
    );
    assert_eq!(last_type(&types), "Int");
}

#[test]
fn struct_get_returns_correct_field_type_float() {
    let types = check_ok(
        "(define Vec2 (ffi-struct \"Vec2\" (list (list \"x\" \"f32\") (list \"y\" \"f32\"))))
         (define v (struct-new Vec2 1.0 2.0))
         (+ (struct-get v \"x\") 1.0)",
    );
    assert_eq!(last_type(&types), "Float");
}

#[test]
fn struct_get_unknown_field_errors() {
    let err = check_err(
        "(define Color (ffi-struct \"Color\" (list (list \"r\" \"u8\") (list \"g\" \"u8\"))))
         (define c (struct-new Color 255 0))
         (struct-get c \"z\")",
    );
    assert!(err.contains("no field") && err.contains("z"), "got: {err}");
}

#[test]
fn struct_set_wrong_value_type_errors() {
    let err = check_err(
        "(define Color (ffi-struct \"Color\" (list (list \"r\" \"u8\") (list \"g\" \"u8\"))))
         (define c (struct-new Color 255 0))
         (struct-set c \"r\" \"not-a-number\")",
    );
    assert!(err.contains("cannot unify"), "got: {err}");
}

#[test]
fn struct_set_unknown_field_errors() {
    let err = check_err(
        "(define Color (ffi-struct \"Color\" (list (list \"r\" \"u8\") (list \"g\" \"u8\"))))
         (define c (struct-new Color 255 0))
         (struct-set c \"z\" 42)",
    );
    assert!(err.contains("no field") && err.contains("z"), "got: {err}");
}

#[test]
fn struct_new_unknown_def_polymorphic_fallback() {
    // When struct def is not known, fall back to generic FFIStruct
    let types = check_ok(
        "(define unknown-def 42)
         (struct-new unknown-def 1 2 3)",
    );
    assert_eq!(last_type(&types), "FFIStruct");
}

#[test]
fn struct_get_unknown_def_polymorphic_fallback() {
    // When struct type can't be determined, struct-get returns a fresh type var
    check_ok(
        "(define unknown-def 42)
         (define s (struct-new unknown-def 1 2))
         (struct-get s \"field\")",
    );
}

// ===== Import Tests =====

struct MockModuleLoader {
    modules: HashMap<String, String>,
}

impl MockModuleLoader {
    fn new() -> Self {
        MockModuleLoader {
            modules: HashMap::new(),
        }
    }

    fn add_module(&mut self, path: &str, source: &str) {
        self.modules.insert(path.to_string(), source.to_string());
    }
}

impl TypeModuleLoader for MockModuleLoader {
    fn resolve_and_load(
        &self,
        import_path: &str,
        _from_file: Option<&Path>,
        interner: &Interner,
    ) -> Result<(Vec<Expr>, PathBuf), TypeError> {
        let source = self
            .modules
            .get(import_path)
            .ok_or_else(|| TypeError::InternalError {
                message: format!("Mock module not found: {}", import_path),
                span: None,
            })?;

        let tokens = lex(source, interner).map_err(|e| TypeError::InternalError {
            message: format!("Lex error: {}", e),
            span: None,
        })?;

        let exprs =
            parse_with_interner(&tokens, interner).map_err(|e| TypeError::InternalError {
                message: format!("Parse error: {}", e),
                span: None,
            })?;

        let resolved = PathBuf::from(format!("/mock/{}.mar", import_path));
        Ok((exprs, resolved))
    }
}

fn check_with_modules(
    source: &str,
    loader: &dyn TypeModuleLoader,
) -> Result<Vec<crate::ast::Type>, String> {
    let full_source = format!("{}\n{}", TEST_PRELUDE, source);
    let interner = Interner::new();
    let prelude_tokens = lex(TEST_PRELUDE, &interner).expect("prelude lex failed");
    let prelude_count = parse_with_interner(&prelude_tokens, &interner)
        .expect("prelude parse failed")
        .len();
    let tokens = lex(&full_source, &interner).expect("lex failed");
    let exprs = parse_with_interner(&tokens, &interner).expect("parse failed");
    match type_check_with_modules(&exprs, &interner, Some(Path::new("/test/main.mar")), loader) {
        Ok(output) => Ok(output.types[prelude_count..].to_vec()),
        Err(error) => Err(format_error(&error)),
    }
}

#[test]
fn import_qualified_function() {
    let mut loader = MockModuleLoader::new();
    loader.add_module("mymod", "(define add1 (lambda (x) (+ x 1)))");

    let types = check_with_modules(r#"(import "mymod" as m) (m/add1 5)"#, &loader).unwrap();
    assert_eq!(last_type(&types), "Int");
}

#[test]
fn import_qualified_data_type() {
    let mut loader = MockModuleLoader::new();
    loader.add_module(
        "colors",
        r#"
        (data Color Red Green Blue)
        (define is-red (lambda (c)
            (match c
                (Red #t)
                (Green #f)
                (Blue #f))))
    "#,
    );

    let types = check_with_modules(r#"(import "colors" as c) (c/is-red c/Red)"#, &loader).unwrap();
    assert_eq!(last_type(&types), "Bool");
}

#[test]
fn import_selective() {
    let mut loader = MockModuleLoader::new();
    loader.add_module(
        "math",
        "(define square (lambda (x) (* x x)))
         (define double (lambda (x) (+ x x)))",
    );

    let types = check_with_modules(r#"(import "math" (square)) (square 5)"#, &loader).unwrap();
    assert_eq!(last_type(&types), "Int");
}

#[test]
fn import_selective_not_imported_is_error() {
    let mut loader = MockModuleLoader::new();
    loader.add_module(
        "math",
        "(define square (lambda (x) (* x x)))
         (define double (lambda (x) (+ x x)))",
    );

    let err = check_with_modules(r#"(import "math" (square)) (double 5)"#, &loader).unwrap_err();
    assert!(err.contains("Undefined variable"), "error was: {}", err);
    assert!(err.contains("double"), "error was: {}", err);
}

#[test]
fn import_all() {
    let mut loader = MockModuleLoader::new();
    loader.add_module(
        "utils",
        "(define inc (lambda (x) (+ x 1)))
         (define dec (lambda (x) (- x 1)))",
    );

    let types = check_with_modules(r#"(import "utils") (inc 5)"#, &loader).unwrap();
    assert_eq!(last_type(&types), "Int");
}

#[test]
fn import_qualified_effect_wrapper() {
    // Effects are accessed through wrapper functions, not directly via qualified perform.
    // The call must be wrapped in a handler since Greet is not a runtime-handled effect.
    let mut loader = MockModuleLoader::new();
    loader.add_module(
        "myeffect",
        r#"
        (effect Greet
            (hello (-> String String)))
        (define greet (lambda (name) (perform hello name)))
    "#,
    );

    let types = check_with_modules(
        r#"(import "myeffect" as g)
           (handle (g/greet "world")
             ((hello name k) -> (resume k (string-append "Hello, " name))))"#,
        &loader,
    )
    .unwrap();
    assert_eq!(last_type(&types), "String");
}

#[test]
fn unhandled_effect_rejected_at_top_level() {
    // A non-runtime effect used at the top level without a handler should be rejected
    let err = check_err(
        r#"(effect Foo
               (bar (-> Unit Int)))
           (perform bar)"#,
    );
    assert!(
        err.contains("Unhandled effect"),
        "expected unhandled effect error, got: {}",
        err
    );
}

#[test]
fn handled_effect_accepted_at_top_level() {
    // A non-runtime effect properly handled should be accepted
    check_ok(
        r#"(effect Foo
               (bar (-> Unit Int)))
           (handle (perform bar)
             ((bar k) -> (resume k 42)))"#,
    );
}

#[test]
fn runtime_effects_accepted_at_top_level() {
    // Runtime-handled effects (IO, Exception) should be accepted without explicit handlers
    check_ok(
        r#"(effect IO
               (println (-> String Unit)))
           (perform println "hello")"#,
    );
}

#[test]
fn import_module_cache_reuse() {
    // Import the same module twice — should work and use cache
    let mut loader = MockModuleLoader::new();
    loader.add_module("shared", "(define val 42)");

    let types = check_with_modules(
        r#"(import "shared" as a)
           (import "shared" as b)
           (+ a/val b/val)"#,
        &loader,
    )
    .unwrap();
    assert_eq!(last_type(&types), "Int");
}

#[test]
fn import_nonexistent_module_errors() {
    let loader = MockModuleLoader::new();

    let err = check_with_modules(r#"(import "nonexistent" as x)"#, &loader).unwrap_err();
    assert!(err.contains("not found"), "error was: {}", err);
}

#[test]
fn import_qualified_preserves_prelude_types() {
    // Prelude types (like Option/List) that exist in both target and source
    // should NOT be rewritten — they should stay as their original names
    let mut loader = MockModuleLoader::new();
    loader.add_module(
        "mymod",
        r#"
        (data (Option a) None (Some a))
        (define wrap-some (lambda (x) (Some x)))
    "#,
    );

    let types = check_with_modules(
        r#"(data (Option a) None (Some a))
           (import "mymod" as m)
           (m/wrap-some 42)"#,
        &loader,
    )
    .unwrap();
    // The result should be (Option Int), not (m/Option Int)
    // because Option already exists in the target env
    assert_eq!(last_type(&types), "(Option Int)");
}

// ===== Variadic Functions (Rest Args) =====

#[test]
fn variadic_string_append_3_args_typechecks() {
    // This was the original net.mar bug: 3-arg string-append
    check_ok(r#"(string-append "a" "b" "c")"#);
}

#[test]
fn variadic_string_append_0_args_typechecks() {
    check_ok(r#"(string-append)"#);
}

#[test]
fn variadic_string_append_1_arg_typechecks() {
    check_ok(r#"(string-append "solo")"#);
}

#[test]
fn variadic_string_append_non_string_fails() {
    let err = check_err(r#"(string-append "ok" 42)"#);
    assert!(
        err.contains("String") || err.contains("unif"),
        "error: {}",
        err
    );
}

#[test]
fn variadic_list_typechecks() {
    let types = check_ok("(list 1 2 3)");
    assert_eq!(last_type(&types), "(List Int)");
}

#[test]
fn variadic_list_empty_typechecks() {
    check_ok("(list)");
}

#[test]
fn variadic_lambda_dot_notation() {
    let types = check_ok(r#"(lambda (. args) args)"#);
    // Result should be variadic function returning list
    let ty = last_type(&types);
    assert!(ty.contains("..."), "type should be variadic: {}", ty);
    assert!(ty.contains("List"), "type should return List: {}", ty);
}

#[test]
fn variadic_lambda_required_plus_rest() {
    let types = check_ok(
        r#"(define f (lambda (a b . rest) rest))
           (f 1 2 3 4)"#,
    );
    assert_eq!(last_type(&types), "(List Int)");
}

#[test]
fn variadic_lambda_rest_type_arrow_annotation() {
    // Test (... T) syntax in type annotations
    let types = check_ok(
        r#"(define (f : (-> (... String) String))
             (lambda (. args) (string-append "hello")))
           (f "a" "b" "c")"#,
    );
    assert_eq!(last_type(&types), "String");
}

#[test]
fn variadic_lambda_too_few_args_fails() {
    // Calling a variadic function with fewer than required params
    let err = check_err(
        r#"(define f (lambda (a b . rest) a))
           (f 1)"#,
    );
    assert!(
        err.contains("argument") || err.contains("arity") || err.contains("Arity"),
        "error: {}",
        err
    );
}

// ===== Effect Row Tests =====

#[test]
fn effect_row_pure_lambda_inferred() {
    // A pure lambda should get Closed([]) effect row
    let types = check_ok("(lambda (x) (+ x 1))");
    let t = format_type(&types[0]);
    assert_eq!(t, "(-> Int {} Int)");
}

#[test]
fn effect_row_effectful_lambda_inferred() {
    // A lambda that performs IO should have IO in its effect row
    let types = check_ok(
        r#"(effect IO
               (println (-> String Unit)))
           (lambda (x) (perform println x))"#,
    );
    let t = format_type(types.last().unwrap());
    assert!(t.contains("{IO}"), "expected {{IO}} in type, got: {}", t);
}

#[test]
fn effect_row_annotation_pure() {
    // Type annotation with empty effect row: (-> String {} String)
    let types = check_ok(
        r#"(define (f : (-> String {} String)) (lambda (s) s))
           (f "hello")"#,
    );
    assert_eq!(last_type(&types), "String");
}

#[test]
fn effect_row_annotation_effectful() {
    // Type annotation with effect row: (-> String {IO} Unit)
    let types = check_ok(
        r#"(effect IO
               (println (-> String Unit)))
           (define (f : (-> String {IO} Unit)) (lambda (s) (perform println s)))
           (f "hello")"#,
    );
    assert_eq!(last_type(&types), "Unit");
}

#[test]
fn effect_row_annotation_open() {
    // Open effect row annotation: (-> String {IO | e} Unit)
    let types = check_ok(
        r#"(effect IO
               (println (-> String Unit)))
           (define (f : (-> String {IO | e} Unit)) (lambda (s) (perform println s)))
           (f "hello")"#,
    );
    assert_eq!(last_type(&types), "Unit");
}

#[test]
fn effect_row_pure_annotated_rejects_effectful() {
    // A pure annotation should reject an effectful body
    let err = check_err(
        r#"(effect IO
               (println (-> String Unit)))
           (define (f : (-> String {} Unit)) (lambda (s) (perform println s)))"#,
    );
    assert!(
        err.contains("effect") || err.contains("unif"),
        "expected effect row error, got: {}",
        err
    );
}

#[test]
fn effect_row_unification_closed_compatible() {
    // Two closed rows with the same effects should unify
    let types = check_ok(
        r#"(effect IO
               (println (-> String Unit)))
           (define (f : (-> String {IO} Unit)) (lambda (s) (perform println s)))
           f"#,
    );
    let t = format_type(types.last().unwrap());
    assert!(t.contains("{IO}"), "expected {{IO}}, got: {}", t);
}

#[test]
fn effect_row_no_annotation_means_pure() {
    // (-> String Unit) with no effect row means pure — rejects effectful body
    let err = check_err(
        r#"(effect IO
               (println (-> String Unit)))
           (define (f : (-> String Unit)) (lambda (s) (perform println s)))"#,
    );
    assert!(
        err.contains("effect") || err.contains("{}"),
        "expected effect row error, got: {}",
        err
    );
}

#[test]
fn effect_row_no_annotation_accepts_pure() {
    // (-> String String) with no effect row means pure — accepts pure body
    let types = check_ok(
        r#"(define (f : (-> String String)) (lambda (s) s))
           (f "hello")"#,
    );
    assert_eq!(last_type(&types), "String");
}

#[test]
fn effect_row_handled_effect_removed() {
    // After handling, the IO effect should be removed from the row
    let types = check_ok(
        r#"(effect IO
               (println (-> String Unit)))
           (handle (perform println "hello")
               ((println msg k) -> (resume k (begin))))"#,
    );
    // The handle expression itself should be pure (IO was handled)
    // Verify the result type is Unit (the body returns Unit from println)
    assert_eq!(last_type(&types), "Unit");
}

// ===== Effect Handler Exhaustiveness =====

#[test]
fn handle_exhaustive_single_op_effect() {
    // Effect with one operation, handler covers it — should pass
    check_ok(
        r#"(effect Ask
               (ask (-> Unit Int)))
           (handle (perform ask ())
               ((ask k) -> (resume k 42)))"#,
    );
}

#[test]
fn handle_exhaustive_multi_op_effect() {
    // Effect with two operations, handler covers both — should pass
    check_ok(
        r#"(effect State
               (get (-> Unit Int))
               (put (-> Int Unit)))
           (handle
             (begin (perform put 10) (perform get ()))
             ((get k) -> (resume k 0))
             ((put v k) -> (resume k (begin))))"#,
    );
}

#[test]
fn handle_non_exhaustive_missing_one_op() {
    // Effect with two operations, handler only covers one — should fail
    let err = check_err(
        r#"(effect State
               (get (-> Unit Int))
               (put (-> Int Unit)))
           (handle
             (perform get ())
             ((get k) -> (resume k 0)))"#,
    );
    assert!(
        err.contains("Non-exhaustive effect handler"),
        "expected non-exhaustive handler error, got: {}",
        err
    );
    assert!(
        err.contains("State"),
        "expected error to mention 'State', got: {}",
        err
    );
    assert!(
        err.contains("put"),
        "expected error to mention missing op 'put', got: {}",
        err
    );
}

#[test]
fn handle_non_exhaustive_missing_multiple_ops() {
    // Effect with three operations, handler only covers one — should fail
    let err = check_err(
        r#"(effect Multi
               (op-a (-> Unit Int))
               (op-b (-> Int Unit))
               (op-c (-> Unit String)))
           (handle
             (perform op-a ())
             ((op-a k) -> (resume k 0)))"#,
    );
    assert!(
        err.contains("Non-exhaustive effect handler"),
        "expected non-exhaustive handler error, got: {}",
        err
    );
    assert!(
        err.contains("op-b"),
        "expected error to mention missing op 'op-b', got: {}",
        err
    );
    assert!(
        err.contains("op-c"),
        "expected error to mention missing op 'op-c', got: {}",
        err
    );
}

#[test]
fn handle_return_only_no_exhaustiveness_required() {
    // Handler with only a return clause and no effect operations — should pass
    // (no effect operations are being handled, so nothing to check)
    check_ok(
        r#"(handle 42
               ((return x) -> (+ x 1)))"#,
    );
}

#[test]
fn handle_no_effect_ops_in_scope() {
    // Handler with an unrecognized operation should fail with undefined variable
    let err = check_err(
        r#"(handle 42
               ((unknown-op k) -> 0))"#,
    );
    assert!(
        err.contains("Undefined variable") || err.contains("effect operation"),
        "expected undefined variable error, got: {}",
        err
    );
}

#[test]
fn handle_multiple_effects_all_exhaustive() {
    // Two effects, handler covers all operations from both — should pass
    check_ok(
        r#"(effect Eff1
               (op1 (-> Unit Int)))
           (effect Eff2
               (op2 (-> Unit String)))
           (handle
             (begin (perform op1 ()) (perform op2 ()))
             ((op1 k) -> (resume k 0))
             ((op2 k) -> (resume k "hi")))"#,
    );
}

#[test]
fn handle_multiple_effects_one_non_exhaustive() {
    // Two effects, handler covers all ops from one but not the other — should fail
    let err = check_err(
        r#"(effect Eff1
               (op1a (-> Unit Int))
               (op1b (-> Unit Int)))
           (effect Eff2
               (op2 (-> Unit String)))
           (handle
             (begin (perform op1a ()) (perform op2 ()))
             ((op1a k) -> (resume k 0))
             ((op2 k) -> (resume k "hi")))"#,
    );
    assert!(
        err.contains("Non-exhaustive effect handler"),
        "expected non-exhaustive handler error, got: {}",
        err
    );
    assert!(
        err.contains("Eff1"),
        "expected error to mention 'Eff1', got: {}",
        err
    );
    assert!(
        err.contains("op1b"),
        "expected error to mention missing op 'op1b', got: {}",
        err
    );
}

#[test]
fn handle_exception_single_op_exhaustive() {
    // Exception has only one op (throw), so handling it is automatically exhaustive
    check_ok(
        r#"(effect Exception
               (throw (-> e a)))
           (handle
             (perform throw "error")
             ((throw msg k) -> msg))"#,
    );
}

#[test]
fn handle_effect_removal_verified() {
    // Verify that handled effects are actually removed from the effect row.
    // A function annotated as pure should accept a handle expression that
    // handles all effects from its body.
    check_ok(
        r#"(effect MyEff
               (my-op (-> Unit Int)))
           (define (f : (-> Unit Int))
             (lambda ()
               (handle
                 (perform my-op ())
                 ((my-op k) -> (resume k 42)))))"#,
    );
}

// ===== Polymorphic throw: custom ADT error values =====

#[test]
fn throw_polymorphic_custom_adt() {
    // Throw a custom ADT and pattern match on it in the handler
    check_ok(
        r#"(data AppError (NotFound String) (InvalidInput String Int))
           (handle
             (perform throw (InvalidInput "age" -1))
             ((throw err k) ->
               (match err
                 ((NotFound msg) 0)
                 ((InvalidInput field code) code))))"#,
    );
}

#[test]
fn throw_polymorphic_recursive_adt() {
    // Throw a recursive ADT and destructure it in the handler
    check_ok(
        r#"(data ErrorChain
             (RootCause String)
             (Wrapped String ErrorChain))
           (handle
             (perform throw (Wrapped "outer" (RootCause "inner")))
             ((throw err k) ->
               (match err
                 ((RootCause msg) msg)
                 ((Wrapped msg _) msg))))"#,
    );
}

#[test]
fn throw_polymorphic_exhaustive_multi_constructor_match() {
    // Throw a custom ADT with three constructors; handler matches all exhaustively
    check_ok(
        r#"(data AppError (NotFound String) (InvalidInput String Int) Unauthorized)
           (handle
             (perform throw (NotFound "user"))
             ((throw err k) ->
               (match err
                 ((NotFound resource) (string-append "not found: " resource))
                 ((InvalidInput field code) (string-append "bad input: " field))
                 (Unauthorized "access denied"))))"#,
    );
}
