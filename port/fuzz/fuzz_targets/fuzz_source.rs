#![no_main]

use libfuzzer_sys::fuzz_target;
use marlang_core::intern::Interner;
use marlang_core::lexer::lex;
use marlang_core::parser::parse_with_interner;
use marlang_types::checker::type_check;

/// Minimal prelude for type-checker fuzzing: data types, builtin declares, and effects.
/// Mirrors the TEST_PRELUDE from tests.rs.
const FUZZ_PRELUDE: &str = r#"
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
  (throw (-> String a)))
"#;

fuzz_target!(|data: &[u8]| {
    // Interpret raw bytes as UTF-8; skip invalid inputs
    let source = match std::str::from_utf8(data) {
        Ok(s) => s,
        Err(_) => return,
    };

    // Cap input size to avoid slow iterations on huge inputs
    if source.len() > 4096 {
        return;
    }

    let interner = Interner::new();

    // Lex the prelude + user source combined
    let full_source = format!("{}\n{}", FUZZ_PRELUDE, source);
    let tokens = match lex(&full_source, &interner) {
        Ok(t) => t,
        Err(_) => return, // Lex errors are expected for random input
    };

    // Parse
    let exprs = match parse_with_interner(&tokens, &interner) {
        Ok(e) => e,
        Err(_) => return, // Parse errors are expected for random input
    };

    // Type check — should never panic, only return Ok or Err
    let _ = type_check(&exprs, &interner);
});
