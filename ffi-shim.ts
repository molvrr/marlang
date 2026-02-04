// Browser shim for bun:ffi
// These functions are not available in the browser and will throw helpful errors

export const FFIType = {
  i8: 0,
  i16: 1,
  i32: 2,
  i64: 3,
  u8: 4,
  u16: 5,
  u32: 6,
  u64: 7,
  f32: 8,
  f64: 9,
  ptr: 10,
  cstring: 11,
  bool: 12,
  void: 13,
} as const;

export type FFIType = (typeof FFIType)[keyof typeof FFIType];

export const suffix = 'so';

export const dlopen = (_path: string, _symbols: Record<string, unknown>): never => {
  throw new Error('FFI is not available in the browser. The ffi-open function cannot be used.');
};

export const ptr = (_buffer: ArrayBuffer | Uint8Array): never => {
  throw new Error('FFI is not available in the browser.');
};

export const CString = class {
  constructor(_ptr: number) {
    throw new Error('FFI is not available in the browser.');
  }
  toString(): string {
    throw new Error('FFI is not available in the browser.');
  }
};

export const toArrayBuffer = (_ptr: number, _offset: number, _size: number): never => {
  throw new Error('FFI is not available in the browser.');
};

export const read = {
  ptr: (_ptr: number, _offset: number): never => {
    throw new Error('FFI is not available in the browser.');
  },
};
