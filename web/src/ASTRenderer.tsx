import { createSignal, createMemo, For, Show, JSX } from 'solid-js';
import type { Expr, LispValue } from '@marlang/types';

// Path identifies a node in the AST (e.g., [0, 1, 2] = first expr, second child, third child)
export type NodePath = number[];

export type ASTRendererProps = {
  exprs: Expr[];
  selectedPath: NodePath | null;
  onSelect: (path: NodePath) => void;
  onDeselect: () => void;
  collapsedPaths: Set<string>;
  onToggleCollapse: (path: NodePath) => void;
  getTypeForPath?: (path: NodePath) => string | null;
  errorPath: NodePath | null;
};

// Special forms that get distinct rendering
const SPECIAL_FORMS = new Set([
  'define', 'lambda', 'if', 'let', 'cond', 'begin',
  'match', 'data', 'class', 'instance', 'type',
  'effect', 'perform', 'handle',
]);

const KEYWORDS = new Set(['true', 'false', 'nil']);

// Convert path to string key for Set operations
const pathKey = (path: NodePath): string => path.join('.');

// Compare two paths for equality
const pathEquals = (a: NodePath | null, b: NodePath): boolean => {
  if (!a) return false;
  if (a.length !== b.length) return false;
  return a.every((v, i) => v === b[i]);
};

// Check if a path is a prefix of another (or equal)
const pathIsPrefix = (prefix: NodePath | null, path: NodePath): boolean => {
  if (!prefix) return false;
  if (prefix.length > path.length) return false;
  return prefix.every((v, i) => v === path[i]);
};

// Convert cons-list Expr to array for easier rendering
function listToArray(expr: Expr): Expr[] {
  const items: Expr[] = [];
  let current = expr;
  while (current.kind === 'list') {
    items.push(current.head);
    current = current.tail;
  }
  return items;
}

// Get the "head" symbol of a list if it's a symbol
function getHeadSymbol(expr: Expr): string | null {
  if (expr.kind !== 'list') return null;
  if (expr.head.kind !== 'atom') return null;
  if (expr.head.value.kind !== 'symbol') return null;
  return expr.head.value.value;
}

// Determine CSS class for a LispValue
function getAtomClass(value: LispValue): string {
  switch (value.kind) {
    case 'number':
      return value.isFloat ? 'ast-float' : 'ast-number';
    case 'string':
      return 'ast-string';
    case 'symbol':
      if (SPECIAL_FORMS.has(value.value)) return 'ast-special';
      if (KEYWORDS.has(value.value)) return 'ast-keyword';
      return 'ast-symbol';
  }
}

// Format a LispValue for display
function formatAtom(value: LispValue): string {
  switch (value.kind) {
    case 'number':
      return String(value.value);
    case 'string':
      return JSON.stringify(value.value);
    case 'symbol':
      return value.value;
  }
}

type NodeProps = {
  expr: Expr;
  path: NodePath;
  selectedPath: NodePath | null;
  onSelect: (path: NodePath) => void;
  collapsedPaths: Set<string>;
  onToggleCollapse: (path: NodePath) => void;
  depth: number;
  getTypeForPath?: (path: NodePath) => string | null;
  errorPath: NodePath | null;
};

function ASTNode(props: NodeProps): JSX.Element {
  const isSelected = createMemo(() => pathEquals(props.selectedPath, props.path));
  const key = createMemo(() => pathKey(props.path));
  const isCollapsed = createMemo(() => props.collapsedPaths.has(key()));
  const hasError = createMemo(() => pathEquals(props.errorPath, props.path));
  const isInErrorPath = createMemo(() => pathIsPrefix(props.errorPath, props.path));
  const [isHovered, setIsHovered] = createSignal(false);

  // Get type for this node (only for atoms, not lists)
  const nodeType = createMemo(() => {
    if (!props.getTypeForPath) return null;
    // Only show type for atoms to avoid layout issues with list tooltips
    if (props.expr.kind !== 'atom') return null;
    return props.getTypeForPath(props.path);
  });

  const handleClick = (e: MouseEvent) => {
    e.stopPropagation();
    props.onSelect(props.path);
  };

  const handleDoubleClick = (e: MouseEvent) => {
    e.stopPropagation();
    if (props.expr.kind === 'list') {
      props.onToggleCollapse(props.path);
    }
  };

  const handleMouseEnter = (e: MouseEvent) => {
    e.stopPropagation();
    setIsHovered(true);
  };

  const handleMouseLeave = () => {
    setIsHovered(false);
  };

  // Render nil (no tooltip - type shown in selection panel)
  if (props.expr.kind === 'nil') {
    return (
      <span
        class={`ast-node ast-nil ${isSelected() ? 'selected' : ''} ${hasError() ? 'has-error' : ''}`}
        onClick={handleClick}
        data-path={key()}
      >
        ()
      </span>
    );
  }

  // Render atom (symbol, number, string)
  if (props.expr.kind === 'atom') {
    const atomClass = getAtomClass(props.expr.value);
    return (
      <span
        class={`ast-node ast-atom ${atomClass} ${isSelected() ? 'selected' : ''} ${hasError() ? 'has-error' : ''}`}
        onClick={handleClick}
        onMouseEnter={handleMouseEnter}
        onMouseLeave={handleMouseLeave}
        data-path={key()}
      >
        {formatAtom(props.expr.value)}
        <Show when={isHovered() && nodeType()}>
          <span class="type-tooltip">{nodeType()}</span>
        </Show>
      </span>
    );
  }

  // Render list
  const items = createMemo(() => listToArray(props.expr));
  const headSymbol = createMemo(() => getHeadSymbol(props.expr));
  const isSpecialForm = createMemo(() => headSymbol() !== null && SPECIAL_FORMS.has(headSymbol()!));

  // Decide layout: vertical for special forms with >2 items, horizontal otherwise
  const useVerticalLayout = createMemo(() => {
    const h = headSymbol();
    if (!h) return false;
    if (['define', 'lambda', 'let', 'match', 'cond', 'data', 'class', 'instance'].includes(h)) {
      return items().length > 2;
    }
    return false;
  });

  // Don't show hover tooltips on lists - only on atoms
  // List types are shown in the selection panel instead
  return (
    <span
      class={`ast-node ast-list ${isSelected() ? 'selected' : ''} ${isSpecialForm() ? 'special-form' : ''} ${useVerticalLayout() ? 'vertical' : 'horizontal'} ${hasError() ? 'has-error' : ''} ${isInErrorPath() && !hasError() ? 'in-error-path' : ''}`}
      onClick={handleClick}
      onDblClick={handleDoubleClick}
      data-path={key()}
    >
      <span class="ast-paren">(</span>
      <Show
        when={!isCollapsed()}
        fallback={
          <span class="ast-collapsed" onClick={(e) => { e.stopPropagation(); props.onToggleCollapse(props.path); }}>
            {headSymbol() ?? '...'} ...
          </span>
        }
      >
        <span class={`ast-list-content ${useVerticalLayout() ? 'vertical' : 'horizontal'}`}>
          <For each={items()}>
            {(item, index) => (
              <ASTNode
                expr={item}
                path={[...props.path, index()]}
                selectedPath={props.selectedPath}
                onSelect={props.onSelect}
                collapsedPaths={props.collapsedPaths}
                onToggleCollapse={props.onToggleCollapse}
                depth={props.depth + 1}
                getTypeForPath={props.getTypeForPath}
                errorPath={props.errorPath}
              />
            )}
          </For>
        </span>
      </Show>
      <span class="ast-paren">)</span>
    </span>
  );
}

export default function ASTRenderer(props: ASTRendererProps): JSX.Element {
  const handleBackgroundClick = () => {
    props.onDeselect();
  };

  return (
    <div class="ast-renderer" onClick={handleBackgroundClick}>
      <For each={props.exprs}>
        {(expr, index) => (
          <div class="ast-top-level">
            <ASTNode
              expr={expr}
              path={[index()]}
              selectedPath={props.selectedPath}
              onSelect={props.onSelect}
              collapsedPaths={props.collapsedPaths}
              onToggleCollapse={props.onToggleCollapse}
              depth={0}
              getTypeForPath={props.getTypeForPath}
              errorPath={props.errorPath}
            />
          </div>
        )}
      </For>
    </div>
  );
}

// Hook for managing AST selection and collapse state
export function createASTState() {
  const [selectedPath, setSelectedPath] = createSignal<NodePath | null>(null);
  const [collapsedPaths, setCollapsedPaths] = createSignal<Set<string>>(new Set());

  const toggleCollapse = (path: NodePath) => {
    const key = pathKey(path);
    setCollapsedPaths((prev) => {
      const next = new Set(prev);
      if (next.has(key)) {
        next.delete(key);
      } else {
        next.add(key);
      }
      return next;
    });
  };

  return {
    selectedPath,
    setSelectedPath,
    collapsedPaths,
    toggleCollapse,
  };
}

// Navigation helpers

// Get a node at a given path in the AST
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

// Get the number of children of a node (0 for atoms/nil)
function getChildCount(expr: Expr): number {
  if (expr.kind !== 'list') return 0;
  return listToArray(expr).length;
}

// Get parent path (drop last element)
function getParentPath(path: NodePath): NodePath | null {
  if (path.length <= 1) return null;
  return path.slice(0, -1);
}

// Get sibling count at current level
function getSiblingCount(exprs: Expr[], path: NodePath): number {
  if (path.length === 1) {
    return exprs.length;
  }
  const parentPath = getParentPath(path);
  if (!parentPath) return 0;
  const parent = getNodeAtPath(exprs, parentPath);
  if (!parent) return 0;
  return getChildCount(parent);
}

export type NavigationDirection =
  | 'prev-sibling'
  | 'next-sibling'
  | 'parent'
  | 'first-child'
  | 'first-sibling'
  | 'last-sibling';

// Navigate from current path in a direction, returns new path or null if can't navigate
export function navigate(
  exprs: Expr[],
  currentPath: NodePath | null,
  direction: NavigationDirection,
  collapsedPaths: Set<string>
): NodePath | null {
  // If nothing selected, select first top-level expression
  if (!currentPath || currentPath.length === 0) {
    if (exprs.length > 0) return [0];
    return null;
  }

  const currentIndex = currentPath[currentPath.length - 1];
  const siblingCount = getSiblingCount(exprs, currentPath);

  switch (direction) {
    case 'prev-sibling': {
      if (currentIndex > 0) {
        return [...currentPath.slice(0, -1), currentIndex - 1];
      }
      return null; // At first sibling, don't wrap
    }

    case 'next-sibling': {
      if (currentIndex < siblingCount - 1) {
        return [...currentPath.slice(0, -1), currentIndex + 1];
      }
      return null; // At last sibling, don't wrap
    }

    case 'parent': {
      return getParentPath(currentPath);
    }

    case 'first-child': {
      const node = getNodeAtPath(exprs, currentPath);
      if (!node || node.kind !== 'list') return null;

      // Don't enter collapsed nodes
      const key = pathKey(currentPath);
      if (collapsedPaths.has(key)) return null;

      const childCount = getChildCount(node);
      if (childCount === 0) return null;
      return [...currentPath, 0];
    }

    case 'first-sibling': {
      return [...currentPath.slice(0, -1), 0];
    }

    case 'last-sibling': {
      return [...currentPath.slice(0, -1), siblingCount - 1];
    }
  }
}

// Scroll the selected node into view
export function scrollToPath(path: NodePath): void {
  const key = pathKey(path);
  const element = document.querySelector(`[data-path="${key}"]`);
  if (element) {
    element.scrollIntoView({ behavior: 'smooth', block: 'nearest', inline: 'nearest' });
  }
}
