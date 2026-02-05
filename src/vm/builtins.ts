// Builtin functions and globals for the Marlang VM

import type { VMValue, VMFFILibrary, VMFFIStruct } from './bytecode';
import { NIL, TRUE, FALSE } from './bytecode';
import { formatVMValue } from './vm';
import type { FFISignature, StructField, StructDef } from '../core/types';

// --- FFI Helpers ---

// Registry for struct definitions (shared across ffi-struct / ffi-call)
const vmStructRegistry = new Map<string, StructDef>();

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

const getTypeAlignment = (typeStr: string): number => {
  return getTypeSize(typeStr);
};

// Lazy-loaded bun:ffi module — avoids top-level import so the VM loads in
// environments without FFI support (e.g. web playground).
let _ffi: typeof import('bun:ffi') | null = null;
function ffi(): typeof import('bun:ffi') {
  if (!_ffi) _ffi = require('bun:ffi');
  return _ffi!;
}

// Map type string to FFIType enum
const stringToFFIType = (typeStr: string): any => {
  const { FFIType } = ffi();
  const typeMap: Record<string, any> = {
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
  if (vmStructRegistry.has(typeStr)) return FFIType.ptr;
  throw new Error(`Unknown FFI type: ${typeStr}`);
};

// Convert VM value to native FFI argument
const vmToFFI = (value: VMValue, typeStr: string): any => {
  const { ptr: ptrFn } = ffi();
  switch (typeStr) {
    case 'i8': case 'i16': case 'i32': case 'i64':
    case 'u8': case 'u16': case 'u32': case 'u64':
    case 'f32': case 'f64':
      if (typeof value !== 'number') throw new Error(`Expected number for ${typeStr}`);
      return value;
    case 'bool':
      if (typeof value === 'number' || value.kind !== 'boolean') throw new Error('Expected boolean for bool');
      return value.value;
    case 'cstring':
    case 'ptr':
      if (typeof value !== 'number') {
        if (value.kind === 'string') return Buffer.from(value.value + '\0');
        if (value.kind === 'ffi-pointer') return value.ptr;
        if (value.kind === 'ffi-struct') return ptrFn(value.buffer);
        if (value.kind === 'nil') return null;
      }
      throw new Error(`Cannot convert ${typeof value === 'number' ? 'number' : value.kind} to ${typeStr}`);
    default:
      if (vmStructRegistry.has(typeStr)) {
        if (typeof value !== 'number' && value.kind === 'ffi-struct') return ptrFn(value.buffer);
        if (typeof value !== 'number' && value.kind === 'nil') return null;
        throw new Error(`Expected struct ${typeStr}, got ${typeof value === 'number' ? 'number' : value.kind}`);
      }
      throw new Error(`Unknown FFI type: ${typeStr}`);
  }
};

// Convert native FFI result to VM value
const ffiToVM = (result: any, typeStr: string): VMValue => {
  const { CString, toArrayBuffer } = ffi();
  switch (typeStr) {
    case 'i8': case 'i16': case 'i32': case 'i64':
    case 'u8': case 'u16': case 'u32': case 'u64':
    case 'f32': case 'f64':
      return Number(result);
    case 'bool':
      return Boolean(result) ? TRUE : FALSE;
    case 'cstring':
      if (result === null) return NIL;
      return { kind: 'string', value: new CString(result).toString() };
    case 'ptr':
      if (result === null || result === 0) return NIL;
      return { kind: 'ffi-pointer', ptr: Number(result) };
    case 'void':
      return NIL;
    default: {
      const structDef = vmStructRegistry.get(typeStr);
      if (structDef) {
        if (result === null || result === 0) return NIL;
        const ptrNum = typeof result === 'bigint' ? Number(result) : result;
        const buffer = new Uint8Array(structDef.size);
        const sourceBuffer = toArrayBuffer(ptrNum, 0, structDef.size);
        buffer.set(new Uint8Array(sourceBuffer));
        return { kind: 'ffi-struct', def: structDef, buffer };
      }
      throw new Error(`Unknown FFI type: ${typeStr}`);
    }
  }
};

// Write a VM value to a struct field via DataView
const writeVMFieldValue = (view: DataView, field: StructField, value: VMValue): void => {
  const { offset, type } = field;
  switch (type) {
    case 'u8':
      if (typeof value !== 'number') throw new Error(`Expected number for ${type}`);
      view.setUint8(offset, value);
      break;
    case 'i8':
      if (typeof value !== 'number') throw new Error(`Expected number for ${type}`);
      view.setInt8(offset, value);
      break;
    case 'u16':
      if (typeof value !== 'number') throw new Error(`Expected number for ${type}`);
      view.setUint16(offset, value, true);
      break;
    case 'i16':
      if (typeof value !== 'number') throw new Error(`Expected number for ${type}`);
      view.setInt16(offset, value, true);
      break;
    case 'u32':
      if (typeof value !== 'number') throw new Error(`Expected number for ${type}`);
      view.setUint32(offset, value, true);
      break;
    case 'i32':
      if (typeof value !== 'number') throw new Error(`Expected number for ${type}`);
      view.setInt32(offset, value, true);
      break;
    case 'f32':
      if (typeof value !== 'number') throw new Error(`Expected number for ${type}`);
      view.setFloat32(offset, value, true);
      break;
    case 'u64':
      if (typeof value !== 'number') throw new Error(`Expected number for ${type}`);
      view.setBigUint64(offset, BigInt(Math.floor(value)), true);
      break;
    case 'i64':
      if (typeof value !== 'number') throw new Error(`Expected number for ${type}`);
      view.setBigInt64(offset, BigInt(Math.floor(value)), true);
      break;
    case 'f64':
      if (typeof value !== 'number') throw new Error(`Expected number for ${type}`);
      view.setFloat64(offset, value, true);
      break;
    case 'ptr':
      if (typeof value !== 'number') {
        if (value.kind === 'ffi-pointer') {
          view.setBigUint64(offset, BigInt(value.ptr), true);
        } else if (value.kind === 'nil') {
          view.setBigUint64(offset, 0n, true);
        } else {
          throw new Error(`Cannot convert ${value.kind} to ptr`);
        }
      } else {
        view.setBigUint64(offset, BigInt(Math.floor(value)), true);
      }
      break;
    case 'bool':
      if (typeof value === 'number' || value.kind !== 'boolean') throw new Error('Expected boolean for bool');
      view.setUint8(offset, value.value ? 1 : 0);
      break;
    default:
      throw new Error(`Unknown field type: ${type}`);
  }
};

// Read a VM value from a struct field via DataView
const readVMFieldValue = (view: DataView, field: StructField): VMValue => {
  const { offset, type } = field;
  switch (type) {
    case 'u8': return view.getUint8(offset);
    case 'i8': return view.getInt8(offset);
    case 'u16': return view.getUint16(offset, true);
    case 'i16': return view.getInt16(offset, true);
    case 'u32': return view.getUint32(offset, true);
    case 'i32': return view.getInt32(offset, true);
    case 'f32': return view.getFloat32(offset, true);
    case 'u64': return Number(view.getBigUint64(offset, true));
    case 'i64': return Number(view.getBigInt64(offset, true));
    case 'f64': return view.getFloat64(offset, true);
    case 'ptr': {
      const ptrValue = Number(view.getBigUint64(offset, true));
      if (ptrValue === 0) return NIL;
      return { kind: 'ffi-pointer', ptr: ptrValue };
    }
    case 'bool': return view.getUint8(offset) !== 0 ? TRUE : FALSE;
    default: throw new Error(`Unknown field type: ${type}`);
  }
};

// --- Main Export ---

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

  // --- FFI Builtins ---

  builtin('ffi-open', (args) => {
    if (args.length !== 2) throw new Error('ffi-open expects 2 arguments: path and signatures');
    const pathArg = args[0];
    const sigsArg = args[1];

    if (typeof pathArg === 'number' || pathArg.kind !== 'string') throw new Error('ffi-open: path must be a string');
    if (typeof sigsArg === 'number' || sigsArg.kind !== 'list') throw new Error('ffi-open: signatures must be a list');

    const { dlopen, suffix } = ffi();
    const libPath = pathArg.value;
    const signatures = new Map<string, FFISignature>();
    const ffiSymbols: Record<string, { args: any[], returns: any }> = {};

    for (const sig of sigsArg.elements) {
      if (typeof sig === 'number' || sig.kind !== 'list' || sig.elements.length !== 3) {
        throw new Error('ffi-open: each signature must be (list "name" (list arg-types...) "return-type")');
      }
      const nameVal = sig.elements[0], argsVal = sig.elements[1], retVal = sig.elements[2];
      if (typeof nameVal === 'number' || nameVal.kind !== 'string') throw new Error('ffi-open: function name must be a string');
      if (typeof argsVal === 'number' || argsVal.kind !== 'list') throw new Error('ffi-open: arg types must be a list');
      if (typeof retVal === 'number' || retVal.kind !== 'string') throw new Error('ffi-open: return type must be a string');

      const fnName = nameVal.value;
      const argTypes: string[] = [];
      const ffiArgTypes: any[] = [];

      for (const argType of argsVal.elements) {
        if (typeof argType === 'number' || argType.kind !== 'string') throw new Error('ffi-open: arg type must be a string');
        if (argType.value === 'void') continue;
        argTypes.push(argType.value);
        ffiArgTypes.push(stringToFFIType(argType.value));
      }

      signatures.set(fnName, { args: argTypes, returns: retVal.value });
      ffiSymbols[fnName] = { args: ffiArgTypes, returns: stringToFFIType(retVal.value) };
    }

    const fullPath = libPath.startsWith('/') || libPath.includes('.')
      ? libPath
      : `lib${libPath}.${suffix}`;
    const lib = dlopen(fullPath, ffiSymbols);

    return { kind: 'ffi-library', path: libPath, lib, signatures } as VMFFILibrary;
  });

  builtin('ffi-call', (args) => {
    if (args.length < 2) throw new Error('ffi-call expects at least 2 arguments: lib and fn-name');
    const libArg = args[0];
    const fnNameArg = args[1];
    const fnArgs = args.slice(2);

    if (typeof libArg === 'number' || libArg.kind !== 'ffi-library') throw new Error('ffi-call: first argument must be an ffi-library');
    if (typeof fnNameArg === 'number' || fnNameArg.kind !== 'string') throw new Error('ffi-call: function name must be a string');

    const fnName = fnNameArg.value;
    const sig = libArg.signatures.get(fnName);
    if (!sig) throw new Error(`ffi-call: function '${fnName}' not found in library`);

    if (fnArgs.length !== sig.args.length) {
      throw new Error(`ffi-call: ${fnName} expects ${sig.args.length} arguments, got ${fnArgs.length}`);
    }

    const convertedArgs = fnArgs.map((arg, i) => {
      const argType = sig.args[i];
      if (argType === undefined) throw new Error('ffi-call: missing argument type');
      return vmToFFI(arg, argType);
    });

    const fn = libArg.lib.symbols[fnName];
    const result = fn(...convertedArgs);
    return ffiToVM(result, sig.returns);
  });

  builtin('ffi-close', (args) => {
    if (args.length !== 1) throw new Error('ffi-close expects 1 argument');
    const libArg = args[0];
    if (typeof libArg === 'number' || libArg.kind !== 'ffi-library') throw new Error('ffi-close: argument must be an ffi-library');
    libArg.lib.close();
    return NIL;
  });

  builtin('ffi-struct', (args) => {
    if (args.length !== 2) throw new Error('ffi-struct expects 2 arguments: name and fields');
    const nameArg = args[0];
    const fieldsArg = args[1];

    if (typeof nameArg === 'number' || nameArg.kind !== 'string') throw new Error('ffi-struct: name must be a string');
    if (typeof fieldsArg === 'number' || fieldsArg.kind !== 'list') throw new Error('ffi-struct: fields must be a list');

    const fields: StructField[] = [];
    let currentOffset = 0;

    for (const field of fieldsArg.elements) {
      if (typeof field === 'number' || field.kind !== 'list' || field.elements.length !== 2) {
        throw new Error('ffi-struct: each field must be (list "name" "type")');
      }
      const fieldName = field.elements[0];
      const fieldType = field.elements[1];
      if (typeof fieldName === 'number' || fieldName.kind !== 'string') throw new Error('ffi-struct: field name must be a string');
      if (typeof fieldType === 'number' || fieldType.kind !== 'string') throw new Error('ffi-struct: field type must be a string');

      const size = getTypeSize(fieldType.value);
      const alignment = getTypeAlignment(fieldType.value);
      // Align current offset
      if (currentOffset % alignment !== 0) {
        currentOffset += alignment - (currentOffset % alignment);
      }

      fields.push({
        name: fieldName.value,
        type: fieldType.value,
        offset: currentOffset,
        size,
      });

      currentOffset += size;
    }

    const def: StructDef = {
      name: nameArg.value,
      fields,
      size: currentOffset,
    };

    vmStructRegistry.set(nameArg.value, def);
    return { kind: 'ffi-struct-def', def };
  });

  builtin('struct-new', (args) => {
    if (args.length < 1) throw new Error('struct-new expects at least 1 argument: struct-def');
    const defArg = args[0];
    if (typeof defArg === 'number' || defArg.kind !== 'ffi-struct-def') throw new Error('struct-new: first argument must be an ffi-struct-def');

    const def = defArg.def;
    const values = args.slice(1);
    if (values.length !== def.fields.length) {
      throw new Error(`struct-new: ${def.name} expects ${def.fields.length} values, got ${values.length}`);
    }

    const buffer = new Uint8Array(def.size);
    const view = new DataView(buffer.buffer);
    for (let i = 0; i < def.fields.length; i++) {
      writeVMFieldValue(view, def.fields[i], values[i]);
    }

    return { kind: 'ffi-struct', def, buffer } as VMFFIStruct;
  });

  builtin('struct-get', (args) => {
    if (args.length !== 2) throw new Error('struct-get expects 2 arguments: struct and field-name');
    const structArg = args[0];
    const fieldNameArg = args[1];

    if (typeof structArg === 'number' || structArg.kind !== 'ffi-struct') throw new Error('struct-get: first argument must be an ffi-struct');
    if (typeof fieldNameArg === 'number' || fieldNameArg.kind !== 'string') throw new Error('struct-get: second argument must be a string');

    const field = structArg.def.fields.find(f => f.name === fieldNameArg.value);
    if (!field) throw new Error(`struct-get: field '${fieldNameArg.value}' not found in struct ${structArg.def.name}`);

    const view = new DataView(structArg.buffer.buffer);
    return readVMFieldValue(view, field);
  });

  builtin('struct-set', (args) => {
    if (args.length !== 3) throw new Error('struct-set expects 3 arguments: struct, field-name, value');
    const structArg = args[0];
    const fieldNameArg = args[1];
    const value = args[2];

    if (typeof structArg === 'number' || structArg.kind !== 'ffi-struct') throw new Error('struct-set: first argument must be an ffi-struct');
    if (typeof fieldNameArg === 'number' || fieldNameArg.kind !== 'string') throw new Error('struct-set: second argument must be a string');
    if (value === undefined) throw new Error('struct-set: value is required');

    const field = structArg.def.fields.find(f => f.name === fieldNameArg.value);
    if (!field) throw new Error(`struct-set: field '${fieldNameArg.value}' not found in struct ${structArg.def.name}`);

    const view = new DataView(structArg.buffer.buffer);
    writeVMFieldValue(view, field, value);
    return NIL;
  });

  return globals;
}
