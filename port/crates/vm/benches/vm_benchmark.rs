use criterion::{black_box, criterion_group, criterion_main, Criterion};
use marlang_core::intern::Interner;
use marlang_core::lexer::lex;
use marlang_core::parser::parse;
use marlang_vm::builtins::register_builtins;
use marlang_vm::compiler::compile_program;
use marlang_vm::vm::{interpret, VM};

fn bench_fib(c: &mut Criterion) {
    let source = r#"
(define fib (lambda (n)
  (if (< n 2)
    n
    (+ (fib (- n 1)) (fib (- n 2))))))
(fib 20)
"#;

    let interner = Interner::new();
    let tokens = lex(source, &interner).unwrap();
    let exprs = parse(&tokens).unwrap();
    let chunk = compile_program(&exprs, &interner).unwrap();

    c.bench_function("fib(20)", |b| {
        b.iter(|| {
            let mut vm = VM::new();
            register_builtins(&mut vm);
            let result = interpret(&mut vm, &chunk).unwrap();
            black_box(result);
        });
    });
}

fn bench_sum_loop(c: &mut Criterion) {
    let source = r#"
(define sum (lambda (n acc)
  (if (= n 0)
    acc
    (sum (- n 1) (+ acc n)))))
(sum 10000 0)
"#;

    let interner = Interner::new();
    let tokens = lex(source, &interner).unwrap();
    let exprs = parse(&tokens).unwrap();
    let chunk = compile_program(&exprs, &interner).unwrap();

    c.bench_function("sum 1..10000", |b| {
        b.iter(|| {
            let mut vm = VM::new();
            register_builtins(&mut vm);
            let result = interpret(&mut vm, &chunk).unwrap();
            black_box(result);
        });
    });
}

fn bench_deep_call_stack(c: &mut Criterion) {
    // Tests frame management performance
    let source = r#"
(define deep (lambda (n)
  (if (= n 0)
    42
    (deep (- n 1)))))
(deep 1000)
"#;

    let interner = Interner::new();
    let tokens = lex(source, &interner).unwrap();
    let exprs = parse(&tokens).unwrap();
    let chunk = compile_program(&exprs, &interner).unwrap();

    c.bench_function("deep recursion 1000", |b| {
        b.iter(|| {
            let mut vm = VM::new();
            register_builtins(&mut vm);
            let result = interpret(&mut vm, &chunk).unwrap();
            black_box(result);
        });
    });
}

fn bench_list_operations(c: &mut Criterion) {
    // Using list constructor instead of cons
    let source = r#"
(define build-list (lambda (n acc)
  (if (= n 0)
    acc
    (build-list (- n 1) (list n acc)))))
(build-list 1000 nil)
"#;

    let interner = Interner::new();
    let tokens = lex(source, &interner).unwrap();
    let exprs = parse(&tokens).unwrap();
    let chunk = compile_program(&exprs, &interner).unwrap();

    c.bench_function("build list 1000", |b| {
        b.iter(|| {
            let mut vm = VM::new();
            register_builtins(&mut vm);
            let result = interpret(&mut vm, &chunk).unwrap();
            black_box(result);
        });
    });
}

fn bench_closure_creation(c: &mut Criterion) {
    let source = r#"
(define make-adder (lambda (x)
  (lambda (y) (+ x y))))
(define add5 (make-adder 5))
(add5 10)
"#;

    let interner = Interner::new();
    let tokens = lex(source, &interner).unwrap();
    let exprs = parse(&tokens).unwrap();
    let chunk = compile_program(&exprs, &interner).unwrap();

    c.bench_function("closure creation", |b| {
        b.iter(|| {
            let mut vm = VM::new();
            register_builtins(&mut vm);
            let result = interpret(&mut vm, &chunk).unwrap();
            black_box(result);
        });
    });
}

criterion_group!(
    benches,
    bench_fib,
    bench_sum_loop,
    bench_deep_call_stack,
    bench_list_operations,
    bench_closure_creation
);
criterion_main!(benches);
