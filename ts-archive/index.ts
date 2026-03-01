// Marlang CLI
import * as readline from 'readline';
import * as path from 'path';
import * as Lexer from './src/core/lexer';
import * as Parser from './src/core/parser';
import * as Interpreter from './src/core/interpreter';
import { typeCheck, formatError, formatTypes } from './src/types/type-checker';
import { compileProgram, createVM, interpret, formatVMValue, VMError, disassemble } from './src/vm';
import { getLastRunFibersPromise } from './src/vm/scheduler';

// Register native IO handlers
import './src/core/io-handlers';

const command = process.argv[2];

const printUsage = () => {
  console.log(`Usage: mar <command> [options]

Commands:
  run <file.mar> [--vm] [--no-typecheck]  Run a Marlang file
  repl                                     Start interactive REPL
  typecheck <file.mar> [--verbose]         Type check a file without running
  disasm <file.mar>                        Disassemble bytecode
  lsp                                      Start the Language Server Protocol server

Examples:
  mar run examples/factorial.mar
  mar run examples/factorial.mar --vm
  mar run examples/factorial.mar --no-typecheck
  mar repl
  mar typecheck examples/factorial.mar
  mar disasm examples/factorial.mar
  mar lsp`);
};

const runFile = async () => {
  const filename = process.argv[3];
  const noTypecheck = process.argv.includes('--no-typecheck');
  const useVM = process.argv.includes('--vm');

  if (!filename) {
    console.error('Error: Missing filename');
    console.error('Usage: mar run <file.mar> [--vm] [--no-typecheck]');
    process.exit(1);
  }

  const file = Bun.file(filename);
  const code = await file.text();

  try {
    const tokens = Lexer.lex(Lexer.empty(code));
    const parsed = Parser.parse(Parser.empty(tokens));
    const absolutePath = path.resolve(filename);

    // Type check unless --no-typecheck flag is provided
    if (!noTypecheck) {
      const typeResult = typeCheck(parsed.exprs, absolutePath);
      if (!typeResult.ok) {
        console.error(`Type error: ${formatError(typeResult.error)}`);
        process.exit(1);
      }
    }

    if (useVM) {
      const chunk = compileProgram(parsed.exprs);
      const vm = createVM();
      const result = interpret(vm, chunk);

      // Check if run-fibers was called — if so, await the async scheduler
      const fibersPromise = getLastRunFibersPromise();
      if (fibersPromise) {
        await fibersPromise;
      } else if (result.kind !== 'nil') {
        const { formatValue } = require('./src/core/interpreter');
        console.log(formatValue(result));
      }
    } else {
      const result = Interpreter.evalMany(Interpreter.empty(absolutePath), parsed.exprs);

      if (result.value.kind !== 'nil') {
        console.log(Interpreter.formatValue(result.value));
      }
    }
  } catch (e) {
    if (e instanceof VMError) {
      console.error(`VM Error: ${e.message}`);
      console.error(e.formatTrace());
      process.exit(1);
    }
    console.error(`Error: ${e instanceof Error ? e.message : String(e)}`);
    process.exit(1);
  }
};

const startRepl = () => {
  const rl = readline.createInterface({
    input: process.stdin,
    output: process.stdout,
  });

  let interp = Interpreter.empty();

  const evaluate = (code: string): string => {
    try {
      const tokens = Lexer.lex(Lexer.empty(code));
      const parsed = Parser.parse(Parser.empty(tokens));
      const result = Interpreter.evalMany(interp, parsed.exprs);
      interp = result.interp;
      return Interpreter.formatValue(result.value);
    } catch (e) {
      return `Error: ${e instanceof Error ? e.message : String(e)}`;
    }
  };

  const prompt = () => {
    rl.question('marlang> ', (input) => {
      if (input === null || input === '.exit' || input === '(quit)') {
        console.log('Goodbye!');
        rl.close();
        return;
      }

      if (input.trim()) {
        console.log(`  => ${evaluate(input)}`);
      }

      prompt();
    });
  };

  console.log('Marlang REPL - Type .exit or (quit) to exit');
  prompt();
};

const typecheckFile = async () => {
  const filename = process.argv[3];
  const verbose = process.argv.includes('--verbose');

  if (!filename) {
    console.error('Error: Missing filename');
    console.error('Usage: mar typecheck <file.mar> [--verbose]');
    process.exit(1);
  }

  const file = Bun.file(filename);
  const code = await file.text();

  try {
    const tokens = Lexer.lex(Lexer.empty(code));
    const parsed = Parser.parse(Parser.empty(tokens));
    const absolutePath = path.resolve(filename);

    const typeResult = typeCheck(parsed.exprs, absolutePath);

    if (!typeResult.ok) {
      console.error(`Type error: ${formatError(typeResult.error)}`);
      process.exit(1);
    }

    if (verbose) {
      const types = formatTypes(typeResult);
      console.log('Inferred types:');
      types.forEach((type, i) => {
        console.log(`  ${i + 1}: ${type}`);
      });
    } else {
      console.log(`Type check passed: ${filename}`);
    }
  } catch (e) {
    console.error(`Error: ${e instanceof Error ? e.message : String(e)}`);
    process.exit(1);
  }
};

const disasmFile = async () => {
  const filename = process.argv[3];

  if (!filename) {
    console.error('Error: Missing filename');
    console.error('Usage: mar disasm <file.mar>');
    process.exit(1);
  }

  const file = Bun.file(filename);
  const code = await file.text();

  try {
    const tokens = Lexer.lex(Lexer.empty(code));
    const parsed = Parser.parse(Parser.empty(tokens));
    const chunk = compileProgram(parsed.exprs);
    console.log(disassemble(chunk));
  } catch (e) {
    console.error(`Error: ${e instanceof Error ? e.message : String(e)}`);
    process.exit(1);
  }
};

switch (command) {
  case 'run':
    await runFile();
    break;
  case 'repl':
    startRepl();
    break;
  case 'typecheck':
    await typecheckFile();
    break;
  case 'disasm':
    await disasmFile();
    break;
  case 'lsp':
    await import('./lsp/server');
    break;
  default:
    printUsage();
    if (command) {
      process.exit(1);
    }
}
