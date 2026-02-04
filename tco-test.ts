import * as Lexer from './lexer';
import * as Parser from './parser';
import * as Interpreter from './interpreter';

// Helper to run code and return result
const run = (interp: Interpreter.T, code: string): { interp: Interpreter.T, value: string } => {
  const tokens = Lexer.lex(Lexer.empty(code));
  const parsed = Parser.parse(Parser.empty(tokens));
  const result = Interpreter.evalMany(interp, parsed.exprs);
  return { interp: result.interp, value: Interpreter.formatValue(result.value) };
};

console.log('=== Tail Call Optimization Tests ===\n');

let interp = Interpreter.empty();

// Test 1: Tail-recursive factorial (would overflow without TCO)
console.log('--- Test 1: Tail-recursive factorial ---');
const factTailCode = `
(define fact-tail (lambda (n acc)
  (if (= n 0)
    acc
    (fact-tail (- n 1) (* n acc)))))
`;
let result = run(interp, factTailCode);
interp = result.interp;
console.log('Defined fact-tail');

// Test with small value first
result = run(interp, '(fact-tail 10 1)');
console.log(`(fact-tail 10 1) => ${result.value}`);
if (result.value !== '3628800') {
  console.error('FAIL: Expected 3628800');
  process.exit(1);
}

// Test with large value - this would stack overflow without TCO
console.log('Testing (fact-tail 50000 1) - this would overflow without TCO...');
const startTime = performance.now();
result = run(interp, '(fact-tail 50000 1)');
const endTime = performance.now();
console.log(`(fact-tail 50000 1) completed in ${(endTime - startTime).toFixed(2)}ms`);
console.log(`Result: ${result.value.length > 50 ? result.value.slice(0, 50) + '...' : result.value}`);
console.log('PASS: No stack overflow!');

// Test 2: Mutual recursion
console.log('\n--- Test 2: Mutual recursion ---');
const isEvenCode = `
(define is-even (lambda (n)
  (if (= n 0) 1 (is-odd (- n 1)))))
`;
const isOddCode = `
(define is-odd (lambda (n)
  (if (= n 0) 0 (is-even (- n 1)))))
`;

result = run(interp, isEvenCode);
interp = result.interp;
result = run(interp, isOddCode);
interp = result.interp;
console.log('Defined is-even and is-odd');

// Test small values
result = run(interp, '(is-even 4)');
console.log(`(is-even 4) => ${result.value}`);
if (result.value !== '1') {
  console.error('FAIL: Expected 1 (true)');
  process.exit(1);
}

result = run(interp, '(is-even 5)');
console.log(`(is-even 5) => ${result.value}`);
if (result.value !== '0') {
  console.error('FAIL: Expected 0 (false)');
  process.exit(1);
}

// Test with large value - this would stack overflow without TCO
console.log('Testing (is-even 50000) - this would overflow without TCO...');
const startTime2 = performance.now();
result = run(interp, '(is-even 50000)');
const endTime2 = performance.now();
console.log(`(is-even 50000) => ${result.value} in ${(endTime2 - startTime2).toFixed(2)}ms`);
if (result.value !== '1') {
  console.error('FAIL: Expected 1 (true)');
  process.exit(1);
}
console.log('PASS: Mutual recursion works!');

// Test 3: Non-tail recursion still works
console.log('\n--- Test 3: Non-tail recursion still works ---');
const factCode = `
(define fact (lambda (n)
  (if (= n 0) 1 (* n (fact (- n 1))))))
`;
result = run(interp, factCode);
interp = result.interp;
result = run(interp, '(fact 10)');
console.log(`(fact 10) => ${result.value}`);
if (result.value !== '3628800') {
  console.error('FAIL: Expected 3628800');
  process.exit(1);
}
console.log('PASS: Non-tail recursion works!');

// Test 4: Tail calls in let bodies
console.log('\n--- Test 4: Tail calls in let bodies ---');
const letTailCode = `
(define count-down (lambda (n)
  (let ((m (- n 1)))
    (if (= m 0)
      0
      (count-down m)))))
`;
result = run(interp, letTailCode);
interp = result.interp;
console.log('Defined count-down');

console.log('Testing (count-down 50000) with tail call in let body...');
const startTime3 = performance.now();
result = run(interp, '(count-down 50000)');
const endTime3 = performance.now();
console.log(`(count-down 50000) => ${result.value} in ${(endTime3 - startTime3).toFixed(2)}ms`);
if (result.value !== '0') {
  console.error('FAIL: Expected 0');
  process.exit(1);
}
console.log('PASS: Tail calls in let bodies work!');

// Test 5: Tail calls in cond
console.log('\n--- Test 5: Tail calls in cond ---');
const condTailCode = `
(define count-cond (lambda (n)
  (cond
    ((= n 0) 0)
    (else (count-cond (- n 1))))))
`;
result = run(interp, condTailCode);
interp = result.interp;
console.log('Defined count-cond');

console.log('Testing (count-cond 50000) with tail call in cond...');
const startTime4 = performance.now();
result = run(interp, '(count-cond 50000)');
const endTime4 = performance.now();
console.log(`(count-cond 50000) => ${result.value} in ${(endTime4 - startTime4).toFixed(2)}ms`);
if (result.value !== '0') {
  console.error('FAIL: Expected 0');
  process.exit(1);
}
console.log('PASS: Tail calls in cond work!');

console.log('\n=== All TCO tests passed! ===');
