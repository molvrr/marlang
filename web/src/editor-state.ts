/**
 * Editor State Management
 *
 * Manages the AST, cursor position, edit mode, and undo/redo history.
 */

import { createSignal, createMemo, batch } from 'solid-js';
import type { Expr } from '@marlang/types';
import type { NodePath } from './ASTRenderer';
import * as Ops from './editor-ops';
import { getNodeAtPath } from './editor-ops';

// ═══════════════════════════════════════════════════════════════
// Types
// ═══════════════════════════════════════════════════════════════

export type EditorMode = 'normal' | 'insert';

export type InsertTarget =
  | { kind: 'after' }       // Insert after current selection
  | { kind: 'before' }      // Insert before current selection
  | { kind: 'child' }       // Append as last child of current list
  | { kind: 'first-child' } // Insert as first child of current list
  | { kind: 'replace' };    // Replace current selection

type HistoryEntry = {
  exprs: Expr[];
  cursor: NodePath;
};

const MAX_HISTORY = 100;

// ═══════════════════════════════════════════════════════════════
// Editor State Hook
// ═══════════════════════════════════════════════════════════════

export function createEditorState(initialExprs: Expr[] = []) {
  // Core state
  const [exprs, setExprs] = createSignal<Expr[]>(initialExprs);
  const [cursor, setCursor] = createSignal<NodePath>([]);
  const [mode, setMode] = createSignal<EditorMode>('normal');
  const [insertTarget, setInsertTarget] = createSignal<InsertTarget>({ kind: 'after' });
  const [inputBuffer, setInputBuffer] = createSignal('');

  // Undo/redo stacks
  const [undoStack, setUndoStack] = createSignal<HistoryEntry[]>([]);
  const [redoStack, setRedoStack] = createSignal<HistoryEntry[]>([]);

  // Clipboard (internal, for AST subtrees)
  const [clipboard, setClipboard] = createSignal<Expr | null>(null);

  // Derived state
  const selectedNode = createMemo(() => {
    const path = cursor();
    if (path.length === 0) return null;
    return Ops.getNodeAtPath(exprs(), path);
  });

  const canUndo = createMemo(() => undoStack().length > 0);
  const canRedo = createMemo(() => redoStack().length > 0);

  // Push current state to undo stack
  function pushUndo() {
    const entry: HistoryEntry = {
      exprs: exprs(),
      cursor: cursor(),
    };
    setUndoStack((stack) => {
      const newStack = [...stack, entry];
      if (newStack.length > MAX_HISTORY) {
        return newStack.slice(newStack.length - MAX_HISTORY);
      }
      return newStack;
    });
    setRedoStack([]);
  }

  // Apply an edit operation with undo support
  function applyEdit(result: Ops.EditResult) {
    pushUndo();
    batch(() => {
      setExprs(result.exprs);
      setCursor(result.cursor);
    });
  }

  // ─────────────────────────────────────────────────────────────
  // Undo/Redo
  // ─────────────────────────────────────────────────────────────

  function undo() {
    const stack = undoStack();
    if (stack.length === 0) return;

    const entry = stack[stack.length - 1];

    // Push current to redo
    setRedoStack((s) => [
      ...s,
      { exprs: exprs(), cursor: cursor() },
    ]);

    // Restore from undo
    batch(() => {
      setUndoStack(stack.slice(0, -1));
      setExprs(entry.exprs);
      setCursor(entry.cursor);
    });
  }

  function redo() {
    const stack = redoStack();
    if (stack.length === 0) return;

    const entry = stack[stack.length - 1];

    // Push current to undo
    setUndoStack((s) => [
      ...s,
      { exprs: exprs(), cursor: cursor() },
    ]);

    // Restore from redo
    batch(() => {
      setRedoStack(stack.slice(0, -1));
      setExprs(entry.exprs);
      setCursor(entry.cursor);
    });
  }

  // ─────────────────────────────────────────────────────────────
  // Mode Transitions
  // ─────────────────────────────────────────────────────────────

  function enterInsertMode(target: InsertTarget) {
    batch(() => {
      setMode('insert');
      setInsertTarget(target);
      setInputBuffer('');
    });
  }

  function exitInsertMode() {
    setMode('normal');
    setInputBuffer('');
  }

  function commitInsert() {
    const buffer = inputBuffer().trim();
    if (!buffer) {
      exitInsertMode();
      return;
    }

    const node = parseInputBuffer(buffer);
    const target = insertTarget();
    const path = cursor();

    let result: Ops.EditResult;

    switch (target.kind) {
      case 'after':
        result = Ops.insertAfter(exprs(), path, node);
        break;
      case 'before':
        result = Ops.insertBefore(exprs(), path, node);
        break;
      case 'child':
        result = Ops.appendChild(exprs(), path, node);
        break;
      case 'first-child':
        result = Ops.prependChild(exprs(), path, node);
        break;
      case 'replace':
        result = Ops.replaceNode(exprs(), path, node);
        break;
    }

    applyEdit(result);
    exitInsertMode();
  }

  // ─────────────────────────────────────────────────────────────
  // Structural Operations
  // ─────────────────────────────────────────────────────────────

  function deleteSelected() {
    if (cursor().length === 0) return;
    applyEdit(Ops.deleteNode(exprs(), cursor()));
  }

  function wrapSelected() {
    if (cursor().length === 0) return;
    applyEdit(Ops.wrapInList(exprs(), cursor()));
  }

  function wrapWithSymbol(sym: string) {
    if (cursor().length === 0) return;
    applyEdit(Ops.wrapWithSymbol(exprs(), cursor(), sym));
  }

  function unwrapSelected() {
    applyEdit(Ops.unwrap(exprs(), cursor()));
  }

  function raiseSelected() {
    applyEdit(Ops.raise(exprs(), cursor()));
  }

  function insertEmptyList() {
    applyEdit(Ops.insertEmptyList(exprs(), cursor()));
  }

  // ─────────────────────────────────────────────────────────────
  // Clipboard Operations
  // ─────────────────────────────────────────────────────────────

  function copy() {
    const path = cursor();
    if (path.length === 0) return;
    const node = getNodeAtPath(exprs(), path);
    if (node) {
      setClipboard(node);
    }
  }

  function cut() {
    copy();
    deleteSelected();
  }

  function pasteAfter() {
    const node = clipboard();
    if (!node) return;
    applyEdit(Ops.insertAfter(exprs(), cursor(), node));
  }

  function pasteBefore() {
    const node = clipboard();
    if (!node) return;
    applyEdit(Ops.insertBefore(exprs(), cursor(), node));
  }

  function pasteReplace() {
    const node = clipboard();
    if (!node || cursor().length === 0) return;
    applyEdit(Ops.replaceNode(exprs(), cursor(), node));
  }

  // ─────────────────────────────────────────────────────────────
  // Input Handling
  // ─────────────────────────────────────────────────────────────

  function handleInsertInput(char: string) {
    setInputBuffer((b) => b + char);
  }

  function handleInsertBackspace() {
    setInputBuffer((b) => b.slice(0, -1));
  }

  // ─────────────────────────────────────────────────────────────
  // Load/Export
  // ─────────────────────────────────────────────────────────────

  function loadExprs(newExprs: Expr[]) {
    batch(() => {
      setExprs(newExprs);
      setCursor(newExprs.length > 0 ? [0] : []);
      setUndoStack([]);
      setRedoStack([]);
      setMode('normal');
    });
  }

  function toText(): string {
    return Ops.exprsToString(exprs());
  }

  return {
    // State
    exprs,
    cursor,
    mode,
    inputBuffer,
    selectedNode,
    insertTarget,

    // Undo/Redo
    canUndo,
    canRedo,
    undo,
    redo,

    // Cursor
    setCursor,

    // Mode
    enterInsertMode,
    exitInsertMode,
    commitInsert,

    // Operations
    deleteSelected,
    wrapSelected,
    wrapWithSymbol,
    unwrapSelected,
    raiseSelected,
    insertEmptyList,

    // Clipboard
    clipboard,
    copy,
    cut,
    pasteAfter,
    pasteBefore,
    pasteReplace,

    // Input
    handleInsertInput,
    handleInsertBackspace,

    // Load/Export
    loadExprs,
    toText,
  };
}

// ═══════════════════════════════════════════════════════════════
// Input Parsing
// ═══════════════════════════════════════════════════════════════

function parseInputBuffer(input: string): Expr {
  // Try to parse as number
  const num = parseFloat(input);
  if (!isNaN(num) && isFinite(num)) {
    const isFloat = input.includes('.') || input.includes('e') || input.includes('E');
    return Ops.number(num, isFloat);
  }

  // Check for string literal
  if (input.startsWith('"') && input.endsWith('"')) {
    try {
      const value = JSON.parse(input);
      return Ops.string(value);
    } catch {
      // Not valid JSON string, treat as symbol
    }
  }

  // Default to symbol
  return Ops.symbol(input);
}

export type EditorState = ReturnType<typeof createEditorState>;
