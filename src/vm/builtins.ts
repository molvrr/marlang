// Builtin functions and globals for the Marlang VM

import type { VMValue } from './bytecode';
import { NIL } from './bytecode';
import { formatVMValue } from './vm';

export function createBuiltinGlobals(): Map<string, VMValue> {
  const globals = new Map<string, VMValue>();

  // --- Builtin values ---
  globals.set('nil', NIL);
  globals.set('#t', { kind: 'boolean', value: true });
  globals.set('#f', { kind: 'boolean', value: false });

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
    const nums = args.map(a => {
      if (a.kind !== 'number') throw new Error('+ expects numbers');
      return a.value;
    });
    return { kind: 'number', value: nums.reduce((a, b) => a + b, 0) };
  });

  builtin('-', (args) => {
    if (args.length === 0) throw new Error('- expects at least one argument');
    const nums = args.map(a => {
      if (a.kind !== 'number') throw new Error('- expects numbers');
      return a.value;
    });
    if (nums.length === 1) return { kind: 'number', value: -nums[0] };
    return { kind: 'number', value: nums.slice(1).reduce((a, b) => a - b, nums[0]) };
  });

  builtin('*', (args) => {
    const nums = args.map(a => {
      if (a.kind !== 'number') throw new Error('* expects numbers');
      return a.value;
    });
    return { kind: 'number', value: nums.reduce((a, b) => a * b, 1) };
  });

  builtin('/', (args) => {
    if (args.length < 2) throw new Error('/ expects at least two arguments');
    const nums = args.map(a => {
      if (a.kind !== 'number') throw new Error('/ expects numbers');
      return a.value;
    });
    return { kind: 'number', value: nums.slice(1).reduce((a, b) => a / b, nums[0]) };
  });

  builtin('negate', (args) => {
    if (args.length !== 1 || args[0]?.kind !== 'number') throw new Error('negate expects one number');
    return { kind: 'number', value: -args[0].value };
  });

  // Comparison (for non-binary calls)
  builtin('<', (args) => {
    if (args.length !== 2 || args[0]?.kind !== 'number' || args[1]?.kind !== 'number') throw new Error('< expects two numbers');
    return { kind: 'boolean', value: args[0].value < args[1].value };
  });

  builtin('>', (args) => {
    if (args.length !== 2 || args[0]?.kind !== 'number' || args[1]?.kind !== 'number') throw new Error('> expects two numbers');
    return { kind: 'boolean', value: args[0].value > args[1].value };
  });

  builtin('=', (args) => {
    if (args.length !== 2 || args[0]?.kind !== 'number' || args[1]?.kind !== 'number') throw new Error('= expects two numbers');
    return { kind: 'boolean', value: args[0].value === args[1].value };
  });

  builtin('<=', (args) => {
    if (args.length !== 2 || args[0]?.kind !== 'number' || args[1]?.kind !== 'number') throw new Error('<= expects two numbers');
    return { kind: 'boolean', value: args[0].value <= args[1].value };
  });

  builtin('>=', (args) => {
    if (args.length !== 2 || args[0]?.kind !== 'number' || args[1]?.kind !== 'number') throw new Error('>= expects two numbers');
    return { kind: 'boolean', value: args[0].value >= args[1].value };
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
