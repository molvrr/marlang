import * as readline from 'readline';
import * as Lexer from './lexer';
import * as Parser from './parser';
import * as Interpreter from './interpreter';

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
