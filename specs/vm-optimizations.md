# VM Performance Optimizations Spec

## Tier 1 — High Impact

### 1. ~~NaN-Boxing / Flat Value Encoding~~ Unboxed Numbers (DONE)
**Location:** `src/vm/bytecode.ts`, `src/vm/vm.ts`, `src/vm/builtins.ts`

**Problem:** Every VMValue is a heap-allocated JS object (`{kind: 'number', value: 42}`). Every arithmetic op, comparison, and constant load allocates a new object. For `fib(40)` (~331M operations), this means ~331M short-lived objects causing massive GC pressure.

**Implemented:** `VMValue = number | VMObject` — raw JS numbers avoid heap allocation. JS engines (V8, JSC/Bun) optimize `typeof x === 'number'` into a single type-tag check, giving us the same benefit as NaN-boxing without DataView overhead. Non-number types remain tagged objects (`VMObject`).

**Learnings:**
- Simpler than full NaN-boxing and still eliminates all number allocation
- `typeof` fast-path must come first in every function that touches VMValue (isTruthy, valuesEqual, formatVMValue, toRuntimeValue)
- Every `.kind` access on VMValue must be guarded with `typeof v !== 'number'` first
- `toRuntimeValue()` re-boxes numbers to `{kind:'number', value:n}` so existing tests and tree-walker interop continue working unchanged
- All 438 tests pass with zero changes to test code

---

### 2. Flat Instruction Encoding
**Location:** `src/vm/bytecode.ts`, `src/vm/vm.ts`, `src/vm/compiler.ts`, `src/vm/debug.ts`

**Problem:** Each instruction is a JS object `{op, operand?, operand2?, line}`. The hot loop dereferences `frame.closure.proto.chunk.code[frame.ip++]` (4-level property chain) per instruction. Optional fields (`operand?`, `operand2?`) cause polymorphic inline caches in V8.

**Solution:**
- Use flat typed arrays: `Uint8Array` for opcodes, `Int32Array` for operands
- Each instruction encoded as a fixed-width entry (e.g., 3 ints: op, operand, operand2)
- Move `line` info to a separate run-length-encoded side table (only needed for error reporting)

**Impact:** Eliminates per-instruction object overhead, improves cache locality. Estimated 1.5-3x speedup.

---

### 3. Pre-Convert Constant Pool (DONE)
**Location:** `src/vm/bytecode.ts`, `src/vm/vm.ts`

**Problem:** Every `PUSH_CONST` creates a new VMValue object by copying from the ConstantValue in the constant pool.

**Implemented:** Added `preconvertConstants()` in bytecode.ts and `vmConstants?: VMValue[]` field on `Chunk`. `ensureVmConstants()` recursively pre-converts all chunks (including nested prototypes) before execution. `PUSH_CONST` is now a single array index: `push(vm, chunk.vmConstants![inst.operand!])`.

**Learnings:**
- Must recurse through prototype constants to pre-convert nested function chunks
- Prototype slots in vmConstants get NIL placeholders — prototypes are only accessed via the original `constants[]` array (for CLOSURE, PUSH_HANDLER)
- Bundled naturally with unboxed numbers (#1) since both change how values flow through PUSH_CONST

---

### 4. Use TRUE/FALSE Singletons (DONE)
**Location:** `src/vm/vm.ts`, `src/vm/builtins.ts`, `src/vm/bytecode.ts`

**Problem:** Every comparison allocated a new boolean object despite TRUE/FALSE singletons existing.

**Implemented:** All comparison opcodes (EQ, LT, GT, LE, GE, NOT) now push `TRUE`/`FALSE` singletons. Builtin comparison functions also return singletons. Boolean globals (`#t`, `#f`) use the shared singletons. `NIL`/`TRUE`/`FALSE` typed as `VMObject` (not `VMValue`) for type safety.

**Learnings:**
- Typed singletons as `VMObject` (not `VMValue`) so they're clearly non-number objects
- Also applied to builtin comparison functions, not just opcodes — easy to miss those
- `preconvertConstants` maps boolean constants to the singletons too

---

### 5. Reuse Locals Array on Self-Tail-Call
**Location:** `src/vm/vm.ts` line 472 (`TAIL_CALL` handler)

**Problem:** Even tail calls allocate `new Array(localCount).fill(NIL)`:
```typescript
const locals = new Array(callee.proto.chunk.localCount).fill(NIL);
```
For self-recursive tail calls (same function, same arity), the existing locals array has the right size.

**Solution:** Detect self-tail-call (same closure proto) and reuse `frame.locals`:
```typescript
if (callee.proto === frame.closure.proto) {
  // Overwrite argument slots, fill rest with NIL
  for (let i = 0; i < arity; i++) frame.locals[i] = args[i];
  for (let i = arity; i < frame.locals.length; i++) frame.locals[i] = NIL;
} else {
  frame.locals = new Array(callee.proto.chunk.localCount).fill(NIL);
}
```

**Impact:** Eliminates array allocation on every tail call iteration. Major for tail-recursive benchmarks.

---

## Tier 2 — Medium Impact

### 6. Cache code/constants Locally in run()
**Location:** `src/vm/vm.ts` `run()` function

**Problem:** Every instruction fetch traverses `frame.closure.proto.chunk.code[frame.ip++]` — a 4-level property chain. `frame` changes on CALL/RETURN which invalidates V8's cached lookups.

**Solution:**
```typescript
let code = frame.closure.proto.chunk.code;
let constants = frame.closure.proto.chunk.constants;
// Update on CALL, TAIL_CALL, RETURN:
// code = frame.closure.proto.chunk.code; constants = ...;
```

**Impact:** Reduces per-instruction overhead. Modest but consistent improvement.

---

### 7. Dense Opcode Numbering
**Location:** `src/vm/bytecode.ts` `OpCode` enum

**Problem:** Opcodes have gaps (0-7, 10-14, 20-25, 30-33, 40-42, 50-52, 60-63, 99). V8 may generate a lookup table instead of a dense jump table for the switch statement.

**Solution:** Renumber all opcodes to 0 through N-1 (contiguous).

**Impact:** Enables V8 to generate an efficient jump table for the main dispatch switch.

---

### 8. Numeric Type Tags Instead of Strings (PARTIALLY SUPERSEDED)
**Location:** `src/vm/bytecode.ts` (VMValue type), `src/vm/vm.ts` (all kind checks)

**Problem:** Every `.kind` check is a string comparison (`value.kind === 'number'`). With 10 different VMValue shapes, V8's inline caches become megamorphic.

**Status:** The most critical case (numbers) is now handled by `typeof x === 'number'` which is faster than any string or numeric tag check. Remaining object types still use string `.kind` tags. Could still apply numeric tags to `VMObject` variants, but diminishing returns since the hot path (numbers) is already optimized.

---

### 9. ~~Reorder valuesEqual for Common Case~~ (DONE — via #1)
**Location:** `src/vm/vm.ts` `valuesEqual()` function

**Implemented:** Numbers are now checked first via `typeof a === 'number'` fast path, which is even faster than the proposed `.kind` reorder. Falls through to object comparisons only for non-numbers.

---

### 10. Pre-Allocate Frames Array
**Location:** `src/vm/vm.ts` `createVM()`

**Problem:** `frames: []` grows dynamically. The stack is pre-allocated to 256 but frames are not.

**Solution:**
```typescript
frames: new Array(64), // pre-allocate like stack
```

**Impact:** Avoids dynamic array growth during deep recursion.

---

### 11. Constant Folding in Compiler
**Location:** `src/vm/compiler.ts`

**Problem:** `(+ 1 2)` emits 3 instructions (`PUSH_CONST 1`, `PUSH_CONST 2`, `ADD`) instead of 1 (`PUSH_CONST 3`). Constants are never folded at compile time.

**Solution:** In `compileExpr`, when compiling binary ops where both operands are literals, evaluate at compile time and emit a single `PUSH_CONST` with the result.

**Impact:** Reduces instruction count for constant expressions. Most beneficial when constants appear in loops.

---

## Tier 3 — Architectural

### 12. Cache Parsed Handler Descriptors
**Location:** `src/vm/vm.ts` `PUSH_HANDLER` handler

**Problem:** Every `PUSH_HANDLER` calls `JSON.parse()` on the handler descriptor string. For the effects benchmark (1000 iterations), this is 1000 `JSON.parse` calls.

**Solution:** Cache parsed descriptors in a `Map<string, ParsedDescriptor>` keyed by the raw string, or pre-parse at compile time and store as a structured constant.

**Impact:** Eliminates JSON.parse overhead in effect-heavy code.

---

### 13. SELF_CALL Opcode
**Location:** `src/vm/compiler.ts`, `src/vm/vm.ts`

**Problem:** Recursive calls use `PUSH_GLOBAL` (Map.get lookup) every iteration to find the function being called, even for direct self-recursion.

**Solution:** Add a `SELF_CALL` opcode that reuses the current frame's closure as the callee, skipping the global lookup entirely.

**Impact:** Eliminates Map.get per recursive call. Significant for deep recursion (fib, factorial).

---

### 14. MAKE_LIST Opcode
**Location:** `src/vm/compiler.ts`, `src/vm/vm.ts`

**Problem:** `(list 1 2 3)` is compiled as a regular function call through the builtin dispatch path (PUSH_GLOBAL "list", args, CALL).

**Solution:** Add a `MAKE_LIST n` opcode that pops n values and creates a list directly, bypassing builtin dispatch overhead.

**Impact:** Faster list construction, avoids args array allocation in builtin path.

---

### 15. Frame Pool / Reusable CallFrame Objects
**Location:** `src/vm/vm.ts`

**Problem:** Every function call allocates a new `CallFrame` object and a new locals array. For `fib(40)` (~331M calls), this is ~331M frame objects.

**Solution:** Maintain a pool of CallFrame objects. On CALL, take from pool; on RETURN, return to pool. Reset fields instead of allocating.

**Impact:** Reduces GC pressure for deep recursion.

---

### 16. Non-Exception Effect Dispatch
**Location:** `src/vm/vm.ts`

**Problem:** `PERFORM` throws `EffectSignal` (a JS exception), caught by `runWithHandler`. V8 deoptimizes code containing try/catch, and exception creation captures stack traces.

**Solution:** Use a trampoline/signal-return approach instead of exceptions:
- `PERFORM` sets a signal flag and returns from `run()`
- The caller (`runWithHandler`) checks the flag and dispatches to the handler
- Avoids V8 deoptimization and stack trace capture

**Impact:** Significant for effect-heavy code. Avoids V8 try/catch deoptimization penalty.

---

### 17. O(1) List Tail via Cons-Cells
**Location:** `src/vm/bytecode.ts`, `src/vm/vm.ts`

**Problem:** `GET_FIELD` with fieldIdx=1 (list tail) calls `elements.slice(1)`, which copies the entire array minus the first element — O(n) per tail access.

**Solution:** Represent lists as cons-cells: `{kind: 'cons', head: VMValue, tail: VMValue}` with `{kind: 'nil'}` as empty list. Tail access becomes O(1) pointer dereference.

**Trade-off:** Loses O(1) random access. Need to profile whether list-heavy code benefits more from O(1) tail or O(1) index.

**Impact:** Critical for recursive list processing patterns.
