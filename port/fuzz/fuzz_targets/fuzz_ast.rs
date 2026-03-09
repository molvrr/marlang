#![no_main]

use arbitrary::Unstructured;
use libfuzzer_sys::fuzz_target;
use marlang_core::ast::{Expr, LispValue};
use marlang_core::intern::Interner;
use marlang_core::span::Span;
use marlang_types::checker::type_check;
use std::sync::Arc;

/// Pool of symbol names covering special forms, variables, types, and constructors.
/// The fuzzer picks from these to build realistic ASTs.
const SYMBOL_POOL: &[&str] = &[
    // Special forms
    "define",
    "lambda",
    "if",
    "let",
    "begin",
    "cond",
    "match",
    "data",
    "effect",
    "perform",
    "handle",
    "resume",
    "declare",
    "quote",
    "import",
    "foreign",
    "foreign-lib",
    "ffi-struct",
    "ffi-call",
    "->",
    "...",
    ":",
    // Variable names
    "x",
    "y",
    "z",
    "f",
    "g",
    "h",
    "a",
    "b",
    "n",
    "m",
    "acc",
    "rest",
    "k",
    "v",
    "msg",
    "self",
    "_",
    // Type names
    "Int",
    "Float",
    "Bool",
    "String",
    "Char",
    "Unit",
    "Option",
    "List",
    "Pair",
    "Result",
    // Constructors
    "None",
    "Some",
    "Nil",
    "Cons",
    "MkPair",
    "Ok",
    "Err",
    // Builtins
    "+",
    "-",
    "*",
    "/",
    "=",
    "<",
    ">",
    "show",
    "equal?",
    "list",
    "error",
    "string-append",
    "string-length",
    // Boolean literals
    "#t",
    "#f",
    // Effect-related
    "__effect_row",
    "__row_var",
    "IO",
    "Exception",
    "throw",
    "print",
    "println",
    "read-line",
];

/// Prelude source to establish the type environment (same as fuzz_source).
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

/// Maximum depth for recursive Expr generation to prevent stack overflow.
const MAX_DEPTH: usize = 8;

/// Maximum number of elements in a generated list.
const MAX_LIST_LEN: usize = 6;

/// Maximum number of top-level expressions to generate.
const MAX_TOP_LEVEL: usize = 5;

/// Generate a random Expr from fuzzer bytes, respecting depth limits.
fn arbitrary_expr(
    u: &mut Unstructured,
    interner: &Interner,
    depth: usize,
) -> arbitrary::Result<Expr> {
    if depth >= MAX_DEPTH {
        // At max depth, only generate atoms or nil
        return arbitrary_atom(u, interner);
    }

    // Choose between atom, nil, and list with bias toward interesting structures
    let choice: u8 = u.int_in_range(0..=9)?;
    match choice {
        0..=3 => arbitrary_atom(u, interner),
        4 => Ok(Expr::Nil(Span::DUMMY)),
        5..=9 => arbitrary_list(u, interner, depth),
        _ => unreachable!(),
    }
}

/// Generate a random atom (symbol, number, or string).
fn arbitrary_atom(u: &mut Unstructured, interner: &Interner) -> arbitrary::Result<Expr> {
    let choice: u8 = u.int_in_range(0..=2)?;
    match choice {
        0 => {
            // Symbol from pool
            let idx = u.choose_index(SYMBOL_POOL.len())?;
            let sym = interner.intern(SYMBOL_POOL[idx]);
            Ok(Expr::Atom(LispValue::Symbol(sym), Span::DUMMY))
        }
        1 => {
            // Number
            let is_float: bool = u.arbitrary()?;
            let n: f64 = if is_float {
                // Small float for interesting behavior
                let raw: i16 = u.arbitrary()?;
                raw as f64 / 100.0
            } else {
                // Small integer
                let raw: i16 = u.arbitrary()?;
                raw as f64
            };
            Ok(Expr::Atom(LispValue::Number(n, is_float), Span::DUMMY))
        }
        2 => {
            // Short string
            let len = u.int_in_range(0..=8_usize)?;
            let bytes: Vec<u8> = (0..len)
                .map(|_| u.int_in_range(0x20..=0x7e_u8))
                .collect::<arbitrary::Result<_>>()?;
            let s = String::from_utf8(bytes).unwrap_or_default();
            Ok(Expr::Atom(
                LispValue::String(Arc::from(s.as_str())),
                Span::DUMMY,
            ))
        }
        _ => unreachable!(),
    }
}

/// Generate a random cons-list of expressions.
fn arbitrary_list(
    u: &mut Unstructured,
    interner: &Interner,
    depth: usize,
) -> arbitrary::Result<Expr> {
    let len = u.int_in_range(1..=MAX_LIST_LEN)?;
    let mut result = Expr::Nil(Span::DUMMY);
    // Build cons list right-to-left
    for _ in 0..len {
        let elem = arbitrary_expr(u, interner, depth + 1)?;
        result = Expr::List {
            head: Box::new(elem),
            tail: Box::new(result),
            span: Span::DUMMY,
        };
    }
    Ok(result)
}

fuzz_target!(|data: &[u8]| {
    // Cap input size
    if data.len() > 2048 {
        return;
    }

    let interner = Interner::new();

    // Parse the prelude to get the base expressions
    let prelude_tokens = match marlang_core::lexer::lex(FUZZ_PRELUDE, &interner) {
        Ok(t) => t,
        Err(_) => return,
    };
    let prelude_exprs = match marlang_core::parser::parse_with_interner(&prelude_tokens, &interner)
    {
        Ok(e) => e,
        Err(_) => return,
    };

    // Generate random AST expressions from the fuzzer bytes
    let mut u = Unstructured::new(data);
    let count = match u.int_in_range(1..=MAX_TOP_LEVEL) {
        Ok(c) => c,
        Err(_) => return,
    };

    let mut all_exprs = prelude_exprs;
    for _ in 0..count {
        match arbitrary_expr(&mut u, &interner, 0) {
            Ok(expr) => all_exprs.push(expr),
            Err(_) => break,
        }
    }

    // Type check — should never panic
    let _ = type_check(&all_exprs, &interner);
});
