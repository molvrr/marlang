// Compiler: AST → Bytecode for Marlang VM

import type { Expr } from '../core/types';
import { Op, type Chunk, type Instruction, type ConstantValue, type FunctionPrototype, type UpvalueDescriptor, type OpCode } from './bytecode';

// --- Compiler Context ---

type Local = {
  name: string;
  depth: number;
  slot: number;
};

type CompilerContext = {
  chunk: Chunk;
  locals: Local[];
  localCount: number;
  upvalues: UpvalueDescriptor[];
  scopeDepth: number;
  enclosing: CompilerContext | null;
  functionName: string;
  knownConstructors: Set<string>;
  knownEffectOps: Map<string, string>; // opName → effectName
};

// --- Helpers ---

function createContext(name: string, enclosing: CompilerContext | null): CompilerContext {
  return {
    chunk: { code: [], constants: [], localCount: 0 },
    locals: [],
    localCount: 0,
    upvalues: [],
    scopeDepth: 0,
    enclosing,
    functionName: name,
    knownConstructors: enclosing ? enclosing.knownConstructors : new Set([
      'None', 'Some', 'Nil', 'Cons', 'MkPair',
      'Ok', 'Err',
    ]),
    knownEffectOps: enclosing ? enclosing.knownEffectOps : new Map(),
  };
}

function emit(ctx: CompilerContext, op: OpCode, line: number, operand?: number, operand2?: number): number {
  const idx = ctx.chunk.code.length;
  ctx.chunk.code.push({ op, operand, operand2, line });
  return idx;
}

function addConstant(ctx: CompilerContext, value: ConstantValue): number {
  // Deduplicate simple constants
  for (let i = 0; i < ctx.chunk.constants.length; i++) {
    const c = ctx.chunk.constants[i];
    if (c.kind === value.kind) {
      if (c.kind === 'nil' && value.kind === 'nil') return i;
      if (c.kind === 'boolean' && value.kind === 'boolean' && c.value === value.value) return i;
      if (c.kind === 'number' && value.kind === 'number' && c.value === value.value) return i;
      if (c.kind === 'string' && value.kind === 'string' && c.value === value.value) return i;
    }
  }
  ctx.chunk.constants.push(value);
  return ctx.chunk.constants.length - 1;
}

function emitConstant(ctx: CompilerContext, value: ConstantValue, line: number): void {
  const idx = addConstant(ctx, value);
  emit(ctx, Op.PUSH_CONST, line, idx);
}

// Emit a jump instruction with a placeholder target. Returns the instruction index for patching.
function emitJump(ctx: CompilerContext, op: OpCode, line: number): number {
  return emit(ctx, op, line, 0xFFFF); // placeholder
}

// Patch a jump instruction to target the current position.
function patchJump(ctx: CompilerContext, jumpIdx: number): void {
  ctx.chunk.code[jumpIdx].operand = ctx.chunk.code.length;
}

// Patch operand2 of a MATCH_TAG instruction to target the current position.
function patchJump2(ctx: CompilerContext, jumpIdx: number): void {
  ctx.chunk.code[jumpIdx].operand2 = ctx.chunk.code.length;
}

// Emit a MATCH_TAG instruction. Returns instruction index for patching operand2 (fail target).
function emitMatchTag(ctx: CompilerContext, nameIdx: number, line: number): number {
  return emit(ctx, Op.MATCH_TAG, line, nameIdx, 0xFFFF);
}

function currentPos(ctx: CompilerContext): number {
  return ctx.chunk.code.length;
}

// --- Scope Management ---

function beginScope(ctx: CompilerContext): void {
  ctx.scopeDepth++;
}

function endScope(ctx: CompilerContext): void {
  ctx.scopeDepth--;
  // Remove locals that went out of scope
  while (ctx.locals.length > 0 && ctx.locals[ctx.locals.length - 1].depth > ctx.scopeDepth) {
    ctx.locals.pop();
  }
}

function addLocal(ctx: CompilerContext, name: string): number {
  const slot = ctx.localCount++;
  ctx.locals.push({ name, depth: ctx.scopeDepth, slot });
  return slot;
}

// --- Variable Resolution ---

function resolveLocal(ctx: CompilerContext, name: string): number {
  for (let i = ctx.locals.length - 1; i >= 0; i--) {
    if (ctx.locals[i].name === name) return ctx.locals[i].slot;
  }
  return -1;
}

function addUpvalue(ctx: CompilerContext, isLocal: boolean, index: number): number {
  // Check if already captured
  for (let i = 0; i < ctx.upvalues.length; i++) {
    if (ctx.upvalues[i].isLocal === isLocal && ctx.upvalues[i].index === index) return i;
  }
  ctx.upvalues.push({ isLocal, index });
  return ctx.upvalues.length - 1;
}

function resolveUpvalue(ctx: CompilerContext, name: string): number {
  if (!ctx.enclosing) return -1;

  // Check enclosing locals
  const localSlot = resolveLocal(ctx.enclosing, name);
  if (localSlot !== -1) {
    return addUpvalue(ctx, true, localSlot);
  }

  // Check enclosing upvalues recursively
  const upvalueIdx = resolveUpvalue(ctx.enclosing, name);
  if (upvalueIdx !== -1) {
    return addUpvalue(ctx, false, upvalueIdx);
  }

  return -1;
}

function emitVariable(ctx: CompilerContext, name: string, line: number): void {
  const localSlot = resolveLocal(ctx, name);
  if (localSlot !== -1) {
    emit(ctx, Op.PUSH_LOCAL, line, localSlot);
    return;
  }

  const upvalueIdx = resolveUpvalue(ctx, name);
  if (upvalueIdx !== -1) {
    emit(ctx, Op.PUSH_UPVALUE, line, upvalueIdx);
    return;
  }

  // Global
  const nameIdx = addConstant(ctx, { kind: 'string', value: name });
  emit(ctx, Op.PUSH_GLOBAL, line, nameIdx);
}

// --- AST Helpers ---

function listToArray(expr: Expr): Expr[] {
  const result: Expr[] = [];
  let current = expr;
  while (current.kind === 'list') {
    result.push(current.head);
    current = current.tail;
  }
  return result;
}

function getLine(expr: Expr): number {
  return expr.loc?.start.line ?? 0;
}

function getSymbol(expr: Expr): string | null {
  if (expr.kind === 'atom' && expr.value.kind === 'symbol') return expr.value.value;
  return null;
}

// Extract parameter name, handling type annotations: x or (x : Type)
function getParamName(p: Expr): string {
  if (p.kind === 'atom' && p.value.kind === 'symbol') return p.value.value;
  if (p.kind === 'list') {
    const parts = listToArray(p);
    if (parts.length === 3) {
      const first = parts[0];
      const colon = parts[1];
      if (first?.kind === 'atom' && first.value.kind === 'symbol' &&
          colon?.kind === 'atom' && colon.value.kind === 'symbol' && colon.value.value === ':') {
        return first.value.value;
      }
    }
  }
  throw new Error('Parameter must be a symbol or (symbol : Type)');
}

// Extract define name, handling type annotations
function getDefineName(nameExpr: Expr): string {
  if (nameExpr.kind === 'atom' && nameExpr.value.kind === 'symbol') return nameExpr.value.value;
  if (nameExpr.kind === 'list') {
    const parts = listToArray(nameExpr);
    if (parts.length === 3) {
      const first = parts[0];
      const colon = parts[1];
      if (first?.kind === 'atom' && first.value.kind === 'symbol' &&
          colon?.kind === 'atom' && colon.value.kind === 'symbol' && colon.value.value === ':') {
        return first.value.value;
      }
    }
  }
  throw new Error('define expects a symbol or (symbol : Type)');
}

// --- Known binary operators ---

const BINARY_OPS: Record<string, OpCode> = {
  '+': Op.ADD,
  '-': Op.SUB,
  '*': Op.MUL,
  '/': Op.DIV,
  '<': Op.LT,
  '>': Op.GT,
  '=': Op.EQ,
  '<=': Op.LE,
  '>=': Op.GE,
};

// --- Main Compilation ---

function compileExpr(ctx: CompilerContext, expr: Expr, isTail: boolean): void {
  const line = getLine(expr);

  // Nil
  if (expr.kind === 'nil') {
    emitConstant(ctx, { kind: 'nil' }, line);
    return;
  }

  // Atom
  if (expr.kind === 'atom') {
    if (expr.value.kind === 'number') {
      emitConstant(ctx, { kind: 'number', value: expr.value.value }, line);
      return;
    }
    if (expr.value.kind === 'string') {
      emitConstant(ctx, { kind: 'string', value: expr.value.value }, line);
      return;
    }
    // Symbol
    const sym = expr.value.value;
    if (sym === '#t') { emitConstant(ctx, { kind: 'boolean', value: true }, line); return; }
    if (sym === '#f') { emitConstant(ctx, { kind: 'boolean', value: false }, line); return; }
    if (sym === 'nil') { emitConstant(ctx, { kind: 'nil' }, line); return; }
    // Variable reference
    emitVariable(ctx, sym, line);
    return;
  }

  // List — check for special forms
  const elements = listToArray(expr);
  if (elements.length === 0) {
    emitConstant(ctx, { kind: 'nil' }, line);
    return;
  }

  const first = elements[0];
  const sym = getSymbol(first);

  if (sym !== null) {
    switch (sym) {
      case 'define': return compileDefine(ctx, elements, line);
      case 'lambda': return compileLambda(ctx, elements, line);
      case 'if': return compileIf(ctx, elements, line, isTail);
      case 'let': return compileLet(ctx, elements, line, isTail);
      case 'begin': return compileBegin(ctx, elements, line, isTail);
      case 'cond': return compileCond(ctx, elements, line, isTail);
      case 'match': return compileMatch(ctx, elements, line, isTail);
      case 'data': return compileData(ctx, elements, line);
      case 'effect': return compileEffect(ctx, elements, line);
      case 'perform': return compilePerform(ctx, elements, line);
      case 'handle': return compileHandle(ctx, elements, line, isTail);
      case 'resume': return compileResume(ctx, elements, line);
      case 'import': return compileImport(ctx, elements, line);
      case 'class': case 'instance': case 'type':
        emitConstant(ctx, { kind: 'nil' }, line);
        return;
    }

    // Check for binary operator optimization
    const binOp = BINARY_OPS[sym];
    if (binOp !== undefined && elements.length === 3) {
      compileExpr(ctx, elements[1], false);
      compileExpr(ctx, elements[2], false);
      emit(ctx, binOp, line);
      return;
    }

    // Unary negate: (- x) or (negate x)
    if ((sym === '-' || sym === 'negate') && elements.length === 2) {
      compileExpr(ctx, elements[1], false);
      emit(ctx, Op.NEG, line);
      return;
    }
  }

  // Function application
  compileCall(ctx, elements, line, isTail);
}

// --- Special Form Compilation ---

function compileDefine(ctx: CompilerContext, elements: Expr[], line: number): void {
  if (elements.length !== 3) throw new Error('define expects 2 arguments');
  const name = getDefineName(elements[1]);
  compileExpr(ctx, elements[2], false);
  const nameIdx = addConstant(ctx, { kind: 'string', value: name });
  emit(ctx, Op.SET_GLOBAL, line, nameIdx);
  emitConstant(ctx, { kind: 'nil' }, line); // define returns nil
}

function compileLambda(ctx: CompilerContext, elements: Expr[], line: number): void {
  if (elements.length < 3) throw new Error('lambda expects at least 2 arguments');

  const paramsExpr = elements[1];
  const paramExprs = listToArray(paramsExpr);
  const params = paramExprs.map(getParamName);
  const bodyExprs = elements.slice(2);

  // Create new compiler context for the function
  const fnCtx = createContext('<lambda>', ctx);
  fnCtx.knownConstructors = ctx.knownConstructors;
  fnCtx.knownEffectOps = ctx.knownEffectOps;

  // Add parameters as locals
  for (const param of params) {
    addLocal(fnCtx, param);
  }

  // Compile body
  compileSequence(fnCtx, bodyExprs, true);
  emit(fnCtx, Op.RETURN, line);

  fnCtx.chunk.localCount = fnCtx.localCount;

  // Create prototype
  const proto: FunctionPrototype = {
    name: '<lambda>',
    arity: params.length,
    upvalues: fnCtx.upvalues,
    chunk: fnCtx.chunk,
  };

  // Emit CLOSURE in the enclosing context
  const protoIdx = addConstant(ctx, { kind: 'prototype', proto });
  emit(ctx, Op.CLOSURE, line, protoIdx);
}

function compileIf(ctx: CompilerContext, elements: Expr[], line: number, isTail: boolean): void {
  if (elements.length < 3 || elements.length > 4) throw new Error('if expects 2 or 3 arguments');

  compileExpr(ctx, elements[1], false); // condition
  const jumpToElse = emitJump(ctx, Op.JUMP_IF_FALSE, line);

  compileExpr(ctx, elements[2], isTail); // then branch
  const jumpToEnd = emitJump(ctx, Op.JUMP, line);

  patchJump(ctx, jumpToElse);
  if (elements.length === 4) {
    compileExpr(ctx, elements[3], isTail); // else branch
  } else {
    emitConstant(ctx, { kind: 'nil' }, line); // no else → nil
  }

  patchJump(ctx, jumpToEnd);
}

function compileLet(ctx: CompilerContext, elements: Expr[], line: number, isTail: boolean): void {
  if (elements.length < 3) throw new Error('let expects at least 2 arguments');

  const bindingsExpr = elements[1];
  const bindingsList = listToArray(bindingsExpr);
  const bodyExprs = elements.slice(2);

  beginScope(ctx);

  for (const binding of bindingsList) {
    const pair = listToArray(binding);
    if (pair.length !== 2) throw new Error('let binding must be (name value)');
    const name = getParamName(pair[0]); // handles (name : Type) too
    compileExpr(ctx, pair[1], false);
    const slot = addLocal(ctx, name);
    emit(ctx, Op.SET_LOCAL, line, slot);
  }

  compileSequence(ctx, bodyExprs, isTail);

  endScope(ctx);
}

function compileBegin(ctx: CompilerContext, elements: Expr[], line: number, isTail: boolean): void {
  const bodyExprs = elements.slice(1);
  if (bodyExprs.length === 0) {
    emitConstant(ctx, { kind: 'nil' }, line);
    return;
  }
  compileSequence(ctx, bodyExprs, isTail);
}

function compileCond(ctx: CompilerContext, elements: Expr[], line: number, isTail: boolean): void {
  const clauses = elements.slice(1);
  if (clauses.length === 0) {
    emitConstant(ctx, { kind: 'nil' }, line);
    return;
  }

  const endJumps: number[] = [];

  for (const clauseExpr of clauses) {
    const clause = listToArray(clauseExpr);
    if (clause.length !== 2) throw new Error('cond clause must be (test expr)');

    const testExpr = clause[0];
    const resultExpr = clause[1];

    // Check for else clause
    if (getSymbol(testExpr) === 'else') {
      compileExpr(ctx, resultExpr, isTail);
      endJumps.push(emitJump(ctx, Op.JUMP, line));
      break;
    }

    compileExpr(ctx, testExpr, false);
    const jumpToNext = emitJump(ctx, Op.JUMP_IF_FALSE, line);
    compileExpr(ctx, resultExpr, isTail);
    endJumps.push(emitJump(ctx, Op.JUMP, line));
    patchJump(ctx, jumpToNext);
  }

  // If no else clause was found, push nil as default
  emitConstant(ctx, { kind: 'nil' }, line);

  for (const jump of endJumps) {
    patchJump(ctx, jump);
  }
}

function compileMatch(ctx: CompilerContext, elements: Expr[], line: number, isTail: boolean): void {
  if (elements.length < 3) throw new Error('match expects scrutinee and at least one branch');

  // Compile scrutinee — leaves value on TOS
  compileExpr(ctx, elements[1], false);

  const branches = elements.slice(2);
  const endJumps: number[] = [];

  for (const branchExpr of branches) {
    const branchElements = listToArray(branchExpr);
    if (branchElements.length !== 2) throw new Error('match branch must be (pattern body)');

    const patternExpr = branchElements[0];
    const bodyExpr = branchElements[1];

    // DUP scrutinee so pattern can consume the copy
    emit(ctx, Op.DUP, line);

    // compilePattern: consumes DUP'd copy on success, leaves it on failure
    const failPatches = compilePattern(ctx, patternExpr, line);

    // Success path: pattern consumed the copy. Original scrutinee still below.
    emit(ctx, Op.POP, line); // pop original scrutinee
    compileExpr(ctx, bodyExpr, isTail);
    endJumps.push(emitJump(ctx, Op.JUMP, line));

    // Fail path: pattern left the DUP'd copy on TOS. Pop it.
    const failTarget = currentPos(ctx);
    emit(ctx, Op.POP, line); // pop DUP'd copy; original scrutinee now on TOS

    // Patch all fail jumps to point here
    for (const fp of failPatches) {
      if (fp.kind === 'jump') {
        ctx.chunk.code[fp.idx].operand = failTarget;
      } else {
        ctx.chunk.code[fp.idx].operand2 = failTarget;
      }
    }
  }

  // No branch matched — scrutinee still on TOS
  emit(ctx, Op.POP, line);
  emitConstant(ctx, { kind: 'string', value: 'No pattern matched in match expression' }, line);
  emit(ctx, Op.HALT, line);

  for (const jump of endJumps) {
    patchJump(ctx, jump);
  }
}

// Compile a pattern. Value to match is on TOS.
// On success: TOS consumed, bindings in locals, falls through.
// On failure: jumps to failTarget (caller patches). TOS restored to original value.
// Returns array of instruction indices needing patch to the overall fail label.
// For JUMP_IF_FALSE: patch operand. For MATCH_TAG: patch operand2.
type FailPatch = { idx: number; kind: 'jump' | 'match_tag' };

function patchFail(ctx: CompilerContext, patch: FailPatch): void {
  if (patch.kind === 'jump') {
    patchJump(ctx, patch.idx);
  } else {
    patchJump2(ctx, patch.idx);
  }
}

function compilePattern(ctx: CompilerContext, pattern: Expr, line: number): FailPatch[] {
  // Wildcard _
  if (pattern.kind === 'atom' && pattern.value.kind === 'symbol' && pattern.value.value === '_') {
    emit(ctx, Op.POP, line);
    return [];
  }

  // Variable binding (not a known constructor)
  if (pattern.kind === 'atom' && pattern.value.kind === 'symbol') {
    const name = pattern.value.value;
    if (ctx.knownConstructors.has(name)) {
      // Nullary constructor pattern: MATCH_TAG peeks, on fail jumps, on success falls through
      const nameIdx = addConstant(ctx, { kind: 'string', value: name });
      const idx = emitMatchTag(ctx, nameIdx, line);
      emit(ctx, Op.POP, line); // consume matched value on success
      return [{ idx, kind: 'match_tag' }];
    }
    // Variable: bind the value (consumes TOS)
    const slot = addLocal(ctx, name);
    emit(ctx, Op.SET_LOCAL, line, slot);
    return [];
  }

  // Number literal
  if (pattern.kind === 'atom' && pattern.value.kind === 'number') {
    emit(ctx, Op.DUP, line);
    emitConstant(ctx, { kind: 'number', value: pattern.value.value }, line);
    emit(ctx, Op.EQ, line);
    const idx = emitJump(ctx, Op.JUMP_IF_FALSE, line);
    emit(ctx, Op.POP, line); // consume matched value
    return [{ idx, kind: 'jump' }];
  }

  // String literal
  if (pattern.kind === 'atom' && pattern.value.kind === 'string') {
    emit(ctx, Op.DUP, line);
    emitConstant(ctx, { kind: 'string', value: pattern.value.value }, line);
    emit(ctx, Op.EQ, line);
    const idx = emitJump(ctx, Op.JUMP_IF_FALSE, line);
    emit(ctx, Op.POP, line);
    return [{ idx, kind: 'jump' }];
  }

  // Nil pattern ()
  if (pattern.kind === 'nil') {
    emit(ctx, Op.DUP, line);
    emitConstant(ctx, { kind: 'nil' }, line);
    emit(ctx, Op.EQ, line);
    const idx = emitJump(ctx, Op.JUMP_IF_FALSE, line);
    emit(ctx, Op.POP, line);
    return [{ idx, kind: 'jump' }];
  }

  // Constructor pattern: (CtorName args...)
  if (pattern.kind === 'list') {
    const patternElements = listToArray(pattern);
    if (patternElements.length === 0) {
      emit(ctx, Op.DUP, line);
      emitConstant(ctx, { kind: 'nil' }, line);
      emit(ctx, Op.EQ, line);
      const idx = emitJump(ctx, Op.JUMP_IF_FALSE, line);
      emit(ctx, Op.POP, line);
      return [{ idx, kind: 'jump' }];
    }

    const ctorExpr = patternElements[0];
    if (ctorExpr.kind !== 'atom' || ctorExpr.value.kind !== 'symbol') {
      throw new Error('Pattern constructor must be a symbol');
    }

    const ctorName = ctorExpr.value.value;
    const argPatterns = patternElements.slice(1);

    // MATCH_TAG: peek TOS, if tag doesn't match jump to fail target (operand2)
    const nameIdx = addConstant(ctx, { kind: 'string', value: ctorName });
    const tagFailIdx = emitMatchTag(ctx, nameIdx, line);
    // If tag matches, fall through. TOS is still the ADT value.

    if (argPatterns.length === 0) {
      // Nullary constructor with parens: (None)
      emit(ctx, Op.POP, line); // consume ADT
      return [{ idx: tagFailIdx, kind: 'match_tag' }];
    }

    // For each arg pattern: GET_FIELD (peek ADT, push field), then match the field.
    // On sub-pattern failure: field value is on TOS (unconsumed), ADT is below.
    // We need cleanup: POP field, then the ADT is TOS and should be left as-is for
    // the overall fail path.
    // Strategy: collect sub-fail patches. After all fields, emit the success cleanup (POP ADT).
    // Then emit cleanup trampolines for each sub-fail: POP field, jump to overall fail.

    const subFailPatches: FailPatch[] = [];

    for (let i = 0; i < argPatterns.length; i++) {
      emit(ctx, Op.GET_FIELD, line, i); // peek ADT, push field[i]
      const subFails = compilePattern(ctx, argPatterns[i], line);
      subFailPatches.push(...subFails);
    }

    // Success: all sub-patterns matched. ADT is still on stack. Pop it.
    emit(ctx, Op.POP, line);
    const successJump = emitJump(ctx, Op.JUMP, line); // jump over cleanup

    // Cleanup for sub-pattern failures:
    // When a sub-pattern fails, the field value is on TOS (unconsumed by the sub-pattern).
    // Below it is the ADT value. We need to POP the field, leaving ADT on TOS.
    // Then we can fall through to the overall fail path where ADT is on TOS.
    const cleanupPos = currentPos(ctx);
    emit(ctx, Op.POP, line); // POP the field value; ADT is now TOS
    // Don't POP the ADT — the overall fail handler (in compileMatch) will handle it.

    // Patch sub-fails to jump to cleanup
    for (const sf of subFailPatches) {
      patchFail(ctx, sf);
      // Redirect to cleanup position
      // But wait — we already patched them to cleanupPos. Actually, patchFail sets
      // the jump target to currentPos at time of call. Let me rethink.
    }

    // Actually, I need to patch all sub-fails BEFORE emitting the cleanup code.
    // Let me restructure: emit cleanup first at a known position, then patch.

    // Oops, I already emitted cleanup. The sub-fails were collected but not yet patched.
    // Let me patch them now to point to cleanupPos.
    for (const sf of subFailPatches) {
      if (sf.kind === 'jump') {
        ctx.chunk.code[sf.idx].operand = cleanupPos;
      } else {
        ctx.chunk.code[sf.idx].operand2 = cleanupPos;
      }
    }

    patchJump(ctx, successJump); // success path continues here

    // Return the tag fail (ADT on TOS, no cleanup needed) plus a new jump from cleanup
    // that leaves ADT on TOS (cleanup already popped the field).
    // After cleanup POP, we need to jump to the overall fail. But compileMatch handles
    // that by patching our returned fail patches. So we need to return a patch for
    // the cleanup path too. Let's emit a jump from cleanup:
    // Wait, after cleanup POP we just fall through. We need an explicit jump to fail.
    // But we don't know fail yet. So add a jump that the caller will patch.

    // Actually let me restructure more cleanly. The cleanup block should jump to
    // a fail target. We return that jump for the caller to patch.

    // Let me redo the end of this function:
    // After "POP field" in cleanup, emit a JUMP to fail (placeholder, returned for patching)
    const cleanupFailJump = emitJump(ctx, Op.JUMP, line);

    // Re-patch successJump to skip over the cleanup+jump
    ctx.chunk.code[successJump].operand = currentPos(ctx);

    return [
      { idx: tagFailIdx, kind: 'match_tag' },       // tag mismatch: ADT on TOS
      { idx: cleanupFailJump, kind: 'jump' },         // sub-pattern fail: ADT on TOS (field was popped)
    ];
  }

  throw new Error(`Unknown pattern kind: ${pattern.kind}`);
}

function compileData(ctx: CompilerContext, elements: Expr[], line: number): void {
  if (elements.length < 3) throw new Error('data expects type name and at least one constructor');

  const ctorExprs = elements.slice(2);

  for (const ctorExpr of ctorExprs) {
    if (ctorExpr.kind === 'atom' && ctorExpr.value.kind === 'symbol') {
      // Nullary constructor
      const ctorName = ctorExpr.value.value;
      ctx.knownConstructors.add(ctorName);
      const nameIdx = addConstant(ctx, { kind: 'string', value: ctorName });
      emit(ctx, Op.CONSTRUCT, line, nameIdx, 0);
      emit(ctx, Op.SET_GLOBAL, line, nameIdx);
    } else if (ctorExpr.kind === 'list') {
      const ctorElements = listToArray(ctorExpr);
      const first = ctorElements[0];
      if (first?.kind !== 'atom' || first.value.kind !== 'symbol') {
        throw new Error('constructor name must be a symbol');
      }
      const ctorName = first.value.value;
      const arity = ctorElements.length - 1;
      ctx.knownConstructors.add(ctorName);

      const nameIdx = addConstant(ctx, { kind: 'string', value: ctorName });
      if (arity === 0) {
        emit(ctx, Op.CONSTRUCT, line, nameIdx, 0);
        emit(ctx, Op.SET_GLOBAL, line, nameIdx);
      } else {
        // Store constructor function as global
        // We push an adt-constructor constant
        const ctorConst = addConstant(ctx, { kind: 'string', value: `ctor:${ctorName}:${arity}` });
        // Use a special encoding: push a marker that the VM interprets
        // Actually, let's just emit code that creates the adt-constructor value
        // We need a way to push an adt-constructor. Let me use CONSTRUCT with a special flag.
        // Simpler: add the adt-constructor to globals at VM init time, or emit inline.
        // For now: emit PUSH_CONST with a string encoding, and have SET_GLOBAL store it.
        // The VM will pre-register constructors.
        // Actually, simplest approach: emit nothing for N-ary constructors, register them in the VM's
        // globals table before running. But the compiler doesn't run VM code...

        // Let me use a different approach: the compiler emits special constants
        // that the VM recognizes as constructor descriptors.
        // Add a new constant kind... or just use the existing ones creatively.

        // Pragmatic approach: emit code that pushes an adt-constructor value.
        // I'll add support for this in the VM by having CONSTRUCT with arity=-1 mean "make constructor fn"
        // No, let me keep it simple. Use PUSH_CONST with a specially tagged string, and SET_GLOBAL.

        // Actually, the cleanest approach: handle data constructors the same way the interpreter does.
        // The interpreter stores adt-constructor values in the env. In the VM, we need
        // adt-constructor values in the globals. Let's register them when the VM is created,
        // similar to builtins. But data defs are user-defined...

        // OK, simplest working approach: the data form at compile time registers constructors
        // as known, and at runtime we pre-populate globals from a "constructor registry" that
        // the compiler builds.

        // For now, I'll use a workaround: store constructor info in the chunk's constants
        // and have the VM handle a DEFINE_CTOR pseudo-instruction... or:

        // Let me just not emit runtime code for N-ary constructors.
        // Instead, the compiler records them, and the VM's init phase creates them as globals.
        // We store them in the chunk metadata.

        // Simplest: just push them as globals during compilation's preamble
        // Actually, I'll do it properly later. For now, we rely on builtins for built-in
        // constructors, and for user-defined data types, we can handle them.

        // WORKAROUND: Use a special constant kind. Let me store adt-constructor info
        // in a constant and use PUSH_CONST + SET_GLOBAL.
        // The VM will check if a constant is an adt-constructor descriptor.

        // I'll extend ConstantValue... but I said I wouldn't modify bytecode.ts much.
        // OK let me just store it as a tagged string for now.
        emitConstant(ctx, { kind: 'string', value: `\0ctor:${ctorName}:${arity}` }, line);
        emit(ctx, Op.SET_GLOBAL, line, nameIdx);
      }
    }
  }

  emitConstant(ctx, { kind: 'nil' }, line); // data returns nil
}

function compileEffect(ctx: CompilerContext, elements: Expr[], line: number): void {
  if (elements.length < 2) throw new Error('effect expects a name');
  const nameExpr = elements[1];
  if (nameExpr?.kind !== 'atom' || nameExpr.value.kind !== 'symbol') {
    throw new Error('effect name must be a symbol');
  }
  const effectName = nameExpr.value.value;

  // Register operations
  for (let i = 2; i < elements.length; i++) {
    const opExpr = elements[i];
    if (opExpr?.kind === 'list') {
      const opElements = listToArray(opExpr);
      const opNameExpr = opElements[0];
      if (opNameExpr?.kind === 'atom' && opNameExpr.value.kind === 'symbol') {
        ctx.knownEffectOps.set(opNameExpr.value.value, effectName);
      }
    }
  }

  emitConstant(ctx, { kind: 'nil' }, line);
}

function compilePerform(ctx: CompilerContext, elements: Expr[], line: number): void {
  if (elements.length < 2) throw new Error('perform expects an operation name');
  const opExpr = elements[1];
  if (opExpr?.kind !== 'atom' || opExpr.value.kind !== 'symbol') {
    throw new Error('perform expects an operation name symbol');
  }
  const opName = opExpr.value.value;

  // Compile arguments
  const argExprs = elements.slice(2);
  for (const argExpr of argExprs) {
    compileExpr(ctx, argExpr, false);
  }

  const nameIdx = addConstant(ctx, { kind: 'string', value: opName });
  emit(ctx, Op.PERFORM, line, nameIdx, argExprs.length);
}

function compileHandle(ctx: CompilerContext, elements: Expr[], line: number, isTail: boolean): void {
  if (elements.length < 3) throw new Error('handle expects an expression and handler clauses');

  // For now, compile handle as a higher-level construct.
  // The body and handler clauses are compiled into prototypes.
  // The VM's PUSH_HANDLER/POP_HANDLER manage the handler stack.

  const bodyExpr = elements[1];
  const handlerClauses = elements.slice(2);

  // Parse handler clauses and compile each handler body as a lambda
  const handlers: { opName: string; params: string[]; contParam: string; bodyProto: FunctionPrototype }[] = [];
  let returnHandler: { param: string; bodyProto: FunctionPrototype } | null = null;

  for (const clause of handlerClauses) {
    const clauseElements = listToArray(clause);
    if (clauseElements.length !== 3) throw new Error('handle clause must be ((pattern) -> body)');
    const arrow = clauseElements[1];
    if (!arrow || getSymbol(arrow) !== '->') throw new Error('handle clause must use ->');

    const patternExpr = clauseElements[0];
    const handlerBody = clauseElements[2];
    const patternElements = listToArray(patternExpr);

    if (patternElements.length === 0) throw new Error('handler pattern cannot be empty');
    const opNameExpr = patternElements[0];
    if (opNameExpr.kind !== 'atom' || opNameExpr.value.kind !== 'symbol') {
      throw new Error('handler pattern must start with operation name');
    }
    const opName = opNameExpr.value.value;

    if (opName === 'return') {
      if (patternElements.length !== 2) throw new Error('return handler expects one parameter');
      const param = getParamName(patternElements[1]);

      // Compile return handler as a function prototype
      const fnCtx = createContext('<return-handler>', ctx);
      fnCtx.knownConstructors = ctx.knownConstructors;
      fnCtx.knownEffectOps = ctx.knownEffectOps;
      addLocal(fnCtx, param);
      compileExpr(fnCtx, handlerBody, true);
      emit(fnCtx, Op.RETURN, line);
      fnCtx.chunk.localCount = fnCtx.localCount;

      returnHandler = {
        param,
        bodyProto: { name: '<return-handler>', arity: 1, upvalues: fnCtx.upvalues, chunk: fnCtx.chunk },
      };
    } else {
      const params: string[] = [];
      for (let i = 1; i < patternElements.length; i++) {
        params.push(getParamName(patternElements[i]));
      }
      let contParam = 'k';
      if (params.length > 0) {
        contParam = params.pop()!;
      }

      // Compile handler body as a function prototype
      const fnCtx = createContext(`<handler:${opName}>`, ctx);
      fnCtx.knownConstructors = ctx.knownConstructors;
      fnCtx.knownEffectOps = ctx.knownEffectOps;
      for (const p of params) addLocal(fnCtx, p);
      addLocal(fnCtx, contParam);
      compileExpr(fnCtx, handlerBody, true);
      emit(fnCtx, Op.RETURN, line);
      fnCtx.chunk.localCount = fnCtx.localCount;

      handlers.push({
        opName,
        params,
        contParam,
        bodyProto: {
          name: `<handler:${opName}>`,
          arity: params.length + 1, // params + continuation
          upvalues: fnCtx.upvalues,
          chunk: fnCtx.chunk,
        },
      });
    }
  }

  // Compile body as a function prototype (thunk)
  const bodyCtx = createContext('<handle-body>', ctx);
  bodyCtx.knownConstructors = ctx.knownConstructors;
  bodyCtx.knownEffectOps = ctx.knownEffectOps;
  compileExpr(bodyCtx, bodyExpr, true);
  emit(bodyCtx, Op.RETURN, line);
  bodyCtx.chunk.localCount = bodyCtx.localCount;

  const bodyProto: FunctionPrototype = {
    name: '<handle-body>',
    arity: 0,
    upvalues: bodyCtx.upvalues,
    chunk: bodyCtx.chunk,
  };

  // Emit handler setup
  // Store handler info as constants
  const bodyProtoIdx = addConstant(ctx, { kind: 'prototype', proto: bodyProto });

  // For each handler, store its prototype
  const handlerInfos: { opNameIdx: number; protoIdx: number; arity: number }[] = [];
  for (const h of handlers) {
    const opNameIdx = addConstant(ctx, { kind: 'string', value: h.opName });
    const protoIdx = addConstant(ctx, { kind: 'prototype', proto: h.bodyProto });
    handlerInfos.push({ opNameIdx, protoIdx, arity: h.bodyProto.arity });
  }

  let returnProtoIdx = -1;
  if (returnHandler) {
    returnProtoIdx = addConstant(ctx, { kind: 'prototype', proto: returnHandler.bodyProto });
  }

  // Encode handler info as a constant (we use a string encoding for simplicity)
  // Format: JSON-like encoding in a string constant
  const handlerDescriptor = JSON.stringify({
    bodyProtoIdx,
    handlers: handlerInfos.map(h => ({ opNameIdx: h.opNameIdx, protoIdx: h.protoIdx, arity: h.arity })),
    returnProtoIdx,
  });
  const descriptorIdx = addConstant(ctx, { kind: 'string', value: `\0handler:${handlerDescriptor}` });

  emit(ctx, Op.PUSH_HANDLER, line, descriptorIdx);
  // The VM will: compile the body thunk, set up handler frame, run body, handle effects
  // Result is left on stack
}

function compileResume(ctx: CompilerContext, elements: Expr[], line: number): void {
  if (elements.length !== 3) throw new Error('resume expects 2 arguments');
  compileExpr(ctx, elements[1], false); // continuation
  compileExpr(ctx, elements[2], false); // value
  emit(ctx, Op.RESUME, line);
}

function compileImport(ctx: CompilerContext, elements: Expr[], line: number): void {
  // For now, imports are not supported in the VM
  // We'll add this in the integration phase
  throw new Error('import is not yet supported in the VM. Use --no-typecheck without --vm for imports.');
}

function compileCall(ctx: CompilerContext, elements: Expr[], line: number, isTail: boolean): void {
  // Compile function expression
  compileExpr(ctx, elements[0], false);

  // Compile arguments
  const arity = elements.length - 1;
  for (let i = 1; i < elements.length; i++) {
    compileExpr(ctx, elements[i], false);
  }

  if (isTail) {
    emit(ctx, Op.TAIL_CALL, line, arity);
  } else {
    emit(ctx, Op.CALL, line, arity);
  }
}

function compileSequence(ctx: CompilerContext, exprs: Expr[], isTail: boolean): void {
  if (exprs.length === 0) {
    emitConstant(ctx, { kind: 'nil' }, 0);
    return;
  }
  for (let i = 0; i < exprs.length - 1; i++) {
    compileExpr(ctx, exprs[i], false);
    emit(ctx, Op.POP, getLine(exprs[i]));
  }
  compileExpr(ctx, exprs[exprs.length - 1], isTail);
}

// --- Public API ---

export function compile(expr: Expr): Chunk {
  const ctx = createContext('<script>', null);
  compileExpr(ctx, expr, false);
  emit(ctx, Op.RETURN, getLine(expr));
  ctx.chunk.localCount = ctx.localCount;
  return ctx.chunk;
}

export function compileProgram(exprs: Expr[]): Chunk {
  const ctx = createContext('<script>', null);

  if (exprs.length === 0) {
    emitConstant(ctx, { kind: 'nil' }, 0);
  } else {
    for (let i = 0; i < exprs.length - 1; i++) {
      compileExpr(ctx, exprs[i], false);
      emit(ctx, Op.POP, getLine(exprs[i]));
    }
    compileExpr(ctx, exprs[exprs.length - 1], false);
  }

  emit(ctx, Op.RETURN, 0);
  ctx.chunk.localCount = ctx.localCount;
  return ctx.chunk;
}
