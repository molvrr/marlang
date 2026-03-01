use marlang_core::intern::Interner;
use marlang_core::lexer::lex;
use marlang_core::parser::parse_with_interner;

use crate::env::*;
use crate::infer::infer_program;

/// Builtin type declarations expressed in Marlang syntax.
/// These are the same `(declare ...)` forms that appear in stdlib/prelude.mar.
/// They are embedded here so that `create_builtin_env()` can produce a
/// fully-populated type environment without requiring filesystem access.
///
/// The canonical source of truth is stdlib/prelude.mar — if you add a new
/// builtin, add its `(declare ...)` in BOTH places.
const BUILTIN_DECLARATIONS: &str = r#"
; Data types needed by builtin signatures
(data (Option a) None (Some a))
(data (List a) Nil (Cons a (List a)))
(data (Pair a b) (MkPair a b))
(data (Result e a) (Err e) (Ok a))

; Int arithmetic
(declare + (-> Int Int Int))
(declare - (-> Int Int Int))
(declare * (-> Int Int Int))
(declare / (-> Int Int Int))
(declare negate (-> Int Int))

; Float arithmetic
(declare +. (-> Float Float Float))
(declare -. (-> Float Float Float))
(declare *. (-> Float Float Float))
(declare /. (-> Float Float Float))
(declare negate. (-> Float Float))

; Int comparison
(declare = (-> Int Int Bool))
(declare < (-> Int Int Bool))
(declare > (-> Int Int Bool))
(declare <= (-> Int Int Bool))
(declare >= (-> Int Int Bool))

; Float comparison
(declare =. (-> Float Float Bool))
(declare <. (-> Float Float Bool))
(declare >. (-> Float Float Bool))
(declare <=. (-> Float Float Bool))
(declare >=. (-> Float Float Bool))

; String comparison
(declare string= (-> String String Bool))
(declare string< (-> String String Bool))
(declare string> (-> String String Bool))
(declare string<= (-> String String Bool))
(declare string>= (-> String String Bool))

; Polymorphic
(declare equal? (-> a a Bool))
(declare show (-> a String))
(declare list (-> (... a) (List a)))
(declare error (-> String a))

; nil constant
(declare nil Unit)

; Math — Int
(declare abs (-> Int Int))
(declare rem (-> Int Int Int))
(declare mod (-> Int Int Int))

; Math — Float
(declare abs. (-> Float Float))
(declare rem. (-> Float Float Float))
(declare mod. (-> Float Float Float))

; Conversion
(declare int->float (-> Int Float))
(declare floor (-> Float Int))
(declare ceil (-> Float Int))
(declare round (-> Float Int))
(declare truncate (-> Float Int))

; Math — Float functions
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

; String builtins
(declare string-length (-> String Int))
(declare string-concat (-> (... String) String))
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
(declare string-char-at (-> String Int (Option String)))
(declare string-split (-> String String (List String)))
(declare string-join (-> String (List String) String))
(declare string-replace (-> String String String String))
(declare string-replace-all (-> String String String String))
(declare string-pad-start (-> String Int String String))
(declare string-pad-end (-> String Int String String))
(declare char-code (-> String Int))
(declare char-from-code (-> Int String))

; Number/string conversion
(declare number->string (-> Float String))
(declare __try-string->number (-> String (Option Float)))

; Concurrency
(declare run-fibers (-> (-> Unit {| e} a) a))
(declare run-fibers-parallel (-> (-> Unit {| e} a) Int a))

; Profiling
(declare time-it (-> (-> Unit {| e} a) (Pair a Float)))

; FFI
(declare ffi-open (-> String (List a) FFILibrary))
(declare ffi-call (-> FFILibrary String a))
(declare ffi-close (-> FFILibrary Unit))
(declare ffi-struct (-> String (List a) FFIStructDef))
(declare struct-new (-> FFIStructDef a FFIStruct))
(declare struct-get (-> FFIStruct String a))
(declare struct-set (-> FFIStruct String a Unit))

; Macro/quote builtins
(declare make-symbol (-> String Symbol))
(declare gensym (-> Unit Symbol))
(declare symbol->string (-> Symbol String))
(declare string->symbol (-> String Symbol))

; Type predicates
(declare symbol? (-> a Bool))
(declare pair? (-> a Bool))
(declare nil? (-> a Bool))
(declare number? (-> a Bool))
(declare string? (-> a Bool))
(declare list? (-> a Bool))
"#;

/// Create the initial type environment by parsing builtin declarations.
///
/// This parses the embedded `(declare ...)` and `(data ...)` forms to produce
/// a type environment with all builtin type signatures. The same declarations
/// also appear in `stdlib/prelude.mar` for the full CLI pipeline.
pub fn create_builtin_env() -> TypeEnv {
    let interner = Interner::new();
    let tokens = lex(BUILTIN_DECLARATIONS, &interner).expect("builtin declarations should lex");
    let exprs = parse_with_interner(&tokens, &interner).expect("builtin declarations should parse");

    let env = empty_env();
    match infer_program(&env, &exprs, &interner) {
        Ok((env, _)) => env,
        Err(e) => panic!("builtin declarations should type-check: {:?}", e),
    }
}
