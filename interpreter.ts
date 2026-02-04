import type { Expr, RuntimeValue, Env, FFISignature, TailCall, EvalOutcome, StructField, StructDef } from './types';
import { dlopen, FFIType, suffix, ptr, CString, toArrayBuffer, read } from 'bun:ffi';

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
  ['negate', (args) => {
    if (args.length !== 1) throw new Error('negate expects one argument');
    const a = args[0];
    if (a?.kind !== 'number') throw new Error('negate expects a number');
    return { kind: 'number', value: -a.value };
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
  ['show', (args) => {
    if (args.length !== 1) throw new Error('show expects exactly 1 argument');
    const value = args[0];
    if (value === undefined) throw new Error('show expects exactly 1 argument');
    return { kind: 'string', value: formatValue(value) };
  }],
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
        // Skip "void" in argument list - it means "no arguments" (C convention)
        if (argType.value === 'void') continue;
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
  ['ffi-struct', (args) => {
    if (args.length !== 2) throw new Error('ffi-struct expects 2 arguments: name and fields');
    const nameArg = args[0];
    const fieldsArg = args[1];

    if (nameArg?.kind !== 'string') throw new Error('ffi-struct: name must be a string');
    if (fieldsArg?.kind !== 'list') throw new Error('ffi-struct: fields must be a list');

    const structName = nameArg.value;
    const fields: StructField[] = [];
    let offset = 0;

    for (const fieldDef of fieldsArg.elements) {
      if (fieldDef.kind !== 'list' || fieldDef.elements.length !== 2) {
        throw new Error('ffi-struct: each field must be (list "name" "type")');
      }
      const fieldName = fieldDef.elements[0];
      const fieldType = fieldDef.elements[1];
      if (fieldName?.kind !== 'string') throw new Error('ffi-struct: field name must be a string');
      if (fieldType?.kind !== 'string') throw new Error('ffi-struct: field type must be a string');

      const size = getTypeSize(fieldType.value);
      const alignment = getTypeAlignment(fieldType.value);

      // Align offset
      offset = Math.ceil(offset / alignment) * alignment;

      fields.push({
        name: fieldName.value,
        type: fieldType.value,
        offset,
        size
      });

      offset += size;
    }

    // Final size must be aligned to largest alignment
    const maxAlignment = Math.max(...fields.map(f => getTypeAlignment(f.type)), 1);
    const totalSize = Math.ceil(offset / maxAlignment) * maxAlignment;

    const def: StructDef = { name: structName, fields, size: totalSize };
    // Register the struct for FFI type conversion
    structRegistry.set(structName, def);
    return { kind: 'ffi-struct-def', def };
  }],
  ['struct-new', (args) => {
    if (args.length < 1) throw new Error('struct-new expects at least 1 argument: struct-def');
    const defArg = args[0];
    if (defArg?.kind !== 'ffi-struct-def') throw new Error('struct-new: first argument must be an ffi-struct-def');

    const def = defArg.def;
    const values = args.slice(1);

    if (values.length !== def.fields.length) {
      throw new Error(`struct-new: ${def.name} expects ${def.fields.length} values, got ${values.length}`);
    }

    const buffer = new Uint8Array(def.size);
    const view = new DataView(buffer.buffer);

    for (let i = 0; i < def.fields.length; i++) {
      const field = def.fields[i]!;
      const value = values[i]!;
      writeFieldValue(view, field, value);
    }

    return { kind: 'ffi-struct', def, buffer };
  }],
  ['struct-get', (args) => {
    if (args.length !== 2) throw new Error('struct-get expects 2 arguments: struct and field-name');
    const structArg = args[0];
    const fieldNameArg = args[1];

    if (structArg?.kind !== 'ffi-struct') throw new Error('struct-get: first argument must be an ffi-struct');
    if (fieldNameArg?.kind !== 'string') throw new Error('struct-get: second argument must be a string');

    const field = structArg.def.fields.find(f => f.name === fieldNameArg.value);
    if (!field) throw new Error(`struct-get: field '${fieldNameArg.value}' not found in struct ${structArg.def.name}`);

    const view = new DataView(structArg.buffer.buffer);
    return readFieldValue(view, field);
  }],
  ['struct-set', (args) => {
    if (args.length !== 3) throw new Error('struct-set expects 3 arguments: struct, field-name, value');
    const structArg = args[0];
    const fieldNameArg = args[1];
    const value = args[2];

    if (structArg?.kind !== 'ffi-struct') throw new Error('struct-set: first argument must be an ffi-struct');
    if (fieldNameArg?.kind !== 'string') throw new Error('struct-set: second argument must be a string');
    if (value === undefined) throw new Error('struct-set: value is required');

    const field = structArg.def.fields.find(f => f.name === fieldNameArg.value);
    if (!field) throw new Error(`struct-set: field '${fieldNameArg.value}' not found in struct ${structArg.def.name}`);

    const view = new DataView(structArg.buffer.buffer);
    writeFieldValue(view, field, value);

    return { kind: 'nil' };
  }],
];

// Get size of a type in bytes
const getTypeSize = (typeStr: string): number => {
  const sizeMap: Record<string, number> = {
    'i8': 1, 'u8': 1,
    'i16': 2, 'u16': 2,
    'i32': 4, 'u32': 4, 'f32': 4,
    'i64': 8, 'u64': 8, 'f64': 8, 'ptr': 8,
    'bool': 1,
  };
  const size = sizeMap[typeStr];
  if (size === undefined) throw new Error(`Unknown type size: ${typeStr}`);
  return size;
};

// Get alignment of a type
const getTypeAlignment = (typeStr: string): number => {
  // Alignment is typically the same as size for primitive types
  return getTypeSize(typeStr);
};

// Write a value to a struct field
const writeFieldValue = (view: DataView, field: StructField, value: RuntimeValue): void => {
  const { offset, type } = field;
  switch (type) {
    case 'u8':
      if (value.kind !== 'number') throw new Error(`Expected number for ${type}`);
      view.setUint8(offset, value.value);
      break;
    case 'i8':
      if (value.kind !== 'number') throw new Error(`Expected number for ${type}`);
      view.setInt8(offset, value.value);
      break;
    case 'u16':
      if (value.kind !== 'number') throw new Error(`Expected number for ${type}`);
      view.setUint16(offset, value.value, true);
      break;
    case 'i16':
      if (value.kind !== 'number') throw new Error(`Expected number for ${type}`);
      view.setInt16(offset, value.value, true);
      break;
    case 'u32':
      if (value.kind !== 'number') throw new Error(`Expected number for ${type}`);
      view.setUint32(offset, value.value, true);
      break;
    case 'i32':
      if (value.kind !== 'number') throw new Error(`Expected number for ${type}`);
      view.setInt32(offset, value.value, true);
      break;
    case 'f32':
      if (value.kind !== 'number') throw new Error(`Expected number for ${type}`);
      view.setFloat32(offset, value.value, true);
      break;
    case 'u64':
      if (value.kind !== 'number') throw new Error(`Expected number for ${type}`);
      view.setBigUint64(offset, BigInt(Math.floor(value.value)), true);
      break;
    case 'i64':
      if (value.kind !== 'number') throw new Error(`Expected number for ${type}`);
      view.setBigInt64(offset, BigInt(Math.floor(value.value)), true);
      break;
    case 'f64':
      if (value.kind !== 'number') throw new Error(`Expected number for ${type}`);
      view.setFloat64(offset, value.value, true);
      break;
    case 'ptr':
      if (value.kind === 'ffi-pointer') {
        view.setBigUint64(offset, BigInt(value.ptr), true);
      } else if (value.kind === 'nil') {
        view.setBigUint64(offset, 0n, true);
      } else if (value.kind === 'number') {
        view.setBigUint64(offset, BigInt(Math.floor(value.value)), true);
      } else {
        throw new Error(`Cannot convert ${value.kind} to ptr`);
      }
      break;
    case 'bool':
      if (value.kind !== 'boolean') throw new Error('Expected boolean for bool');
      view.setUint8(offset, value.value ? 1 : 0);
      break;
    default:
      throw new Error(`Unknown field type: ${type}`);
  }
};

// Read a value from a struct field
const readFieldValue = (view: DataView, field: StructField): RuntimeValue => {
  const { offset, type } = field;
  switch (type) {
    case 'u8':
      return { kind: 'number', value: view.getUint8(offset) };
    case 'i8':
      return { kind: 'number', value: view.getInt8(offset) };
    case 'u16':
      return { kind: 'number', value: view.getUint16(offset, true) };
    case 'i16':
      return { kind: 'number', value: view.getInt16(offset, true) };
    case 'u32':
      return { kind: 'number', value: view.getUint32(offset, true) };
    case 'i32':
      return { kind: 'number', value: view.getInt32(offset, true) };
    case 'f32':
      return { kind: 'number', value: view.getFloat32(offset, true) };
    case 'u64':
      return { kind: 'number', value: Number(view.getBigUint64(offset, true)) };
    case 'i64':
      return { kind: 'number', value: Number(view.getBigInt64(offset, true)) };
    case 'f64':
      return { kind: 'number', value: view.getFloat64(offset, true) };
    case 'ptr': {
      const ptrValue = Number(view.getBigUint64(offset, true));
      if (ptrValue === 0) return { kind: 'nil' };
      return { kind: 'ffi-pointer', ptr: ptrValue };
    }
    case 'bool':
      return { kind: 'boolean', value: view.getUint8(offset) !== 0 };
    default:
      throw new Error(`Unknown field type: ${type}`);
  }
};

// Registry for struct definitions (for FFI conversion)
const structRegistry = new Map<string, StructDef>();

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
  if (ffiType !== undefined) return ffiType;

  // Check if it's a struct type - structs are passed as pointers
  if (structRegistry.has(typeStr)) {
    return FFIType.ptr;
  }

  throw new Error(`Unknown FFI type: ${typeStr}`);
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
      if (value.kind === 'ffi-struct') {
        // Structs are passed as pointers to their buffer
        return ptr(value.buffer);
      }
      if (value.kind === 'nil') return null;
      throw new Error(`Cannot convert ${value.kind} to ${typeStr}`);
    default:
      // Check if it's a struct type
      if (structRegistry.has(typeStr)) {
        if (value.kind === 'ffi-struct') {
          return ptr(value.buffer);
        }
        if (value.kind === 'nil') return null;
        throw new Error(`Expected struct ${typeStr}, got ${value.kind}`);
      }
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
      // Check if it's a struct type
      const structDef = structRegistry.get(typeStr);
      if (structDef) {
        // Result is a pointer to struct data - copy it into a new buffer
        if (result === null || result === 0) return { kind: 'nil' };
        const ptrNum = typeof result === 'bigint' ? Number(result) : result;
        const buffer = new Uint8Array(structDef.size);
        const sourceBuffer = toArrayBuffer(ptrNum, 0, structDef.size);
        buffer.set(new Uint8Array(sourceBuffer));
        return { kind: 'ffi-struct', def: structDef, buffer };
      }
      throw new Error(`Unknown FFI type: ${typeStr}`);
  }
};

export const createDefaultEnv = (): Env => {
  let env = Environment.empty();
  // Add nil as a special value
  env = Environment.define(env, 'nil', { kind: 'nil' });
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

// Set of known constructor names (populated by data definitions)
const knownConstructors = new Set<string>();

// Pattern matching helper
// Returns null if pattern doesn't match, otherwise returns bindings
const matchPattern = (pattern: Expr, value: RuntimeValue, env: Env): Map<string, RuntimeValue> | null => {
  // Wildcard pattern: _
  if (pattern.kind === 'atom' && pattern.value.kind === 'symbol' && pattern.value.value === '_') {
    return new Map();
  }

  // Variable pattern or nullary constructor
  if (pattern.kind === 'atom' && pattern.value.kind === 'symbol') {
    const name = pattern.value.value;

    // Check if it's a known nullary constructor by looking up in environment
    try {
      const binding = Environment.lookup(env, name);
      if (binding && binding.kind === 'adt-value' && binding.args.length === 0) {
        // It's a nullary constructor pattern - match against same constructor
        if (value.kind === 'adt-value' && value.constructor === name && value.args.length === 0) {
          return new Map();
        }
        return null;
      }
    } catch (e) {
      // Only treat "Undefined variable" errors as "not found" - rethrow others
      if (!(e instanceof Error && e.message.startsWith('Undefined variable:'))) {
        throw e;
      }
      // Variable not found in environment - treat as binding variable
    }

    // Otherwise it's a variable that binds the value
    return new Map([[name, value]]);
  }

  // Number literal pattern
  if (pattern.kind === 'atom' && pattern.value.kind === 'number') {
    if (value.kind === 'number' && value.value === pattern.value.value) {
      return new Map();
    }
    return null;
  }

  // String literal pattern
  if (pattern.kind === 'atom' && pattern.value.kind === 'string') {
    if (value.kind === 'string' && value.value === pattern.value.value) {
      return new Map();
    }
    return null;
  }

  // Nil pattern
  if (pattern.kind === 'nil') {
    if (value.kind === 'nil') {
      return new Map();
    }
    // Also match Nil constructor
    if (value.kind === 'adt-value' && value.constructor === 'Nil' && value.args.length === 0) {
      return new Map();
    }
    return null;
  }

  // Constructor pattern: (CtorName args...)
  if (pattern.kind === 'list') {
    const patternElements = listToArray(pattern);
    if (patternElements.length === 0) {
      // Empty list matches nil
      return value.kind === 'nil' ? new Map() : null;
    }

    const first = patternElements[0]!;
    if (first.kind !== 'atom' || first.value.kind !== 'symbol') {
      return null;
    }

    const ctorName = first.value.value;
    const argPatterns = patternElements.slice(1);

    // Match against ADT value
    if (value.kind === 'adt-value') {
      if (value.constructor !== ctorName) return null;
      if (value.args.length !== argPatterns.length) return null;

      const bindings = new Map<string, RuntimeValue>();
      for (let i = 0; i < argPatterns.length; i++) {
        const subMatch = matchPattern(argPatterns[i]!, value.args[i]!, env);
        if (subMatch === null) return null;
        for (const [k, v] of subMatch) {
          bindings.set(k, v);
        }
      }
      return bindings;
    }

    // Match list against Cons pattern
    if (ctorName === 'Cons' && argPatterns.length === 2 && value.kind === 'list') {
      if (value.elements.length === 0) return null;
      const head = value.elements[0]!;
      const tail: RuntimeValue = { kind: 'list', elements: value.elements.slice(1) };

      const headMatch = matchPattern(argPatterns[0]!, head, env);
      if (headMatch === null) return null;

      const tailMatch = matchPattern(argPatterns[1]!, tail, env);
      if (tailMatch === null) return null;

      const bindings = new Map<string, RuntimeValue>();
      for (const [k, v] of headMatch) bindings.set(k, v);
      for (const [k, v] of tailMatch) bindings.set(k, v);
      return bindings;
    }

    // Match empty list against Nil
    if (ctorName === 'Nil' && argPatterns.length === 0 && value.kind === 'list') {
      return value.elements.length === 0 ? new Map() : null;
    }

    return null;
  }

  return null;
};

// Evaluate a sequence of expressions, returning the last value
const evalSequence = (interp: T, exprs: Expr[], isTail: boolean): EvalOutcome => {
  if (exprs.length === 0) {
    return { kind: 'nil' };
  }
  for (let i = 0; i < exprs.length - 1; i++) {
    const expr = exprs[i];
    if (expr !== undefined) trampoline(evalExprTail(interp, expr, false));
  }
  const lastExpr = exprs[exprs.length - 1];
  return lastExpr ? evalExprTail(interp, lastExpr, isTail) : { kind: 'nil' };
};

// Check if an outcome is a tail call thunk
const isTailCall = (outcome: EvalOutcome): outcome is TailCall => {
  return typeof outcome === 'object' && 'kind' in outcome && outcome.kind === 'tail-call';
};

// Execute a tail call thunk, returning the next outcome
const applyTailCall = (tc: TailCall): EvalOutcome => {
  return applyInternal(tc.fn, tc.args);
};

// Trampoline: execute thunks until we get a real value (tmj, @leandronsp)
const trampoline = (outcome: EvalOutcome): RuntimeValue => {
  while (isTailCall(outcome)) {
    outcome = applyTailCall(outcome);
  }
  return outcome;
};

// Internal apply that may return a TailCall thunk (for user-defined functions)
const applyInternal = (fn: RuntimeValue, args: RuntimeValue[]): EvalOutcome => {
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
    // Evaluate body in new environment with tail position enabled
    return evalSequence({ env: fnEnv }, fn.body, true);
  }
  // ADT constructor application
  if (fn.kind === 'adt-constructor') {
    if (args.length !== fn.arity) {
      throw new Error(`Constructor ${fn.name} expects ${fn.arity} arguments, got ${args.length}`);
    }
    return { kind: 'adt-value', constructor: fn.name, args };
  }
  throw new Error(`Cannot apply non-function: ${fn.kind}`);
};

// Apply a function to arguments (public API - trampolines the result)
const apply = (fn: RuntimeValue, args: RuntimeValue[]): RuntimeValue => {
  return trampoline(applyInternal(fn, args));
};

// Tail-position-aware expression evaluation
// Returns EvalOutcome which may be a TailCall thunk when isTail is true
const evalExprTail = (interp: T, expr: Expr, isTail: boolean): EvalOutcome => {
  // Nil
  if (expr.kind === 'nil') {
    return { kind: 'nil' };
  }

  // Atom
  if (expr.kind === 'atom') {
    if (expr.value.kind === 'number') {
      return { kind: 'number', value: expr.value.value };
    }
    if (expr.value.kind === 'string') {
      return { kind: 'string', value: expr.value.value };
    }
    // Symbol lookup
    return Environment.lookup(interp.env, expr.value.value);
  }

  // List - check for special forms
  const elements = listToArray(expr);
  const first = elements[0];
  if (first === undefined) {
    return { kind: 'nil' };
  }

  // Special forms
  if (first.kind === 'atom' && first.value.kind === 'symbol') {
    const sym = first.value.value;

    // define - never in tail position (returns nil, side effect)
    // Supports: (define name value) or (define (name : Type) value)
    if (sym === 'define') {
      if (elements.length !== 3) throw new Error('define expects 2 arguments');
      const nameExpr = elements[1];
      const valueExpr = elements[2];

      // Check for type-annotated define: (define (name : Type) value)
      let name: string;
      if (nameExpr?.kind === 'list') {
        const nameParts = listToArray(nameExpr);
        // Should be (name : Type)
        if (nameParts.length === 3) {
          const first = nameParts[0];
          const colon = nameParts[1];
          if (first?.kind === 'atom' && first.value.kind === 'symbol' &&
              colon?.kind === 'atom' && colon.value.kind === 'symbol' && colon.value.value === ':') {
            name = first.value.value;
          } else {
            throw new Error('define expects (name : Type) for type annotation');
          }
        } else {
          throw new Error('define expects a symbol or (symbol : Type) as first argument');
        }
      } else if (nameExpr?.kind === 'atom' && nameExpr.value.kind === 'symbol') {
        name = nameExpr.value.value;
      } else {
        throw new Error('define expects a symbol as first argument');
      }

      if (valueExpr === undefined) throw new Error('define expects a value');
      // For recursive functions: mutate the environment in-place so the closure can see itself
      interp.env.bindings.set(name, { kind: 'nil' }); // placeholder
      const value = trampoline(evalExprTail(interp, valueExpr, false));
      interp.env.bindings.set(name, value); // update with actual value
      return { kind: 'nil' };
    }

    // lambda - returns function value, not a tail call
    // Supports: (lambda (params...) body) or (lambda ((x : Type)...) body)
    if (sym === 'lambda') {
      if (elements.length < 3) throw new Error('lambda expects at least 2 arguments');
      const paramsExpr = elements[1];
      if (paramsExpr === undefined) throw new Error('lambda expects parameters');

      // Parse parameters (may be annotated: (x : Type) or plain: x)
      const paramExprs = listToArray(paramsExpr);
      const params: string[] = [];
      for (const p of paramExprs) {
        // Check for annotated parameter: (x : Type)
        if (p.kind === 'list') {
          const parts = listToArray(p);
          if (parts.length === 3) {
            const first = parts[0];
            const colon = parts[1];
            if (first?.kind === 'atom' && first.value.kind === 'symbol' &&
                colon?.kind === 'atom' && colon.value.kind === 'symbol' && colon.value.value === ':') {
              params.push(first.value.value);
              continue;
            }
          }
          throw new Error('lambda parameters must be symbols or (symbol : Type)');
        }
        if (p.kind !== 'atom' || p.value.kind !== 'symbol') {
          throw new Error('lambda parameters must be symbols');
        }
        params.push(p.value.value);
      }

      // Collect body expressions (all expressions after params)
      const bodyExprs = elements.slice(2);
      return { kind: 'function', params, body: bodyExprs, closure: interp.env };
    }

    // if - propagates tail position to branches
    if (sym === 'if') {
      if (elements.length < 3 || elements.length > 4) {
        throw new Error('if expects 2 or 3 arguments');
      }
      const condExpr = elements[1];
      const thenExpr = elements[2];
      const elseExpr = elements[3];
      if (condExpr === undefined || thenExpr === undefined) throw new Error('if expects 2 or 3 arguments');
      const cond = trampoline(evalExprTail(interp, condExpr, false));
      if (isTruthy(cond)) {
        return evalExprTail(interp, thenExpr, isTail);
      } else if (elseExpr !== undefined) {
        return evalExprTail(interp, elseExpr, isTail);
      }
      return { kind: 'nil' };
    }

    // let - propagates tail position to body
    // Supports: (let ((name value)...) body) or (let (((name : Type) value)...) body)
    if (sym === 'let') {
      if (elements.length < 3) throw new Error('let expects at least 2 arguments');
      const bindingsExpr = elements[1];
      if (bindingsExpr === undefined) throw new Error('let expects bindings');

      // Create child environment
      let letEnv = Environment.child(interp.env);

      // Parse bindings ((name value) ...) or (((name : Type) value) ...)
      const bindingsList = listToArray(bindingsExpr);
      for (const binding of bindingsList) {
        const pair = listToArray(binding);
        if (pair.length !== 2) throw new Error('let binding must be (name value)');
        const nameExpr = pair[0];
        const valExpr = pair[1];

        // Check for type-annotated binding: ((name : Type) value)
        let name: string;
        if (nameExpr?.kind === 'list') {
          const nameParts = listToArray(nameExpr);
          if (nameParts.length === 3) {
            const first = nameParts[0];
            const colon = nameParts[1];
            if (first?.kind === 'atom' && first.value.kind === 'symbol' &&
                colon?.kind === 'atom' && colon.value.kind === 'symbol' && colon.value.value === ':') {
              name = first.value.value;
            } else {
              throw new Error('let binding name must be a symbol or (symbol : Type)');
            }
          } else {
            throw new Error('let binding name must be a symbol or (symbol : Type)');
          }
        } else if (nameExpr?.kind === 'atom' && nameExpr.value.kind === 'symbol') {
          name = nameExpr.value.value;
        } else {
          throw new Error('let binding name must be a symbol');
        }

        if (valExpr === undefined) throw new Error('let binding must have a value');
        // Evaluate value in the let environment (for sequential let)
        const value = trampoline(evalExprTail({ env: letEnv }, valExpr, false));
        letEnv = Environment.define(letEnv, name, value);
      }

      // Collect body expressions (all expressions after bindings)
      const bodyExprs = elements.slice(2);
      // Evaluate body in let environment - propagate tail position
      return evalSequence({ env: letEnv }, bodyExprs, isTail);
    }

    // cond - propagates tail position to result expressions
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
          return evalExprTail(interp, resultExpr, isTail);
        }

        const test = trampoline(evalExprTail(interp, testExpr, false));
        if (isTruthy(test)) {
          return evalExprTail(interp, resultExpr, isTail);
        }
      }
      return { kind: 'nil' };
    }

    // begin - evaluate sequence of expressions, return last value
    if (sym === 'begin') {
      const bodyExprs = elements.slice(1);
      return evalSequence(interp, bodyExprs, isTail);
    }

    // current-env
    if (sym === 'current-env') {
      if (elements.length !== 1) throw new Error('current-env takes no arguments');
      return { kind: 'environment', env: interp.env };
    }

    // data - define algebraic data type
    // (data (TypeName params...) constructor...)
    if (sym === 'data') {
      if (elements.length < 3) throw new Error('data expects type name and at least one constructor');

      const typeExpr = elements[1];
      const ctorExprs = elements.slice(2);

      // Parse type name (ignore params - they're for type checking only)
      let typeName: string;
      if (typeExpr?.kind === 'list') {
        const typeElements = listToArray(typeExpr);
        const first = typeElements[0];
        if (first?.kind !== 'atom' || first.value.kind !== 'symbol') {
          throw new Error('data type name must be a symbol');
        }
        typeName = first.value.value;
      } else if (typeExpr?.kind === 'atom' && typeExpr.value.kind === 'symbol') {
        typeName = typeExpr.value.value;
      } else {
        throw new Error('data type name must be a symbol');
      }

      // Register constructors
      for (const ctorExpr of ctorExprs) {
        if (ctorExpr.kind === 'atom' && ctorExpr.value.kind === 'symbol') {
          // Nullary constructor
          const ctorName = ctorExpr.value.value;
          const ctorValue: RuntimeValue = { kind: 'adt-value', constructor: ctorName, args: [] };
          interp.env.bindings.set(ctorName, ctorValue);
        } else if (ctorExpr.kind === 'list') {
          // Constructor with parameters
          const ctorElements = listToArray(ctorExpr);
          const first = ctorElements[0];
          if (first?.kind !== 'atom' || first.value.kind !== 'symbol') {
            throw new Error('constructor name must be a symbol');
          }
          const ctorName = first.value.value;
          const arity = ctorElements.length - 1;

          if (arity === 0) {
            // Nullary constructor
            const ctorValue: RuntimeValue = { kind: 'adt-value', constructor: ctorName, args: [] };
            interp.env.bindings.set(ctorName, ctorValue);
          } else {
            // Constructor function
            const ctorFn: RuntimeValue = { kind: 'adt-constructor', name: ctorName, arity };
            interp.env.bindings.set(ctorName, ctorFn);
          }
        }
      }

      return { kind: 'nil' };
    }

    // match - pattern matching
    // (match scrutinee (pattern body)...)
    if (sym === 'match') {
      if (elements.length < 3) throw new Error('match expects scrutinee and at least one branch');

      const scrutineeExpr = elements[1];
      const branches = elements.slice(2);

      if (scrutineeExpr === undefined) throw new Error('match expects a scrutinee');
      const scrutinee = trampoline(evalExprTail(interp, scrutineeExpr, false));

      for (const branchExpr of branches) {
        const branchElements = listToArray(branchExpr);
        if (branchElements.length !== 2) {
          throw new Error('match branch must be (pattern body)');
        }

        const patternExpr = branchElements[0]!;
        const bodyExpr = branchElements[1]!;

        const matchResult = matchPattern(patternExpr, scrutinee, interp.env);
        if (matchResult !== null) {
          // Pattern matched - extend environment with bindings and evaluate body
          let matchEnv = Environment.child(interp.env);
          for (const [name, value] of matchResult) {
            matchEnv = Environment.define(matchEnv, name, value);
          }
          return evalExprTail({ env: matchEnv }, bodyExpr, isTail);
        }
      }

      throw new Error('No pattern matched in match expression');
    }

    // class - define typeclass (no-op at runtime)
    if (sym === 'class') {
      return { kind: 'nil' };
    }

    // instance - define typeclass instance (no-op at runtime for now)
    if (sym === 'instance') {
      return { kind: 'nil' };
    }

    // type - define type alias (no-op at runtime)
    if (sym === 'type') {
      return { kind: 'nil' };
    }

    // effect - define effect type (no-op at runtime for now)
    // Syntax: (effect EffectName (op1 : Type1 -> Type2) ...)
    if (sym === 'effect') {
      // Effect declarations are processed at type-check time
      // At runtime, we just validate basic structure
      if (elements.length < 2) throw new Error('effect expects a name');
      return { kind: 'nil' };
    }

    // perform - invoke an effect operation (placeholder)
    // Syntax: (perform op-name arg)
    if (sym === 'perform') {
      if (elements.length < 2) throw new Error('perform expects an operation name');
      throw new Error('perform requires effect handlers (not yet implemented)');
    }

    // handle - install effect handlers (placeholder)
    // Syntax: (handle expr (pattern) -> handler ...)
    if (sym === 'handle') {
      if (elements.length < 2) throw new Error('handle expects an expression');
      throw new Error('handle requires delimited continuations (not yet implemented)');
    }
  }

  // Function application
  const fn = trampoline(evalExprTail(interp, first, false));
  const args: RuntimeValue[] = [];
  for (let i = 1; i < elements.length; i++) {
    const argExpr = elements[i];
    if (argExpr === undefined) continue;
    const value = trampoline(evalExprTail(interp, argExpr, false));
    args.push(value);
  }

  // In tail position, return a thunk for user-defined functions
  if (isTail && fn.kind === 'function') {
    return { kind: 'tail-call', fn, args };
  }

  // Not in tail position or builtin: apply immediately
  return apply(fn, args);
};

// Evaluate a single expression (public API)
export const evalExpr = (interp: T, expr: Expr): EvalResult => {
  const outcome = evalExprTail(interp, expr, false);
  const value = trampoline(outcome);
  return { value, interp };
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
    case 'ffi-struct-def': return `#<ffi-struct-def ${value.def.name} (${value.def.size} bytes)>`;
    case 'ffi-struct': return `#<ffi-struct ${value.def.name}>`;
    case 'environment': {
      const names = Array.from(value.env.bindings.keys());
      const hasParent = value.env.parent !== null;
      return `#<environment [${names.join(', ')}]${hasParent ? ' +parent' : ''}>`;
    }
    case 'adt-value': {
      if (value.args.length === 0) {
        return value.constructor;
      }
      return `(${value.constructor} ${value.args.map(formatValue).join(' ')})`;
    }
    case 'adt-constructor':
      return `#<constructor ${value.name}/${value.arity}>`;
  }
};
