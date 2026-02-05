// Benchmark runner: compares tree-walker vs bytecode VM
import * as Lexer from '../src/core/lexer';
import * as Parser from '../src/core/parser';
import * as Interpreter from '../src/core/interpreter';
import { compileProgram, createVM, interpret, formatVMValue } from '../src/vm';
import '../src/core/io-handlers';

const files = ['fib', 'fib-tail', 'sum-loop', 'pattern', 'effects'];
const WARMUP = 2;
const RUNS = 5;

async function main() {
  console.log('Marlang Benchmark: Tree-Walker vs Bytecode VM');
  console.log('='.repeat(60));
  console.log(`Warmup: ${WARMUP} runs, Measured: ${RUNS} runs (median)\n`);

  for (const name of files) {
    const filePath = `bench/${name}.mar`;
    let source: string;
    try {
      source = await Bun.file(filePath).text();
    } catch {
      console.log(`  ${name}: SKIP (file not found)`);
      continue;
    }

    const tokens = Lexer.lex(Lexer.empty(source));
    const parsed = Parser.parse(Parser.empty(tokens));

    // Tree-walker
    const twTimes: number[] = [];
    let twResult: string = '';
    for (let i = 0; i < WARMUP + RUNS; i++) {
      const start = performance.now();
      const result = Interpreter.evalMany(Interpreter.empty(), parsed.exprs);
      const elapsed = performance.now() - start;
      if (i >= WARMUP) twTimes.push(elapsed);
      twResult = Interpreter.formatValue(result.value);
    }

    // VM
    const vmTimes: number[] = [];
    let vmResult: string = '';
    const chunk = compileProgram(parsed.exprs);
    for (let i = 0; i < WARMUP + RUNS; i++) {
      const vm = createVM();
      const start = performance.now();
      const result = interpret(vm, chunk);
      const elapsed = performance.now() - start;
      if (i >= WARMUP) vmTimes.push(elapsed);
      const { formatValue } = require('../src/core/interpreter');
      vmResult = formatValue(result);
    }

    const twMedian = median(twTimes);
    const vmMedian = median(vmTimes);
    const speedup = twMedian / vmMedian;
    const match = twResult === vmResult ? 'OK' : `MISMATCH (tw=${twResult}, vm=${vmResult})`;

    console.log(`  ${name.padEnd(12)} TW: ${fmt(twMedian)}  VM: ${fmt(vmMedian)}  ${speedup.toFixed(2)}x  ${match}`);
  }

  console.log('\n' + '='.repeat(60));
}

function median(arr: number[]): number {
  const sorted = [...arr].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 !== 0 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
}

function fmt(ms: number): string {
  if (ms < 1) return `${(ms * 1000).toFixed(0)}µs`.padStart(8);
  return `${ms.toFixed(2)}ms`.padStart(8);
}

main();
