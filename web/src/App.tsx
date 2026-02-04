import { createSignal, createMemo, createEffect, onMount, onCleanup, Show, batch } from 'solid-js';
import * as Marlang from '@marlang/browser';
import ASTRenderer, { createASTState, navigate, scrollToPath, type NodePath, type NavigationDirection } from './ASTRenderer';
import ReferencePanel from './ReferencePanel';
import type { Expr } from '@marlang/types';
import type { TypeMapResult } from '@marlang/browser';
import { createEditorState, type EditorMode } from './editor-state';

const INITIAL_CODE = `(define factorial
  (lambda (n)
    (if (<= n 1)
        1
        (* n (factorial (- n 1))))))

(factorial 5)`;

type ViewMode = 'text' | 'ast';
type OutputTab = 'output' | 'reference';

export default function App() {
  const [code, setCode] = createSignal(INITIAL_CODE);
  const [output, setOutput] = createSignal<{ type: 'success' | 'error'; text: string } | null>(null);
  const [viewMode, setViewMode] = createSignal<ViewMode>('ast');
  const [showTypes, setShowTypes] = createSignal(true);
  const [outputTab, setOutputTab] = createSignal<OutputTab>('output');

  // Get builtin reference for Reference tab
  const builtinReference = Marlang.getBuiltinReference();

  // Editor state (for structural editing)
  const editor = createEditorState();

  // AST state (for visual selection/collapse in display)
  const astState = createASTState();

  // Sync editor cursor with AST state selection
  createEffect(() => {
    astState.setSelectedPath(editor.cursor());
  });

  // Load initial code into editor on mount (only once)
  onMount(() => {
    try {
      const exprs = Marlang.parse(code());
      editor.loadExprs(exprs);
    } catch {
      // Parse error - start with empty editor
    }
  });

  // Get current AST based on view mode
  const currentExprs = createMemo<Expr[]>(() => {
    if (viewMode() === 'text') {
      try {
        return Marlang.parse(code());
      } catch {
        return [];
      }
    }
    return editor.exprs();
  });

  // Parse result for display (includes error info)
  const parsedAST = createMemo<{ ok: true; exprs: Expr[] } | { ok: false; error: string }>(() => {
    if (viewMode() === 'text') {
      try {
        const exprs = Marlang.parse(code());
        return { ok: true, exprs };
      } catch (e) {
        return { ok: false, error: (e as Error).message };
      }
    }
    return { ok: true, exprs: editor.exprs() };
  });

  // Type check the current AST - runs on every change
  const typeInfo = createMemo<TypeMapResult | null>(() => {
    const exprs = currentExprs();
    if (exprs.length === 0) return null;
    return Marlang.buildTypeMap(exprs);
  });

  // Get the type for a node path
  const getTypeForPath = (path: NodePath): string | null => {
    const info = typeInfo();
    if (!info) return null;
    const key = Marlang.pathKey(path);
    const nodeInfo = info.types.get(key);
    return nodeInfo?.formattedType ?? null;
  };

  // Get type error if any
  const typeError = createMemo<{ message: string; path: NodePath | null } | null>(() => {
    const info = typeInfo();
    if (!info || info.ok) return null;
    return {
      message: Marlang.formatError(info.error),
      path: info.errorPath,
    };
  });

  const handleSelect = (path: NodePath) => {
    editor.setCursor(path);
  };

  const handleDeselect = () => {
    editor.setCursor([]);
  };

  // Sync text when switching from AST to text mode
  const switchToTextMode = () => {
    setCode(editor.toText());
    setViewMode('text');
  };

  const switchToASTMode = () => {
    // Parse current text and load into editor
    try {
      const exprs = Marlang.parse(code());
      editor.loadExprs(exprs);
    } catch {
      // Parse error - keep existing editor state
    }
    setViewMode('ast');
  };

  const runCode = () => {
    try {
      // Use editor's text representation in AST mode
      const source = viewMode() === 'ast' ? editor.toText() : code();
      const result = Marlang.run(source, { typecheck: true });
      setOutput({ type: 'success', text: `Result: ${Marlang.formatValue(result.value)}` });
    } catch (e) {
      setOutput({ type: 'error', text: `Error: ${(e as Error).message}` });
    }
  };

  const typeCheck = () => {
    try {
      // Use editor's exprs directly in AST mode
      const exprs = viewMode() === 'ast' ? editor.exprs() : Marlang.parse(code());
      const result = Marlang.typeCheck(exprs);
      if (result.ok) {
        const types = Marlang.formatTypes(result);
        setOutput({
          type: 'success',
          text: `Type check passed!\n\nInferred types:\n${types.map((t, i) => `  ${i + 1}: ${t}`).join('\n')}`,
        });
      } else {
        setOutput({ type: 'error', text: `Type error: ${Marlang.formatError(result.error)}` });
      }
    } catch (e) {
      setOutput({ type: 'error', text: `Error: ${(e as Error).message}` });
    }
  };

  // Get selected node's type for display
  const selectedType = createMemo(() => {
    const path = astState.selectedPath();
    if (!path) return null;
    return getTypeForPath(path);
  });

  // Keyboard navigation and editing
  const handleKeyDown = (e: KeyboardEvent) => {
    // Only handle keys when in AST view mode
    if (viewMode() !== 'ast') return;

    // Don't handle if user is typing in an input (except our input)
    if (e.target instanceof HTMLTextAreaElement) return;

    const parsed = parsedAST();
    if (!parsed.ok) return;

    // Handle insert mode
    if (editor.mode() === 'insert') {
      handleInsertModeKey(e);
      return;
    }

    // Normal mode
    let direction: NavigationDirection | null = null;

    switch (e.key) {
      // Navigation
      case 'h':
      case 'ArrowLeft':
        direction = 'prev-sibling';
        break;
      case 'l':
      case 'ArrowRight':
        direction = 'next-sibling';
        break;
      case 'k':
      case 'ArrowUp':
        direction = 'parent';
        break;
      case 'j':
      case 'ArrowDown':
      case 'Enter':
        direction = 'first-child';
        break;
      case '0':
      case 'Home':
        direction = 'first-sibling';
        break;
      case '$':
      case 'End':
        direction = 'last-sibling';
        break;

      // Escape deselects
      case 'Escape':
        editor.setCursor([]);
        e.preventDefault();
        return;

      // Space to toggle collapse
      case ' ':
        const current = editor.cursor();
        if (current.length > 0) {
          astState.toggleCollapse(current);
          e.preventDefault();
        }
        return;

      // Structural editing
      case 'd':
      case 'Delete':
        editor.deleteSelected();
        e.preventDefault();
        return;

      case 'x':
        // Cut (copy + delete)
        editor.cut();
        e.preventDefault();
        return;

      case 'w':
        // Wrap in list
        editor.wrapSelected();
        e.preventDefault();
        return;

      case 'W':
        // Wrap with lambda
        editor.wrapWithSymbol('lambda');
        e.preventDefault();
        return;

      case 'u':
        // Unwrap/splice
        editor.unwrapSelected();
        e.preventDefault();
        return;

      case 'r':
        // Raise
        editor.raiseSelected();
        e.preventDefault();
        return;

      case 'o':
        // Insert empty list after
        editor.insertEmptyList();
        e.preventDefault();
        return;

      // Clipboard
      case 'y':
        if (e.metaKey || e.ctrlKey) {
          // Ctrl+Y = Redo
          editor.redo();
        } else {
          // y = Yank (copy)
          editor.copy();
        }
        e.preventDefault();
        return;

      case 'p':
        // Paste after
        editor.pasteAfter();
        e.preventDefault();
        return;

      case 'P':
        // Paste before
        editor.pasteBefore();
        e.preventDefault();
        return;

      // Insert mode triggers
      case 'i': {
        // If a list (or empty list) is selected, insert as first child; otherwise insert before
        const node = editor.selectedNode();
        if (node && (node.kind === 'list' || node.kind === 'nil')) {
          editor.enterInsertMode({ kind: 'first-child' });
        } else {
          editor.enterInsertMode({ kind: 'before' });
        }
        e.preventDefault();
        return;
      }

      case 'a': {
        // If a list (or empty list) is selected, append as last child; otherwise insert after
        const node = editor.selectedNode();
        if (node && (node.kind === 'list' || node.kind === 'nil')) {
          editor.enterInsertMode({ kind: 'child' });
        } else {
          editor.enterInsertMode({ kind: 'after' });
        }
        e.preventDefault();
        return;
      }

      case 'c':
        // Change (replace)
        editor.enterInsertMode({ kind: 'replace' });
        e.preventDefault();
        return;

      case 'A':
        // Append child
        editor.enterInsertMode({ kind: 'child' });
        e.preventDefault();
        return;

      // Undo/Redo
      case 'z':
        if (e.metaKey || e.ctrlKey) {
          if (e.shiftKey) {
            editor.redo();
          } else {
            editor.undo();
          }
          e.preventDefault();
        }
        return;

      default:
        return;
    }

    if (direction) {
      e.preventDefault();
      const newPath = navigate(
        parsed.exprs,
        editor.cursor(),
        direction,
        astState.collapsedPaths()
      );
      if (newPath) {
        editor.setCursor(newPath);
      }
    }
  };

  // Handle keys in insert mode
  const handleInsertModeKey = (e: KeyboardEvent) => {
    switch (e.key) {
      case 'Escape':
        editor.exitInsertMode();
        e.preventDefault();
        return;

      case 'Enter':
        editor.commitInsert();
        e.preventDefault();
        return;

      case 'Backspace':
        editor.handleInsertBackspace();
        e.preventDefault();
        return;

      default:
        // Only accept printable characters
        if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) {
          editor.handleInsertInput(e.key);
          e.preventDefault();
        }
        return;
    }
  };

  // Set up keyboard listener
  onMount(() => {
    document.addEventListener('keydown', handleKeyDown);
  });

  onCleanup(() => {
    document.removeEventListener('keydown', handleKeyDown);
  });

  // Scroll to selected node when cursor changes
  createEffect(() => {
    const path = editor.cursor();
    if (path.length > 0) {
      // Small delay to ensure DOM is updated
      requestAnimationFrame(() => scrollToPath(path));
    }
  });

  return (
    <div class="app">
      <header>
        <h1>Marlang Editor</h1>
      </header>

      <main>
        <section class="editor-section">
          <div class="editor-controls">
            <div class="view-toggle">
              <button
                class={viewMode() === 'text' ? 'active' : ''}
                onClick={switchToTextMode}
              >
                Text
              </button>
              <button
                class={viewMode() === 'ast' ? 'active' : ''}
                onClick={switchToASTMode}
              >
                AST
              </button>
            </div>

            <Show when={viewMode() === 'ast'}>
              <div class={`mode-indicator ${editor.mode()}`}>
                {editor.mode() === 'insert' ? 'INSERT' : 'NORMAL'}
              </div>
            </Show>

            <Show when={viewMode() === 'ast'}>
              <label class="types-toggle">
                <input
                  type="checkbox"
                  checked={showTypes()}
                  onChange={(e) => setShowTypes(e.currentTarget.checked)}
                />
                <span>Hover Types</span>
              </label>
            </Show>
          </div>

          <Show when={typeError()}>
            {(error) => (
              <div class="type-error-banner">
                <span class="error-icon">⚠</span>
                <span>{error().message}</span>
              </div>
            )}
          </Show>

          <Show
            when={viewMode() === 'ast'}
            fallback={
              <textarea
                value={code()}
                onInput={(e) => setCode(e.currentTarget.value)}
                spellcheck={false}
              />
            }
          >
            <Show
              when={parsedAST().ok}
              fallback={
                <div class="ast-renderer">
                  <pre class="error">Parse error: {(parsedAST() as { ok: false; error: string }).error}</pre>
                </div>
              }
            >
              <ASTRenderer
                exprs={currentExprs()}
                selectedPath={astState.selectedPath()}
                onSelect={handleSelect}
                onDeselect={handleDeselect}
                collapsedPaths={astState.collapsedPaths()}
                onToggleCollapse={astState.toggleCollapse}
                getTypeForPath={showTypes() ? getTypeForPath : undefined}
                errorPath={typeError()?.path ?? null}
              />
            </Show>
          </Show>

          <div class="info-panel">
            <Show
              when={selectedType()}
              fallback={
                <span class="info-hint">Click a node or press any arrow key to start</span>
              }
            >
              {(type) => (
                <>
                  <span class="type-label">Type:</span>
                  <code class="type-value">{type()}</code>
                </>
              )}
            </Show>
          </div>

          <Show when={viewMode() === 'ast' && editor.mode() === 'insert'}>
            <div class="insert-input">
              <span class="insert-label">
                {editor.insertTarget().kind === 'replace' ? 'Change:' :
                 editor.insertTarget().kind === 'child' ? 'Append:' :
                 editor.insertTarget().kind === 'before' ? 'Insert:' : 'Insert:'}
              </span>
              <span class="insert-buffer">{editor.inputBuffer()}<span class="cursor-blink">|</span></span>
              <span class="insert-hint">Enter to confirm, Esc to cancel</span>
            </div>
          </Show>

          <Show when={viewMode() === 'ast' && editor.mode() === 'normal'}>
            <div class="keyboard-hint">
              <Show
                when={editor.cursor().length > 0}
                fallback={
                  <>
                    <span><kbd>j</kbd> or click to select</span>
                    <span><kbd>o</kbd> new list</span>
                    <span><kbd>a</kbd><kbd>i</kbd> insert atom</span>
                  </>
                }
              >
                <span><kbd>h</kbd><kbd>j</kbd><kbd>k</kbd><kbd>l</kbd> navigate</span>
                <span><kbd>d</kbd> delete</span>
                <span><kbd>w</kbd> wrap</span>
                <span><kbd>u</kbd> unwrap</span>
                <span><kbd>r</kbd> raise</span>
                <span><kbd>o</kbd> new list</span>
                <span><kbd>y</kbd><kbd>x</kbd><kbd>p</kbd> yank/cut/paste</span>
                <span><kbd>i</kbd><kbd>a</kbd><kbd>c</kbd> insert/change</span>
              </Show>
            </div>
          </Show>

          <div class="toolbar">
            <button onClick={runCode}>Run</button>
            <button onClick={typeCheck}>Type Check</button>
          </div>
        </section>

        <section class="output-section">
          <div class="output-tabs">
            <button
              class={outputTab() === 'output' ? 'active' : ''}
              onClick={() => setOutputTab('output')}
            >
              Output
            </button>
            <button
              class={outputTab() === 'reference' ? 'active' : ''}
              onClick={() => setOutputTab('reference')}
            >
              Reference
            </button>
          </div>
          <Show
            when={outputTab() === 'output'}
            fallback={<ReferencePanel reference={builtinReference} />}
          >
            <pre class={output()?.type ?? ''}>
              {output()?.text ?? 'Click "Run" or "Type Check" to see output'}
            </pre>
          </Show>
        </section>
      </main>
    </div>
  );
}
