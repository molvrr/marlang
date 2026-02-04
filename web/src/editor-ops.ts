/**
 * Structural Edit Operations
 *
 * All operations are immutable - they return new AST arrays.
 * Operations always produce valid syntax.
 */

import type { Expr, LispValue } from '@marlang/types';
import type { NodePath } from './ASTRenderer';

// ═══════════════════════════════════════════════════════════════
// AST Construction Helpers
// ═══════════════════════════════════════════════════════════════

export const nil: Expr = { kind: 'nil' };

export function atom(value: LispValue): Expr {
  return { kind: 'atom', value };
}

export function symbol(name: string): Expr {
  return atom({ kind: 'symbol', value: name });
}

export function number(value: number, isFloat = false): Expr {
  return atom({ kind: 'number', value, isFloat });
}

export function string(value: string): Expr {
  return atom({ kind: 'string', value });
}

// Build a list from an array of expressions (creates cons cells)
export function list(...items: Expr[]): Expr {
  let result: Expr = nil;
  for (let i = items.length - 1; i >= 0; i--) {
    result = { kind: 'list', head: items[i], tail: result };
  }
  return result;
}

// Convert cons-list to array
export function listToArray(expr: Expr): Expr[] {
  const items: Expr[] = [];
  let current = expr;
  while (current.kind === 'list') {
    items.push(current.head);
    current = current.tail;
  }
  return items;
}

// ═══════════════════════════════════════════════════════════════
// Path Operations
// ═══════════════════════════════════════════════════════════════

// Get node at path in AST
export function getNodeAtPath(exprs: Expr[], path: NodePath): Expr | null {
  if (path.length === 0) return null;

  let current: Expr | null = exprs[path[0]] ?? null;

  for (let i = 1; i < path.length && current !== null; i++) {
    if (current.kind !== 'list') return null;
    const items = listToArray(current);
    current = items[path[i]] ?? null;
  }

  return current;
}

// Get parent path (drop last element)
export function getParentPath(path: NodePath): NodePath | null {
  if (path.length <= 1) return null;
  return path.slice(0, -1);
}

// ═══════════════════════════════════════════════════════════════
// Immutable AST Transformation
// ═══════════════════════════════════════════════════════════════

type Transform = (expr: Expr) => Expr | null; // null = delete

// Apply a transformation at a specific path
export function transformAtPath(
  exprs: Expr[],
  path: NodePath,
  transform: Transform
): { exprs: Expr[]; deleted?: boolean } {
  if (path.length === 0) {
    return { exprs };
  }

  const [topIndex, ...rest] = path;

  if (topIndex < 0 || topIndex >= exprs.length) {
    return { exprs };
  }

  if (rest.length === 0) {
    // Transform top-level expression
    const result = transform(exprs[topIndex]);
    if (result === null) {
      // Delete top-level expression
      return {
        exprs: [...exprs.slice(0, topIndex), ...exprs.slice(topIndex + 1)],
        deleted: true,
      };
    }
    return {
      exprs: [...exprs.slice(0, topIndex), result, ...exprs.slice(topIndex + 1)],
    };
  }

  // Recursively transform within a top-level expression
  const transformed = transformInExpr(exprs[topIndex], rest, transform);
  return {
    exprs: [...exprs.slice(0, topIndex), transformed.expr, ...exprs.slice(topIndex + 1)],
    deleted: transformed.deleted,
  };
}

function transformInExpr(
  expr: Expr,
  path: NodePath,
  transform: Transform
): { expr: Expr; deleted?: boolean } {
  if (path.length === 0) {
    const result = transform(expr);
    return { expr: result ?? nil, deleted: result === null };
  }

  if (expr.kind !== 'list') {
    return { expr };
  }

  const [index, ...rest] = path;
  const items = listToArray(expr);

  if (index < 0 || index >= items.length) {
    return { expr };
  }

  if (rest.length === 0) {
    // Transform the item at index
    const result = transform(items[index]);
    if (result === null) {
      // Delete the item
      const newItems = [...items.slice(0, index), ...items.slice(index + 1)];
      return { expr: list(...newItems), deleted: true };
    }
    const newItems = [...items.slice(0, index), result, ...items.slice(index + 1)];
    return { expr: list(...newItems) };
  }

  // Recurse deeper
  const transformed = transformInExpr(items[index], rest, transform);
  const newItems = [...items.slice(0, index), transformed.expr, ...items.slice(index + 1)];
  return { expr: list(...newItems), deleted: transformed.deleted };
}

// ═══════════════════════════════════════════════════════════════
// Structural Edit Operations
// ═══════════════════════════════════════════════════════════════

export type EditResult = {
  exprs: Expr[];
  cursor: NodePath;
};

/**
 * Delete the node at the given path
 */
export function deleteNode(exprs: Expr[], path: NodePath): EditResult {
  if (path.length === 0) {
    return { exprs, cursor: path };
  }

  const result = transformAtPath(exprs, path, () => null);

  // Determine new cursor position
  let newCursor: NodePath;
  const lastIndex = path[path.length - 1];

  if (path.length === 1) {
    // Deleted top-level: go to previous sibling, or next, or empty
    if (result.exprs.length === 0) {
      newCursor = [];
    } else if (lastIndex > 0) {
      newCursor = [lastIndex - 1];
    } else {
      newCursor = [0];
    }
  } else {
    // Deleted within a list: go to previous sibling, or parent
    const parentPath = path.slice(0, -1);
    const parent = getNodeAtPath(result.exprs, parentPath);

    if (parent && parent.kind === 'list') {
      const siblings = listToArray(parent);
      if (siblings.length === 0) {
        // List is now empty, go to parent
        newCursor = parentPath;
      } else if (lastIndex > 0) {
        newCursor = [...parentPath, lastIndex - 1];
      } else {
        newCursor = [...parentPath, 0];
      }
    } else {
      newCursor = parentPath;
    }
  }

  return { exprs: result.exprs, cursor: newCursor };
}

/**
 * Insert a new node after the selected node
 */
export function insertAfter(
  exprs: Expr[],
  path: NodePath,
  newNode: Expr
): EditResult {
  if (path.length === 0) {
    // No selection, insert at end
    return { exprs: [...exprs, newNode], cursor: [exprs.length] };
  }

  const lastIndex = path[path.length - 1];

  if (path.length === 1) {
    // Insert at top level
    const newExprs = [
      ...exprs.slice(0, lastIndex + 1),
      newNode,
      ...exprs.slice(lastIndex + 1),
    ];
    return { exprs: newExprs, cursor: [lastIndex + 1] };
  }

  // Insert within a list
  const parentPath = path.slice(0, -1);
  const result = transformAtPath(exprs, parentPath, (parent) => {
    if (parent.kind !== 'list') return parent;
    const items = listToArray(parent);
    const newItems = [
      ...items.slice(0, lastIndex + 1),
      newNode,
      ...items.slice(lastIndex + 1),
    ];
    return list(...newItems);
  });

  return { exprs: result.exprs, cursor: [...parentPath, lastIndex + 1] };
}

/**
 * Insert a new node before the selected node
 */
export function insertBefore(
  exprs: Expr[],
  path: NodePath,
  newNode: Expr
): EditResult {
  if (path.length === 0) {
    // No selection, insert at start
    return { exprs: [newNode, ...exprs], cursor: [0] };
  }

  const lastIndex = path[path.length - 1];

  if (path.length === 1) {
    // Insert at top level
    const newExprs = [
      ...exprs.slice(0, lastIndex),
      newNode,
      ...exprs.slice(lastIndex),
    ];
    return { exprs: newExprs, cursor: [lastIndex] };
  }

  // Insert within a list
  const parentPath = path.slice(0, -1);
  const result = transformAtPath(exprs, parentPath, (parent) => {
    if (parent.kind !== 'list') return parent;
    const items = listToArray(parent);
    const newItems = [
      ...items.slice(0, lastIndex),
      newNode,
      ...items.slice(lastIndex),
    ];
    return list(...newItems);
  });

  return { exprs: result.exprs, cursor: [...parentPath, lastIndex] };
}

/**
 * Wrap the selected node in a new list (selection becomes first child)
 */
export function wrapInList(exprs: Expr[], path: NodePath): EditResult {
  if (path.length === 0) {
    return { exprs, cursor: path };
  }

  const result = transformAtPath(exprs, path, (node) => {
    return list(node);
  });

  // Cursor goes to the first (and only) child of the new list
  return { exprs: result.exprs, cursor: [...path, 0] };
}

/**
 * Wrap with a head symbol: (sym selection)
 */
export function wrapWithSymbol(
  exprs: Expr[],
  path: NodePath,
  sym: string
): EditResult {
  if (path.length === 0) {
    return { exprs, cursor: path };
  }

  const result = transformAtPath(exprs, path, (node) => {
    return list(symbol(sym), node);
  });

  // Cursor goes to the wrapped expression (second child)
  return { exprs: result.exprs, cursor: [...path, 1] };
}

/**
 * Unwrap/Splice: Replace a list with its contents
 * If the list has one child, that child replaces the list.
 * If multiple children, they splice into the parent.
 */
export function unwrap(exprs: Expr[], path: NodePath): EditResult {
  if (path.length === 0) {
    return { exprs, cursor: path };
  }

  const node = getNodeAtPath(exprs, path);
  if (!node) {
    return { exprs, cursor: path };
  }

  // Handle nil (empty list) - delete it
  if (node.kind === 'nil') {
    return deleteNode(exprs, path);
  }

  // Can only unwrap lists
  if (node.kind !== 'list') {
    return { exprs, cursor: path };
  }

  const children = listToArray(node);
  if (children.length === 0) {
    // Empty list - just delete it
    return deleteNode(exprs, path);
  }

  if (children.length === 1) {
    // Single child - replace list with child
    const result = transformAtPath(exprs, path, () => children[0]);
    return { exprs: result.exprs, cursor: path };
  }

  // Multiple children - splice into parent
  const lastIndex = path[path.length - 1];

  if (path.length === 1) {
    // Splicing at top level
    const newExprs = [
      ...exprs.slice(0, lastIndex),
      ...children,
      ...exprs.slice(lastIndex + 1),
    ];
    return { exprs: newExprs, cursor: [lastIndex] };
  }

  // Splicing within a list
  const parentPath = path.slice(0, -1);
  const result = transformAtPath(exprs, parentPath, (parent) => {
    if (parent.kind !== 'list') return parent;
    const items = listToArray(parent);
    const newItems = [
      ...items.slice(0, lastIndex),
      ...children,
      ...items.slice(lastIndex + 1),
    ];
    return list(...newItems);
  });

  return { exprs: result.exprs, cursor: [...parentPath, lastIndex] };
}

/**
 * Raise: Replace parent with the selected node
 */
export function raise(exprs: Expr[], path: NodePath): EditResult {
  if (path.length <= 1) {
    // Can't raise a top-level expression (no parent to replace)
    return { exprs, cursor: path };
  }

  const node = getNodeAtPath(exprs, path);
  if (!node) {
    return { exprs, cursor: path };
  }

  const parentPath = path.slice(0, -1);
  const result = transformAtPath(exprs, parentPath, () => node);

  return { exprs: result.exprs, cursor: parentPath };
}

/**
 * Replace the node at path with a new node
 */
export function replaceNode(
  exprs: Expr[],
  path: NodePath,
  newNode: Expr
): EditResult {
  if (path.length === 0) {
    return { exprs, cursor: path };
  }

  const result = transformAtPath(exprs, path, () => newNode);
  return { exprs: result.exprs, cursor: path };
}

/**
 * Add a child to a list (at the end)
 */
export function appendChild(
  exprs: Expr[],
  path: NodePath,
  child: Expr
): EditResult {
  if (path.length === 0) {
    // Add new top-level expression
    return { exprs: [...exprs, child], cursor: [exprs.length] };
  }

  const node = getNodeAtPath(exprs, path);
  if (!node) {
    return { exprs, cursor: path };
  }

  // Handle nil (empty list) - convert to list with child
  if (node.kind === 'nil') {
    const result = transformAtPath(exprs, path, () => list(child));
    return { exprs: result.exprs, cursor: [...path, 0] };
  }

  if (node.kind !== 'list') {
    return { exprs, cursor: path };
  }

  const children = listToArray(node);
  const result = transformAtPath(exprs, path, () => list(...children, child));

  return { exprs: result.exprs, cursor: [...path, children.length] };
}

/**
 * Add a child to a list (at the beginning)
 */
export function prependChild(
  exprs: Expr[],
  path: NodePath,
  child: Expr
): EditResult {
  if (path.length === 0) {
    // Add new top-level expression at start
    return { exprs: [child, ...exprs], cursor: [0] };
  }

  const node = getNodeAtPath(exprs, path);
  if (!node) {
    return { exprs, cursor: path };
  }

  // Handle nil (empty list) - convert to list with child
  if (node.kind === 'nil') {
    const result = transformAtPath(exprs, path, () => list(child));
    return { exprs: result.exprs, cursor: [...path, 0] };
  }

  if (node.kind !== 'list') {
    return { exprs, cursor: path };
  }

  const children = listToArray(node);
  const result = transformAtPath(exprs, path, () => list(child, ...children));

  return { exprs: result.exprs, cursor: [...path, 0] };
}

/**
 * Create an empty list at cursor position
 */
export function insertEmptyList(exprs: Expr[], path: NodePath): EditResult {
  return insertAfter(exprs, path, list());
}

// ═══════════════════════════════════════════════════════════════
// AST to Text Conversion
// ═══════════════════════════════════════════════════════════════

export function exprToString(expr: Expr): string {
  switch (expr.kind) {
    case 'nil':
      return '()';
    case 'atom':
      return atomToString(expr.value);
    case 'list': {
      const items = listToArray(expr);
      if (items.length === 0) return '()';
      return '(' + items.map(exprToString).join(' ') + ')';
    }
  }
}

function atomToString(value: LispValue): string {
  switch (value.kind) {
    case 'symbol':
      return value.value;
    case 'number':
      return String(value.value);
    case 'string':
      return JSON.stringify(value.value);
  }
}

export function exprsToString(exprs: Expr[]): string {
  return exprs.map(exprToString).join('\n\n');
}
