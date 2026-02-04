// Native IO handlers for Marlang
// These provide default implementations for IO effect operations
import { registerDefaultHandler, formatValue } from './interpreter';
import * as fs from 'fs';

// Format a runtime value for printing (handles Show typeclass at runtime)
const formatForPrint = (value: any): string => {
  if (value?.kind === 'string') {
    // Strings are printed without quotes
    return value.value;
  }
  // All other values use formatValue
  return formatValue(value);
};

// Print without newline
registerDefaultHandler('IO', 'print', (args) => {
  const msg = args[0];
  process.stdout.write(formatForPrint(msg));
  return { kind: 'nil' };
});

// Print with newline
registerDefaultHandler('IO', 'println', (args) => {
  const msg = args[0];
  console.log(formatForPrint(msg));
  return { kind: 'nil' };
});

// Read line - synchronous using Bun's prompt or readline
registerDefaultHandler('IO', 'read-line', () => {
  // Bun supports prompt() for synchronous input
  const line = prompt('') ?? '';
  return { kind: 'string', value: line };
});

// Read file - synchronous
registerDefaultHandler('IO', 'read-file', (args) => {
  const path = args[0];
  if (path?.kind === 'string') {
    const content = fs.readFileSync(path.value, 'utf-8');
    return { kind: 'string', value: content };
  }
  throw new Error('read-file: path must be a string');
});

// Write file - synchronous
registerDefaultHandler('IO', 'write-file', (args) => {
  const path = args[0];
  const content = args[1];
  if (path?.kind === 'string' && content?.kind === 'string') {
    fs.writeFileSync(path.value, content.value);
    return { kind: 'nil' };
  }
  throw new Error('write-file: path and content must be strings');
});
