import * as Lexer from './lexer';
import * as Parser from './parser';
import * as Interpreter from './interpreter';

const filename = process.argv[2];

if (!filename) {
  console.error('Usage: bun run.ts <filename.mar>');
  process.exit(1);
}

const file = Bun.file(filename);
const code = await file.text();

try {
  const tokens = Lexer.lex(Lexer.empty(code));
  const parsed = Parser.parse(Parser.empty(tokens));
  const result = Interpreter.evalMany(Interpreter.empty(), parsed.exprs);

  // Print the final result if it's not nil
  if (result.value.kind !== 'nil') {
    console.log(Interpreter.formatValue(result.value));
  }
} catch (e) {
  console.error(`Error: ${e instanceof Error ? e.message : String(e)}`);
  process.exit(1);
}
