// Disassembler for Marlang bytecode

import { OpName, type Chunk, type Instruction, type FunctionPrototype } from './bytecode';

export function disassemble(chunk: Chunk, name?: string): string {
  let output = `=== ${name ?? '<script>'} ===\n`;

  for (let i = 0; i < chunk.code.length; i++) {
    output += formatInstruction(chunk, i);
  }

  // Disassemble nested prototypes
  for (const constant of chunk.constants) {
    if (constant.kind === 'prototype') {
      output += '\n' + disassemble(constant.proto.chunk, constant.proto.name);
    }
  }

  return output;
}

function formatInstruction(chunk: Chunk, offset: number): string {
  const inst = chunk.code[offset];
  const line = inst.line;
  const opName = OpName[inst.op] ?? `UNKNOWN(${inst.op})`;

  let base = `${offset.toString().padStart(4)} | ${line.toString().padStart(4)} | ${opName.padEnd(16)}`;

  if (inst.operand !== undefined) {
    base += ` ${inst.operand.toString().padStart(4)}`;

    // Add constant info for readability
    const annotation = getAnnotation(chunk, inst);
    if (annotation) {
      base += `    ; ${annotation}`;
    }

    if (inst.operand2 !== undefined) {
      base += ` ${inst.operand2.toString().padStart(4)}`;
    }
  }

  return base + '\n';
}

function getAnnotation(chunk: Chunk, inst: Instruction): string | null {
  const opName = OpName[inst.op];
  if (!opName) return null;

  // Opcodes that reference constants
  if (opName === 'PUSH_CONST' || opName === 'PUSH_GLOBAL' || opName === 'SET_GLOBAL' ||
      opName === 'MATCH_TAG' || opName === 'CONSTRUCT' || opName === 'PERFORM') {
    const constant = chunk.constants[inst.operand!];
    if (!constant) return null;
    switch (constant.kind) {
      case 'number': return String(constant.value);
      case 'string': {
        const s = constant.value;
        if (s.startsWith('\0')) return `<special>`;
        return s.length > 30 ? `"${s.substring(0, 30)}..."` : `"${s}"`;
      }
      case 'boolean': return constant.value ? '#t' : '#f';
      case 'nil': return 'nil';
      case 'prototype': return `<fn ${constant.proto.name}/${constant.proto.arity}>`;
    }
  }

  if (opName === 'CLOSURE') {
    const constant = chunk.constants[inst.operand!];
    if (constant?.kind === 'prototype') {
      return `<fn ${constant.proto.name}/${constant.proto.arity}>`;
    }
  }

  return null;
}
