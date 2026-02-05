# Bytecode VM for Marlang

**Type:** Feature Plan
**Effort:** XL (>2 days)
**Status:** Ready for implementation

## Problem

The tree-walking interpreter re-traverses AST nodes on every evaluation. This causes measurable overhead for:
- Recursive functions (e.g., `fib(35)`)
- Tight loops
- Pattern matching dispatch
- Effect-heavy code with many perform/handle cycles

## Constraints

| Constraint | Impact |
|------------|--------|
| Effects from day one | Must implement continuation capture in VM |
| Feature parity | All current interpreter features must work |
| Separate module | Coexists with tree-walker, activated via `--vm` flag |
| Debug info priority | Stack traces and source mapping are critical |
| No mutation | Flat closures (copy values) - no shared cells needed |

## Solution

**Stack-based VM with stack copying for continuations.**

Rationale:
- Stack-based is simpler to implement and debug than register-based
- Stack copying maps directly to how delimited continuations work
- Research confirms stack-based is adequate for interpreted VMs
- Can optimize to register-based later if profiling warrants it

## Architecture Overview

```
┌─────────────┐     ┌──────────────┐     ┌─────────────┐
│   Parser    │ ──► │   Compiler   │ ──► │     VM      │
│  (existing) │     │ (new)        │     │   (new)     │
└─────────────┘     └──────────────┘     └─────────────┘
      │                    │                    │
      ▼                    ▼                    ▼
    Expr              Chunk/Proto          RuntimeValue
   (AST)             (bytecode)            (result)
```

## Deliverables

### D1: Bytecode IR and Instruction Set (M)

**File:** `src/vm/bytecode.ts`

```typescript
export type OpCode =
  // Stack manipulation
  | 'PUSH_CONST'      // [idx] Push constant from pool
  | 'PUSH_LOCAL'      // [slot] Push local variable
  | 'SET_LOCAL'       // [slot] Set local (for let bindings)
  | 'PUSH_UPVALUE'    // [idx] Push captured variable
  | 'POP'             // Discard top of stack
  | 'DUP'             // Duplicate top of stack

  // Arithmetic
  | 'ADD' | 'SUB' | 'MUL' | 'DIV' | 'NEG'

  // Comparison
  | 'EQ' | 'LT' | 'GT' | 'LE' | 'GE'

  // Boolean
  | 'NOT'

  // Control flow
  | 'JUMP'            // [offset] Unconditional jump
  | 'JUMP_IF_FALSE'   // [offset] Pop, jump if falsy
  | 'JUMP_IF_TRUE'    // [offset] Pop, jump if truthy
  | 'RETURN'          // Return top of stack

  // Functions
  | 'CALL'            // [arity] Call function
  | 'TAIL_CALL'       // [arity] Tail call (reuse frame)
  | 'CLOSURE'         // [proto_idx, upvalue_count] Create closure

  // ADTs
  | 'CONSTRUCT'       // [ctor_idx, arity] Build ADT value
  | 'GET_FIELD'       // [field_idx] Extract field from ADT
  | 'MATCH_TAG'       // [ctor_idx, jump_offset] Check tag, jump if mismatch

  // Effects
  | 'PUSH_HANDLER'    // [handler_proto_idx] Install handler frame
  | 'POP_HANDLER'     // Remove handler frame
  | 'PERFORM'         // [op_idx, arity] Perform effect operation
  | 'RESUME'          // Resume continuation with value

  // Builtins
  | 'CALL_BUILTIN'    // [builtin_idx, arity] Call native function

export type Instruction = {
  op: OpCode;
  operand?: number;
  operand2?: number;  // For ops needing two operands
};

export type Chunk = {
  code: Instruction[];
  constants: ConstantValue[];
  lines: number[];           // Source line per instruction
  sourceFile?: string;
};

export type ConstantValue =
  | { kind: 'number'; value: number }
  | { kind: 'string'; value: string }
  | { kind: 'boolean'; value: boolean }
  | { kind: 'nil' }
  | { kind: 'prototype'; proto: FunctionPrototype };

export type FunctionPrototype = {
  name: string;
  arity: number;
  upvalues: UpvalueDescriptor[];
  chunk: Chunk;
};

export type UpvalueDescriptor = {
  isLocal: boolean;  // true = capture from enclosing local, false = from enclosing upvalue
  index: number;
};
```

**Acceptance criteria:**
- [ ] All opcodes defined with clear semantics
- [ ] Chunk can be serialized/deserialized (for debugging)
- [ ] Line number mapping preserves source locations

---

### D2: Compiler (AST → Bytecode) (L)

**File:** `src/vm/compiler.ts`

```typescript
export type CompilerContext = {
  chunk: Chunk;
  locals: Local[];
  upvalues: UpvalueDescriptor[];
  scopeDepth: number;
  enclosing: CompilerContext | null;
  functionName: string;
};

export type Local = {
  name: string;
  depth: number;
  slot: number;
};

export function compile(expr: Expr): Chunk;
export function compileProgram(exprs: Expr[]): Chunk;
```

**Key compilation rules:**

| Source | Bytecode |
|--------|----------|
| Number literal | `PUSH_CONST [idx]` |
| Variable reference | `PUSH_LOCAL [slot]` or `PUSH_UPVALUE [idx]` |
| `(+ a b)` | compile(a), compile(b), `ADD` |
| `(if c t e)` | compile(c), `JUMP_IF_FALSE [else]`, compile(t), `JUMP [end]`, compile(e) |
| `(let ((x v)) body)` | compile(v), `SET_LOCAL [slot]`, compile(body) |
| `(lambda (x) body)` | Create FunctionPrototype, `CLOSURE [proto_idx]` |
| `(f arg1 arg2)` | compile(f), compile(arg1), compile(arg2), `CALL [2]` |
| `(f arg)` in tail position | compile(f), compile(arg), `TAIL_CALL [1]` |
| `(SomeCtor a b)` | compile(a), compile(b), `CONSTRUCT [ctor_idx, 2]` |
| `(match v ...)` | Series of `MATCH_TAG` + `JUMP_IF_FALSE` + body |
| `(perform op arg)` | compile(arg), `PERFORM [op_idx, 1]` |
| `(handle body ...)` | `PUSH_HANDLER [proto]`, compile(body), `POP_HANDLER` |

**Free variable analysis:**

```typescript
function resolveVariable(name: string, ctx: CompilerContext):
  | { kind: 'local'; slot: number }
  | { kind: 'upvalue'; index: number }
  | { kind: 'global'; name: string };
```

Walk up compiler contexts, marking upvalues as needed.

**Acceptance criteria:**
- [ ] All special forms compile correctly
- [ ] Tail position detection works for `if`, `let`, `begin`, `cond`, `match`
- [ ] Free variable analysis captures correct values
- [ ] Pattern matching compiles to efficient dispatch
- [ ] Source line tracking preserved through compilation

---

### D3: VM Core (L)

**File:** `src/vm/vm.ts`

```typescript
export type Closure = {
  kind: 'closure';
  proto: FunctionPrototype;
  upvalues: RuntimeValue[];  // Flat closure - values copied
};

export type CallFrame = {
  closure: Closure;
  ip: number;
  stackBase: number;
  // Debug info
  callSite?: { line: number; file?: string };
};

export type VM = {
  stack: RuntimeValue[];
  stackTop: number;
  frames: CallFrame[];
  frameCount: number;
  handlerStack: HandlerFrame[];
  // For debugging
  currentFile?: string;
};

export function createVM(): VM;
export function interpret(vm: VM, chunk: Chunk): RuntimeValue;
export function run(vm: VM): RuntimeValue;
```

**Main execution loop:**

```typescript
function run(vm: VM): RuntimeValue {
  let frame = vm.frames[vm.frameCount - 1];

  while (true) {
    const inst = frame.closure.proto.chunk.code[frame.ip++];

    switch (inst.op) {
      case 'PUSH_CONST':
        push(vm, frame.closure.proto.chunk.constants[inst.operand!]);
        break;

      case 'ADD': {
        const b = pop(vm);
        const a = pop(vm);
        push(vm, { kind: 'number', value: a.value + b.value });
        break;
      }

      case 'CALL': {
        const arity = inst.operand!;
        const callee = peek(vm, arity);
        if (callee.kind === 'closure') {
          // Push new frame
          const newFrame: CallFrame = {
            closure: callee,
            ip: 0,
            stackBase: vm.stackTop - arity,
            callSite: { line: getCurrentLine(frame), file: vm.currentFile }
          };
          vm.frames[vm.frameCount++] = newFrame;
          frame = newFrame;
        } else if (callee.kind === 'builtin') {
          // Call native
          const args = popN(vm, arity);
          pop(vm); // callee
          push(vm, callee.fn(args));
        }
        break;
      }

      case 'TAIL_CALL': {
        const arity = inst.operand!;
        // Reuse current frame instead of pushing new one
        const callee = peek(vm, arity);
        const args = popN(vm, arity);
        vm.stackTop = frame.stackBase;
        for (const arg of args) push(vm, arg);
        frame.closure = callee;
        frame.ip = 0;
        break;
      }

      case 'RETURN': {
        const result = pop(vm);
        vm.frameCount--;
        if (vm.frameCount === 0) return result;
        vm.stackTop = frame.stackBase;
        frame = vm.frames[vm.frameCount - 1];
        push(vm, result);
        break;
      }

      case 'PERFORM':
        return handlePerform(vm, frame, inst);

      case 'RESUME':
        return handleResume(vm, frame);

      // ... other cases
    }
  }
}
```

**Acceptance criteria:**
- [ ] All opcodes execute correctly
- [ ] TCO works (fib with accumulator doesn't overflow)
- [ ] Stack traces available on error
- [ ] Builtins callable from bytecode

---

### D4: Effect System (L)

**File:** `src/vm/effects.ts`

```typescript
export type HandlerFrame = {
  handlers: Map<string, HandlerClause>;
  returnHandler?: { paramSlot: number; bodyProto: FunctionPrototype };
  stackSnapshot: number;  // Stack top when handler installed
  frameSnapshot: number;  // Frame count when handler installed
};

export type HandlerClause = {
  paramSlots: number[];
  continuationSlot: number;
  bodyProto: FunctionPrototype;
};

export type VMContinuation = {
  kind: 'vm-continuation';
  frames: CallFrame[];           // Copied frames from perform to handler
  stack: RuntimeValue[];         // Copied stack segment
  handlerStack: HandlerFrame[];  // Handler state at capture
  resumeIp: number;              // Where to resume in innermost frame
};
```

**PERFORM implementation:**

```typescript
function handlePerform(vm: VM, frame: CallFrame, inst: Instruction): RuntimeValue {
  const opIdx = inst.operand!;
  const arity = inst.operand2!;
  const opName = getOperationName(opIdx);
  const args = popN(vm, arity);

  // Find handler
  for (let i = vm.handlerStack.length - 1; i >= 0; i--) {
    const hframe = vm.handlerStack[i];
    const clause = hframe.handlers.get(opName);
    if (clause) {
      // Capture continuation
      const continuation = captureContination(vm, hframe);

      // Set up handler environment
      setupHandlerCall(vm, clause, args, continuation);

      // Jump to handler
      return run(vm);
    }
  }

  throw new VMError(`Unhandled effect: ${opName}`, getStackTrace(vm));
}

function captureContinuation(vm: VM, hframe: HandlerFrame): VMContinuation {
  // Copy frames from current down to handler frame
  const frameStart = hframe.frameSnapshot;
  const copiedFrames = vm.frames.slice(frameStart, vm.frameCount).map(f => ({
    ...f,
    // Deep copy any mutable state
  }));

  // Copy stack segment
  const stackStart = hframe.stackSnapshot;
  const copiedStack = vm.stack.slice(stackStart, vm.stackTop);

  // Copy nested handlers
  const copiedHandlers = vm.handlerStack.slice(
    vm.handlerStack.indexOf(hframe) + 1
  );

  return {
    kind: 'vm-continuation',
    frames: copiedFrames,
    stack: copiedStack,
    handlerStack: copiedHandlers,
    resumeIp: vm.frames[vm.frameCount - 1].ip,
  };
}
```

**RESUME implementation:**

```typescript
function handleResume(vm: VM, frame: CallFrame): RuntimeValue {
  const value = pop(vm);
  const cont = pop(vm);

  if (cont.kind !== 'vm-continuation') {
    throw new VMError('resume expects a continuation');
  }

  // Restore frames
  for (const f of cont.frames) {
    vm.frames[vm.frameCount++] = { ...f };
  }

  // Restore stack
  for (const v of cont.stack) {
    push(vm, v);
  }

  // Push resume value
  push(vm, value);

  // Restore nested handlers
  for (const h of cont.handlerStack) {
    vm.handlerStack.push({ ...h });
  }

  // Continue from where we left off
  return run(vm);
}
```

**Acceptance criteria:**
- [ ] Simple perform/handle works
- [ ] Continuation can be resumed
- [ ] Continuation can be discarded (early return)
- [ ] Nested handlers work correctly
- [ ] Multiple resumes of same continuation fails (one-shot)
- [ ] Effects in handler body are handled by same frame (deep handlers)

---

### D5: Integration & Benchmarks (S)

**Files:**
- `index.ts` (CLI changes)
- `src/vm/index.ts` (public API)
- `bench/` (benchmark suite)

**CLI interface:**

```bash
# Run with VM
bun index.ts run file.mar --vm

# Run with tree-walker (default, unchanged)
bun index.ts run file.mar

# Disassemble bytecode (debugging)
bun index.ts disasm file.mar

# Benchmark comparison
bun index.ts bench file.mar
```

**Public API:**

```typescript
// src/vm/index.ts
export { compile, compileProgram } from './compiler';
export { createVM, interpret } from './vm';
export { disassemble } from './debug';
```

**Benchmark suite:**

```
bench/
  fib.mar          # (define (fib n) ...) - recursive
  fib-tail.mar     # Tail-recursive with accumulator
  sum-loop.mar     # Sum 1 to N
  effects.mar      # Many perform/handle cycles
  pattern.mar      # Heavy pattern matching
```

**Benchmark script:**

```typescript
// bench/run.ts
const files = ['fib', 'fib-tail', 'sum-loop', 'effects', 'pattern'];
for (const file of files) {
  const source = readFileSync(`bench/${file}.mar`);

  // Tree-walker
  const twStart = performance.now();
  runTreeWalker(source);
  const twTime = performance.now() - twStart;

  // VM
  const vmStart = performance.now();
  runVM(source);
  const vmTime = performance.now() - vmStart;

  console.log(`${file}: TW=${twTime}ms, VM=${vmTime}ms, speedup=${twTime/vmTime}x`);
}
```

**Acceptance criteria:**
- [ ] `--vm` flag works
- [ ] `disasm` command shows readable bytecode
- [ ] Benchmark shows measurable speedup on fib(30)
- [ ] All existing tests pass with `--vm`

---

## Debug Support

Given the priority on debugging, the VM includes:

### Stack Traces

```typescript
type VMError = {
  message: string;
  stackTrace: StackFrame[];
};

type StackFrame = {
  functionName: string;
  file?: string;
  line: number;
};

function getStackTrace(vm: VM): StackFrame[] {
  return vm.frames.slice(0, vm.frameCount).map(f => ({
    functionName: f.closure.proto.name,
    file: f.closure.proto.chunk.sourceFile,
    line: f.closure.proto.chunk.lines[f.ip - 1],
  }));
}
```

### Disassembler

```typescript
function disassemble(chunk: Chunk): string {
  let output = '';
  for (let i = 0; i < chunk.code.length; i++) {
    const inst = chunk.code[i];
    const line = chunk.lines[i];
    output += `${i.toString().padStart(4)} | ${line.toString().padStart(4)} | ${formatInstruction(inst)}\n`;
  }
  return output;
}
```

Example output:
```
   0 |    1 | PUSH_CONST    0    ; 35
   1 |    1 | PUSH_LOCAL    0    ; fib
   2 |    1 | CALL          1
   3 |    1 | RETURN
```

---

## File Structure

```
src/vm/
  index.ts          # Public exports
  bytecode.ts       # D1: Types, instruction set, VM value types (incl. FFI)
  compiler.ts       # D2: AST → Bytecode
  vm.ts             # D3: Execution engine + D4: Effect handling + default IO handlers
  debug.ts          # Disassembler, stack traces
  builtins.ts       # Builtin function table for VM (arithmetic, comparison, FFI, structs)
```

> **Note:** Effects (D4) were implemented directly in `vm.ts` rather than a separate `effects.ts` file.
> FFI builtins use lazy `require('bun:ffi')` so the VM module loads in non-FFI environments (web playground).

---

## Implementation Order

```
Week 1:
  D1 (bytecode.ts) ──► D2 (compiler.ts, no effects/closures)
                            │
                            ▼
                       D3 (vm.ts, basic ops)
                            │
                            ▼
                       Test: fib works

Week 2:
  D2 (add closures) ──► D3 (CLOSURE, upvalues)
                            │
                            ▼
                       D2 (add pattern match) ──► D3 (MATCH_TAG, etc)
                            │
                            ▼
                       Test: ADTs + match work

Week 3:
  D4 (effects.ts) ──► D2 (handle/perform compilation)
                            │
                            ▼
                       Test: effects work
                            │
                            ▼
                       D5 (integration, benchmarks)
```

---

## Risks & Mitigations

| Risk | Likelihood | Mitigation |
|------|------------|------------|
| Continuation capture bugs | High | Extensive test suite, compare with tree-walker output |
| Stack trace quality degrades | Medium | Test error paths explicitly, preserve all source info |
| Performance worse than tree-walker | Low | Benchmark continuously, profile hotspots |
| Semantic drift from tree-walker | Medium | Run both interpreters on test suite, diff outputs |

---

## Test Strategy

1. **Unit tests**: Each opcode in isolation
2. **Integration tests**: Run existing `spec/` tests with `--vm` flag
3. **Comparison tests**: Run same program on both interpreters, compare results
4. **Effect tests**: Specific tests for continuation capture/resume edge cases
5. **Benchmark tests**: Ensure no regression on key benchmarks

---

## Non-Goals (Explicit)

- JIT compilation (future work)
- Register-based VM (optimize later if needed)
- Bytecode serialization to disk (not needed for MVP)
- Debugging protocol (step through, breakpoints) - future work
- Optimizations (constant folding, dead code elimination) - future work

---

## Success Criteria

1. `fib(30)` runs at least 5x faster than tree-walker
2. All existing tests pass with `--vm` flag
3. Stack traces show correct file/line on errors
4. Effects work identically to tree-walker
5. `disasm` output is human-readable
