// Marlang CLI
import * as readline from 'readline';
import * as Lexer from './lexer';
import * as Parser from './parser';
import * as Interpreter from './interpreter';
import { typeCheck, formatError, formatTypes } from './type-checker';

const command = process.argv[2];

const printUsage = () => {
  console.log(`Usage: bun index.ts <command> [options]

Commands:
  run <file.mar> [--no-typecheck]  Run a Marlang file
  repl                             Start interactive REPL
  typecheck <file.mar> [--verbose] Type check a file without running
  lsp                              Start the Language Server Protocol server

Examples:
  bun index.ts run examples/factorial.mar
  bun index.ts run examples/factorial.mar --no-typecheck
  bun index.ts repl
  bun index.ts typecheck examples/factorial.mar
  bun index.ts typecheck examples/factorial.mar --verbose
  bun index.ts lsp`);
};

const runFile = async () => {
  const filename = process.argv[3];
  const noTypecheck = process.argv.includes('--no-typecheck');

  if (!filename) {
    console.error('Error: Missing filename');
    console.error('Usage: bun index.ts run <file.mar> [--no-typecheck]');
    process.exit(1);
  }

  const file = Bun.file(filename);
  const code = await file.text();

  try {
    const tokens = Lexer.lex(Lexer.empty(code));
    const parsed = Parser.parse(Parser.empty(tokens));

    // Type check unless --no-typecheck flag is provided
    if (!noTypecheck) {
      const typeResult = typeCheck(parsed.exprs);
      if (!typeResult.ok) {
        console.error(`Type error: ${formatError(typeResult.error)}`);
        process.exit(1);
      }
    }

    const result = Interpreter.evalMany(Interpreter.empty(), parsed.exprs);

    // Print the final result if it's not nil
    if (result.value.kind !== 'nil') {
      console.log(Interpreter.formatValue(result.value));
    }
  } catch (e) {
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
    console.error('Usage: bun index.ts typecheck <file.mar> [--verbose]');
    process.exit(1);
  }

  const file = Bun.file(filename);
  const code = await file.text();

  try {
    const tokens = Lexer.lex(Lexer.empty(code));
    const parsed = Parser.parse(Parser.empty(tokens));

    const typeResult = typeCheck(parsed.exprs);

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
  case 'lsp':
    await import('./lsp/server');
    break;
  default:
    printUsage();
    if (command) {
      process.exit(1);
    }
}
