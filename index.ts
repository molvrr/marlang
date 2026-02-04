import * as Lexer from './lexer';
import * as Parser from './parser';
import * as Interpreter from './interpreter';

// Helper to run code and print result
const run = (interp: Interpreter.T, code: string): Interpreter.T => {
  const tokens = Lexer.lex(Lexer.empty(code));
  const parsed = Parser.parse(Parser.empty(tokens));
  const result = Interpreter.evalMany(interp, parsed.exprs);

  for (const expr of parsed.exprs) {
    // Skip printing nil for define statements
    if (expr.kind === 'list') {
      const head = expr.head;
      if (head.kind === 'atom' && head.value.kind === 'symbol' && head.value.value === 'define') {
        continue;
      }
    }
  }

  console.log(`${code}`);
  console.log(`  => ${Interpreter.formatValue(result.value)}`);
  return result.interp;
};

// Run all tests
console.log('=== Marlang Interpreter Tests ===\n');

let interp = Interpreter.empty();

// Basic arithmetic
console.log('--- Arithmetic ---');
interp = run(interp, '(+ 1 2)');
interp = run(interp, '(- 10 3)');
interp = run(interp, '(* 4 5)');
interp = run(interp, '(/ 20 4)');
interp = run(interp, '(+ 1 2 3 4 5)');

// Variables
console.log('\n--- Variables ---');
interp = run(interp, '(define x 10)');
interp = run(interp, '(+ x 5)');

// Functions
console.log('\n--- Functions ---');
interp = run(interp, '(define square (lambda (n) (* n n)))');
interp = run(interp, '(square 4)');

// Conditionals
console.log('\n--- Conditionals ---');
interp = run(interp, '(if (< 1 2) 10 20)');
interp = run(interp, '(if (> 1 2) 10 20)');

// Let bindings
console.log('\n--- Let Bindings ---');
interp = run(interp, '(let ((a 1) (b 2)) (+ a b))');

// Recursion (factorial)
console.log('\n--- Recursion ---');
interp = run(interp, '(define fact (lambda (n) (if (= n 0) 1 (* n (fact (- n 1))))))');
interp = run(interp, '(fact 5)');

// Closures
console.log('\n--- Closures ---');
interp = run(interp, '(define make-adder (lambda (n) (lambda (x) (+ n x))))');
interp = run(interp, '((make-adder 5) 10)');

// Cond
console.log('\n--- Cond ---');
interp = run(interp, '(define grade (lambda (score) (cond ((>= score 90) 1) ((>= score 80) 2) ((>= score 70) 3) (else 4))))');
interp = run(interp, '(grade 95)');
interp = run(interp, '(grade 85)');
interp = run(interp, '(grade 65)');

// Strings
console.log('\n--- Strings ---');
interp = run(interp, '"hello world"');
interp = run(interp, '(list "a" "b" "c")');

// FFI with libm
console.log('\n--- FFI (libm) ---');
interp = run(interp, '(define math (ffi-open "m" (list (list "sin" (list "f64") "f64") (list "cos" (list "f64") "f64") (list "pow" (list "f64" "f64") "f64"))))');
interp = run(interp, '(ffi-call math "sin" 0.0)');
interp = run(interp, '(ffi-call math "sin" 1.5707963267948966)');
interp = run(interp, '(ffi-call math "cos" 0.0)');
interp = run(interp, '(ffi-call math "pow" 2.0 10.0)');
interp = run(interp, '(ffi-close math)');

console.log('\n=== All tests completed ===');
