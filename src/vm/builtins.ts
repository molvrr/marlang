// Builtin functions and globals for the Marlang VM

import type { VMValue } from './bytecode';
import { NIL, TRUE, FALSE } from './bytecode';
import { formatVMValue } from './vm';

export function createBuiltinGlobals(): Map<string, VMValue> {
  const globals = new Map<string, VMValue>();

  // --- Builtin values ---
  globals.set('nil', NIL);
  globals.set('#t', TRUE);
  globals.set('#f', FALSE);

  // --- Built-in ADT constructors ---
  globals.set('None', { kind: 'adt-value', constructor: 'None', args: [] });
  globals.set('Some', { kind: 'adt-constructor', name: 'Some', arity: 1 });
  globals.set('Nil', { kind: 'adt-value', constructor: 'Nil', args: [] });
  globals.set('Cons', { kind: 'adt-constructor', name: 'Cons', arity: 2 });
  globals.set('MkPair', { kind: 'adt-constructor', name: 'MkPair', arity: 2 });
  globals.set('Ok', { kind: 'adt-constructor', name: 'Ok', arity: 1 });
  globals.set('Err', { kind: 'adt-constructor', name: 'Err', arity: 1 });

  // --- Builtin functions ---

  const builtin = (name: string, fn: (args: VMValue[]) => VMValue) => {
    globals.set(name, { kind: 'vm-builtin', name, fn });
  };

  // Arithmetic (variadic versions for when direct opcodes aren't used)
  builtin('+', (args) => {
    let sum = 0;
    for (const a of args) {
      if (typeof a !== 'number') throw new Error('+ expects numbers');
      sum += a;
    }
    return sum;
  });

  builtin('-', (args) => {
    if (args.length === 0) throw new Error('- expects at least one argument');
    if (typeof args[0] !== 'number') throw new Error('- expects numbers');
    if (args.length === 1) return -args[0];
    let result = args[0];
    for (let i = 1; i < args.length; i++) {
      if (typeof args[i] !== 'number') throw new Error('- expects numbers');
      result -= args[i] as number;
    }
    return result;
  });

  builtin('*', (args) => {
    let product = 1;
    for (const a of args) {
      if (typeof a !== 'number') throw new Error('* expects numbers');
      product *= a;
    }
    return product;
  });

  builtin('/', (args) => {
    if (args.length < 2) throw new Error('/ expects at least two arguments');
    if (typeof args[0] !== 'number') throw new Error('/ expects numbers');
    let result = args[0];
    for (let i = 1; i < args.length; i++) {
      if (typeof args[i] !== 'number') throw new Error('/ expects numbers');
      result /= args[i] as number;
    }
    return result;
  });

  builtin('negate', (args) => {
    if (args.length !== 1 || typeof args[0] !== 'number') throw new Error('negate expects one number');
    return -args[0];
  });

  // Comparison (for non-binary calls)
  builtin('<', (args) => {
    if (args.length !== 2 || typeof args[0] !== 'number' || typeof args[1] !== 'number') throw new Error('< expects two numbers');
    return args[0] < args[1] ? TRUE : FALSE;
  });

  builtin('>', (args) => {
    if (args.length !== 2 || typeof args[0] !== 'number' || typeof args[1] !== 'number') throw new Error('> expects two numbers');
    return args[0] > args[1] ? TRUE : FALSE;
  });

  builtin('=', (args) => {
    if (args.length !== 2 || typeof args[0] !== 'number' || typeof args[1] !== 'number') throw new Error('= expects two numbers');
    return args[0] === args[1] ? TRUE : FALSE;
  });

  builtin('<=', (args) => {
    if (args.length !== 2 || typeof args[0] !== 'number' || typeof args[1] !== 'number') throw new Error('<= expects two numbers');
    return args[0] <= args[1] ? TRUE : FALSE;
  });

  builtin('>=', (args) => {
    if (args.length !== 2 || typeof args[0] !== 'number' || typeof args[1] !== 'number') throw new Error('>= expects two numbers');
    return args[0] >= args[1] ? TRUE : FALSE;
  });

  // List
  builtin('list', (args) => ({ kind: 'list', elements: args }));

  // Show
  builtin('show', (args) => {
    if (args.length !== 1) throw new Error('show expects exactly 1 argument');
    return { kind: 'string', value: formatVMValue(args[0]) };
  });

  return globals;
}
