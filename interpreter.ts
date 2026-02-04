import type { Expr, RuntimeValue, Env, FFISignature } from './types';
import { dlopen, FFIType, suffix, ptr, CString } from 'bun:ffi';

// Environment operations
export namespace Environment {
  export const empty = (): Env => ({
    bindings: new Map(),
    parent: null
  });

  export const extend = (env: Env, name: string, value: RuntimeValue): Env => ({
    bindings: new Map(env.bindings).set(name, value),
    parent: env.parent
  });

  export const extendMany = (env: Env, bindings: [string, RuntimeValue][]): Env => {
    const newBindings = new Map(env.bindings);
    for (const [name, value] of bindings) {
      newBindings.set(name, value);
    }
    return { bindings: newBindings, parent: env.parent };
  };

  export const child = (parent: Env): Env => ({
    bindings: new Map(),
    parent
  });

  export const lookup = (env: Env, name: string): RuntimeValue => {
    const value = env.bindings.get(name);
    if (value !== undefined) return value;
    if (env.parent) return lookup(env.parent, name);
    throw new Error(`Undefined variable: ${name}`);
  };

  export const define = (env: Env, name: string, value: RuntimeValue): Env => ({
    bindings: new Map(env.bindings).set(name, value),
    parent: env.parent
  });
}

// Built-in functions
const builtins: [string, (args: RuntimeValue[]) => RuntimeValue][] = [
  ['+', (args) => {
    const nums = args.map(a => {
      if (a.kind !== 'number') throw new Error('+ expects numbers');
      return a.value;
    });
    return { kind: 'number', value: nums.reduce((a, b) => a + b, 0) };
  }],
  ['-', (args) => {
    if (args.length === 0) throw new Error('- expects at least one argument');
    const nums = args.map(a => {
      if (a.kind !== 'number') throw new Error('- expects numbers');
      return a.value;
    });
    const first = nums[0];
    if (first === undefined) throw new Error('- expects at least one argument');
    if (nums.length === 1) return { kind: 'number', value: -first };
    return { kind: 'number', value: nums.slice(1).reduce((a, b) => a - b, first) };
  }],
  ['*', (args) => {
    const nums = args.map(a => {
      if (a.kind !== 'number') throw new Error('* expects numbers');
      return a.value;
    });
    return { kind: 'number', value: nums.reduce((a, b) => a * b, 1) };
  }],
  ['/', (args) => {
    if (args.length < 2) throw new Error('/ expects at least two arguments');
    const nums = args.map(a => {
      if (a.kind !== 'number') throw new Error('/ expects numbers');
      return a.value;
    });
    const first = nums[0];
    if (first === undefined) throw new Error('/ expects at least two arguments');
    return { kind: 'number', value: nums.slice(1).reduce((a, b) => a / b, first) };
  }],
  ['<', (args) => {
    if (args.length !== 2) throw new Error('< expects two arguments');
    const a = args[0], b = args[1];
    if (a?.kind !== 'number' || b?.kind !== 'number') throw new Error('< expects numbers');
    return { kind: 'boolean', value: a.value < b.value };
  }],
  ['>', (args) => {
    if (args.length !== 2) throw new Error('> expects two arguments');
    const a = args[0], b = args[1];
    if (a?.kind !== 'number' || b?.kind !== 'number') throw new Error('> expects numbers');
    return { kind: 'boolean', value: a.value > b.value };
  }],
  ['=', (args) => {
    if (args.length !== 2) throw new Error('= expects two arguments');
    const a = args[0], b = args[1];
    if (a?.kind !== 'number' || b?.kind !== 'number') throw new Error('= expects numbers');
    return { kind: 'boolean', value: a.value === b.value };
  }],
  ['<=', (args) => {
    if (args.length !== 2) throw new Error('<= expects two arguments');
    const a = args[0], b = args[1];
    if (a?.kind !== 'number' || b?.kind !== 'number') throw new Error('<= expects numbers');
    return { kind: 'boolean', value: a.value <= b.value };
  }],
  ['>=', (args) => {
    if (args.length !== 2) throw new Error('>= expects two arguments');
    const a = args[0], b = args[1];
    if (a?.kind !== 'number' || b?.kind !== 'number') throw new Error('>= expects numbers');
    return { kind: 'boolean', value: a.value >= b.value };
  }],
  ['list', (args) => ({ kind: 'list', elements: args })],
  ['ffi-open', (args) => {
    if (args.length !== 2) throw new Error('ffi-open expects 2 arguments: path and signatures');
    const pathArg = args[0];
    const sigsArg = args[1];

    if (pathArg?.kind !== 'string') throw new Error('ffi-open: path must be a string');
    if (sigsArg?.kind !== 'list') throw new Error('ffi-open: signatures must be a list');

    const libPath = pathArg.value;
    const signatures = new Map<string, FFISignature>();
    const ffiSymbols: Record<string, { args: FFIType[], returns: FFIType }> = {};

    // Parse signatures list: (list (list "name" (list "arg-types"...) "return-type")...)
    for (const sig of sigsArg.elements) {
      if (sig.kind !== 'list' || sig.elements.length !== 3) {
        throw new Error('ffi-open: each signature must be (list "name" (list arg-types...) "return-type")');
      }

      const nameVal = sig.elements[0], argsVal = sig.elements[1], retVal = sig.elements[2];
      if (nameVal?.kind !== 'string') throw new Error('ffi-open: function name must be a string');
      if (argsVal?.kind !== 'list') throw new Error('ffi-open: arg types must be a list');
      if (retVal?.kind !== 'string') throw new Error('ffi-open: return type must be a string');

      const fnName = nameVal.value;
      const argTypes: string[] = [];
      const ffiArgTypes: FFIType[] = [];

      for (const argType of argsVal.elements) {
        if (argType.kind !== 'string') throw new Error('ffi-open: arg type must be a string');
        argTypes.push(argType.value);
        ffiArgTypes.push(stringToFFIType(argType.value));
      }

      signatures.set(fnName, { args: argTypes, returns: retVal.value });
      ffiSymbols[fnName] = {
        args: ffiArgTypes,
        returns: stringToFFIType(retVal.value)
      };
    }

    // Open the library
    // If the path is absolute or contains an extension, use it as-is
    const fullPath = libPath.startsWith('/') || libPath.includes('.')
      ? libPath
      : `lib${libPath}.${suffix}`;
    const lib = dlopen(fullPath, ffiSymbols);

    return { kind: 'ffi-library', path: libPath, lib, signatures };
  }],
  ['ffi-call', (args) => {
    if (args.length < 2) throw new Error('ffi-call expects at least 2 arguments: lib and fn-name');
    const libArg = args[0];
    const fnNameArg = args[1];
    const fnArgs = args.slice(2);

    if (libArg?.kind !== 'ffi-library') throw new Error('ffi-call: first argument must be an ffi-library');
    if (fnNameArg?.kind !== 'string') throw new Error('ffi-call: function name must be a string');

    const fnName = fnNameArg.value;
    const sig = libArg.signatures.get(fnName);
    if (!sig) throw new Error(`ffi-call: function '${fnName}' not found in library`);

    if (fnArgs.length !== sig.args.length) {
      throw new Error(`ffi-call: ${fnName} expects ${sig.args.length} arguments, got ${fnArgs.length}`);
    }

    // Convert Lisp values to FFI arguments
    const convertedArgs = fnArgs.map((arg, i) => {
      const argType = sig.args[i];
      if (argType === undefined) throw new Error('ffi-call: missing argument type');
      return lispToFFI(arg, argType);
    });

    // Call the function
    const fn = libArg.lib.symbols[fnName];
    const result = fn(...convertedArgs);

    // Convert result back to Lisp
    return ffiToLisp(result, sig.returns);
  }],
  ['ffi-close', (args) => {
    if (args.length !== 1) throw new Error('ffi-close expects 1 argument');
    const libArg = args[0];
    if (libArg?.kind !== 'ffi-library') throw new Error('ffi-close: argument must be an ffi-library');
    libArg.lib.close();
    return { kind: 'nil' };
  }],
  ['env-bindings', (args) => {
    if (args.length !== 1) throw new Error('env-bindings expects 1 argument');
    const envArg = args[0];
    if (envArg?.kind !== 'environment') throw new Error('env-bindings: argument must be an environment');
    const pairs: RuntimeValue[] = [];
    for (const [name, value] of envArg.env.bindings) {
      pairs.push({ kind: 'list', elements: [{ kind: 'string', value: name }, value] });
    }
    return { kind: 'list', elements: pairs };
  }],
  ['env-get', (args) => {
    if (args.length !== 2) throw new Error('env-get expects 2 arguments');
    const envArg = args[0];
    const nameArg = args[1];
    if (envArg?.kind !== 'environment') throw new Error('env-get: first argument must be an environment');
    if (nameArg?.kind !== 'string') throw new Error('env-get: second argument must be a string');
    try {
      return Environment.lookup(envArg.env, nameArg.value);
    } catch {
      return { kind: 'nil' };
    }
  }],
  ['env-parent', (args) => {
    if (args.length !== 1) throw new Error('env-parent expects 1 argument');
    const envArg = args[0];
    if (envArg?.kind !== 'environment') throw new Error('env-parent: argument must be an environment');
    if (envArg.env.parent === null) return { kind: 'nil' };
    return { kind: 'environment', env: envArg.env.parent };
  }],
];

// Map type string to FFIType
const stringToFFIType = (typeStr: string): FFIType => {
  const typeMap: Record<string, FFIType> = {
    'i8': FFIType.i8,
    'i16': FFIType.i16,
    'i32': FFIType.i32,
    'i64': FFIType.i64,
    'u8': FFIType.u8,
    'u16': FFIType.u16,
    'u32': FFIType.u32,
    'u64': FFIType.u64,
    'f32': FFIType.f32,
    'f64': FFIType.f64,
    'ptr': FFIType.ptr,
    'cstring': FFIType.cstring,
    'bool': FFIType.bool,
    'void': FFIType.void,
  };
  const ffiType = typeMap[typeStr];
  if (ffiType === undefined) throw new Error(`Unknown FFI type: ${typeStr}`);
  return ffiType;
};

// Convert Lisp value to FFI argument
const lispToFFI = (value: RuntimeValue, typeStr: string): any => {
  switch (typeStr) {
    case 'i8': case 'i16': case 'i32': case 'i64':
    case 'u8': case 'u16': case 'u32': case 'u64':
    case 'f32': case 'f64':
      if (value.kind !== 'number') throw new Error(`Expected number for ${typeStr}`);
      return value.value;
    case 'bool':
      if (value.kind !== 'boolean') throw new Error('Expected boolean for bool');
      return value.value;
    case 'cstring':
    case 'ptr':
      if (value.kind === 'string') {
        return Buffer.from(value.value + '\0');
      }
      if (value.kind === 'ffi-pointer') return value.ptr;
      if (value.kind === 'nil') return null;
      throw new Error(`Cannot convert ${value.kind} to ${typeStr}`);
    default:
      throw new Error(`Unknown FFI type: ${typeStr}`);
  }
};

// Convert FFI result to Lisp value
const ffiToLisp = (result: any, typeStr: string): RuntimeValue => {
  switch (typeStr) {
    case 'i8': case 'i16': case 'i32': case 'i64':
    case 'u8': case 'u16': case 'u32': case 'u64':
    case 'f32': case 'f64':
      return { kind: 'number', value: Number(result) };
    case 'bool':
      return { kind: 'boolean', value: Boolean(result) };
    case 'cstring':
      if (result === null) return { kind: 'nil' };
      return { kind: 'string', value: new CString(result).toString() };
    case 'ptr':
      if (result === null || result === 0) return { kind: 'nil' };
      return { kind: 'ffi-pointer', ptr: Number(result) };
    case 'void':
      return { kind: 'nil' };
    default:
      throw new Error(`Unknown FFI type: ${typeStr}`);
  }
};

export const createDefaultEnv = (): Env => {
  let env = Environment.empty();
  for (const [name, fn] of builtins) {
    env = Environment.define(env, name, { kind: 'builtin', name, fn });
  }
  return env;
};

// Interpreter
export type T = { env: Env }
export type EvalResult = { value: RuntimeValue, interp: T }

export const empty = (): T => ({ env: createDefaultEnv() });

// Convert list Expr to array of Expr
const listToArray = (expr: Expr): Expr[] => {
  const result: Expr[] = [];
  let current = expr;
  while (current.kind === 'list') {
    result.push(current.head);
    current = current.tail;
  }
  return result;
};

// Check if value is truthy (only nil and false are falsy)
const isTruthy = (value: RuntimeValue): boolean => {
  if (value.kind === 'nil') return false;
  if (value.kind === 'boolean' && !value.value) return false;
  return true;
};

// Apply a function to arguments
const apply = (fn: RuntimeValue, args: RuntimeValue[]): RuntimeValue => {
  if (fn.kind === 'builtin') {
    return fn.fn(args);
  }
  if (fn.kind === 'function') {
    if (args.length !== fn.params.length) {
      throw new Error(`Expected ${fn.params.length} arguments, got ${args.length}`);
    }
    // Create new environment with closure as parent
    let fnEnv = Environment.child(fn.closure);
    for (let i = 0; i < fn.params.length; i++) {
      const param = fn.params[i];
      const arg = args[i];
      if (param === undefined || arg === undefined) break;
      fnEnv = Environment.define(fnEnv, param, arg);
    }
    // Evaluate body in new environment
    const result = evalExpr({ env: fnEnv }, fn.body);
    return result.value;
  }
  throw new Error(`Cannot apply non-function: ${fn.kind}`);
};

// Evaluate a single expression
export const evalExpr = (interp: T, expr: Expr): EvalResult => {
  // Nil
  if (expr.kind === 'nil') {
    return { value: { kind: 'nil' }, interp };
  }

  // Atom
  if (expr.kind === 'atom') {
    if (expr.value.kind === 'number') {
      return { value: { kind: 'number', value: expr.value.value }, interp };
    }
    if (expr.value.kind === 'string') {
      return { value: { kind: 'string', value: expr.value.value }, interp };
    }
    // Symbol lookup
    const value = Environment.lookup(interp.env, expr.value.value);
    return { value, interp };
  }

  // List - check for special forms
  const elements = listToArray(expr);
  const first = elements[0];
  if (first === undefined) {
    return { value: { kind: 'nil' }, interp };
  }

  // Special forms
  if (first.kind === 'atom' && first.value.kind === 'symbol') {
    const sym = first.value.value;

    // define
    if (sym === 'define') {
      if (elements.length !== 3) throw new Error('define expects 2 arguments');
      const nameExpr = elements[1];
      const valueExpr = elements[2];
      if (nameExpr?.kind !== 'atom' || nameExpr.value.kind !== 'symbol') {
        throw new Error('define expects a symbol as first argument');
      }
      if (valueExpr === undefined) throw new Error('define expects a value');
      const name = nameExpr.value.value;
      // For recursive functions: mutate the environment in-place so the closure can see itself
      interp.env.bindings.set(name, { kind: 'nil' }); // placeholder
      const { value } = evalExpr(interp, valueExpr);
      interp.env.bindings.set(name, value); // update with actual value
      return { value: { kind: 'nil' }, interp };
    }

    // lambda
    if (sym === 'lambda') {
      if (elements.length !== 3) throw new Error('lambda expects 2 arguments');
      const paramsExpr = elements[1];
      const body = elements[2];
      if (paramsExpr === undefined || body === undefined) throw new Error('lambda expects 2 arguments');

      // Parse parameters
      const paramExprs = listToArray(paramsExpr);
      const params: string[] = [];
      for (const p of paramExprs) {
        if (p.kind !== 'atom' || p.value.kind !== 'symbol') {
          throw new Error('lambda parameters must be symbols');
        }
        params.push(p.value.value);
      }

      return {
        value: { kind: 'function', params, body, closure: interp.env },
        interp
      };
    }

    // if
    if (sym === 'if') {
      if (elements.length < 3 || elements.length > 4) {
        throw new Error('if expects 2 or 3 arguments');
      }
      const condExpr = elements[1];
      const thenExpr = elements[2];
      const elseExpr = elements[3];
      if (condExpr === undefined || thenExpr === undefined) throw new Error('if expects 2 or 3 arguments');
      const { value: cond } = evalExpr(interp, condExpr);
      if (isTruthy(cond)) {
        return evalExpr(interp, thenExpr);
      } else if (elseExpr !== undefined) {
        return evalExpr(interp, elseExpr);
      }
      return { value: { kind: 'nil' }, interp };
    }

    // let
    if (sym === 'let') {
      if (elements.length !== 3) throw new Error('let expects 2 arguments');
      const bindingsExpr = elements[1];
      const body = elements[2];
      if (bindingsExpr === undefined || body === undefined) throw new Error('let expects 2 arguments');

      // Create child environment
      let letEnv = Environment.child(interp.env);

      // Parse bindings ((name value) ...)
      const bindingsList = listToArray(bindingsExpr);
      for (const binding of bindingsList) {
        const pair = listToArray(binding);
        if (pair.length !== 2) throw new Error('let binding must be (name value)');
        const nameExpr = pair[0];
        const valExpr = pair[1];
        if (nameExpr?.kind !== 'atom' || nameExpr.value.kind !== 'symbol') {
          throw new Error('let binding name must be a symbol');
        }
        if (valExpr === undefined) throw new Error('let binding must have a value');
        const name = nameExpr.value.value;
        // Evaluate value in the let environment (for sequential let)
        const { value } = evalExpr({ env: letEnv }, valExpr);
        letEnv = Environment.define(letEnv, name, value);
      }

      // Evaluate body in let environment
      const result = evalExpr({ env: letEnv }, body);
      // Return with original environment
      return { value: result.value, interp };
    }

    // cond
    if (sym === 'cond') {
      for (let i = 1; i < elements.length; i++) {
        const clauseExpr = elements[i];
        if (clauseExpr === undefined) continue;
        const clause = listToArray(clauseExpr);
        if (clause.length !== 2) throw new Error('cond clause must be (test expr)');

        const testExpr = clause[0];
        const resultExpr = clause[1];
        if (testExpr === undefined || resultExpr === undefined) throw new Error('cond clause must be (test expr)');
        // Check for else clause
        if (testExpr.kind === 'atom' && testExpr.value.kind === 'symbol' && testExpr.value.value === 'else') {
          return evalExpr(interp, resultExpr);
        }

        const { value: test } = evalExpr(interp, testExpr);
        if (isTruthy(test)) {
          return evalExpr(interp, resultExpr);
        }
      }
      return { value: { kind: 'nil' }, interp };
    }

    // current-env
    if (sym === 'current-env') {
      if (elements.length !== 1) throw new Error('current-env takes no arguments');
      return {
        value: { kind: 'environment', env: interp.env },
        interp
      };
    }
  }

  // Function application
  const { value: fn } = evalExpr(interp, first);
  const args: RuntimeValue[] = [];
  for (let i = 1; i < elements.length; i++) {
    const argExpr = elements[i];
    if (argExpr === undefined) continue;
    const { value } = evalExpr(interp, argExpr);
    args.push(value);
  }

  const result = apply(fn, args);
  return { value: result, interp };
};

// Evaluate multiple expressions, threading the environment through
export const evalMany = (interp: T, exprs: Expr[]): EvalResult => {
  let result: RuntimeValue = { kind: 'nil' };
  for (const expr of exprs) {
    const evalResult = evalExpr(interp, expr);
    result = evalResult.value;
    interp = evalResult.interp;
  }
  return { value: result, interp };
};

// Format a runtime value for display
export const formatValue = (value: RuntimeValue): string => {
  switch (value.kind) {
    case 'number': return String(value.value);
    case 'boolean': return value.value ? '#t' : '#f';
    case 'nil': return 'nil';
    case 'string': return `"${value.value}"`;
    case 'list': return `(${value.elements.map(formatValue).join(' ')})`;
    case 'function': return `#<function (${value.params.join(' ')})>`;
    case 'builtin': return `#<builtin ${value.name}>`;
    case 'ffi-library': return `#<ffi-library ${value.path}>`;
    case 'ffi-pointer': return `#<ffi-pointer 0x${value.ptr.toString(16)}>`;
    case 'environment': {
      const names = Array.from(value.env.bindings.keys());
      const hasParent = value.env.parent !== null;
      return `#<environment [${names.join(', ')}]${hasParent ? ' +parent' : ''}>`;
    }
  }
};
