import { describe, test, expect } from 'bun:test';
import * as Ops from '../web/src/editor-ops';

// Helper to create test ASTs
const sym = (s: string) => Ops.symbol(s);
const num = (n: number) => Ops.number(n);

describe('AST Construction', () => {
  test('creates atoms', () => {
    expect(sym('foo')).toEqual({
      kind: 'atom',
      value: { kind: 'symbol', value: 'foo' },
    });

    expect(num(42)).toEqual({
      kind: 'atom',
      value: { kind: 'number', value: 42, isFloat: false },
    });

    expect(Ops.string('hello')).toEqual({
      kind: 'atom',
      value: { kind: 'string', value: 'hello' },
    });
  });

  test('creates lists', () => {
    const lst = Ops.list(sym('a'), sym('b'), sym('c'));
    expect(lst.kind).toBe('list');
    expect(Ops.listToArray(lst)).toEqual([sym('a'), sym('b'), sym('c')]);
  });

  test('empty list is nil', () => {
    const empty = Ops.list();
    expect(empty).toEqual(Ops.nil);
  });
});

describe('Path Operations', () => {
  const testAST = [
    Ops.list(sym('define'), sym('x'), num(1)),
    Ops.list(sym('define'), sym('y'), Ops.list(sym('+'), sym('x'), num(2))),
  ];

  test('getNodeAtPath finds top-level', () => {
    expect(Ops.getNodeAtPath(testAST, [0])).toEqual(testAST[0]);
    expect(Ops.getNodeAtPath(testAST, [1])).toEqual(testAST[1]);
  });

  test('getNodeAtPath finds nested', () => {
    expect(Ops.getNodeAtPath(testAST, [0, 0])).toEqual(sym('define'));
    expect(Ops.getNodeAtPath(testAST, [0, 1])).toEqual(sym('x'));
    expect(Ops.getNodeAtPath(testAST, [1, 2, 0])).toEqual(sym('+'));
  });

  test('getNodeAtPath returns null for invalid', () => {
    expect(Ops.getNodeAtPath(testAST, [])).toBeNull();
    expect(Ops.getNodeAtPath(testAST, [5])).toBeNull();
    expect(Ops.getNodeAtPath(testAST, [0, 10])).toBeNull();
  });
});

describe('Delete Operation', () => {
  test('deletes top-level expression', () => {
    const exprs = [sym('a'), sym('b'), sym('c')];
    const result = Ops.deleteNode(exprs, [1]);

    expect(result.exprs).toEqual([sym('a'), sym('c')]);
    expect(result.cursor).toEqual([0]); // Moves to previous sibling
  });

  test('deletes first top-level goes to new first', () => {
    const exprs = [sym('a'), sym('b')];
    const result = Ops.deleteNode(exprs, [0]);

    expect(result.exprs).toEqual([sym('b')]);
    expect(result.cursor).toEqual([0]);
  });

  test('deletes last top-level leaves empty cursor', () => {
    const exprs = [sym('a')];
    const result = Ops.deleteNode(exprs, [0]);

    expect(result.exprs).toEqual([]);
    expect(result.cursor).toEqual([]);
  });

  test('deletes element from list', () => {
    const exprs = [Ops.list(sym('a'), sym('b'), sym('c'))];
    const result = Ops.deleteNode(exprs, [0, 1]);

    expect(Ops.listToArray(result.exprs[0])).toEqual([sym('a'), sym('c')]);
    expect(result.cursor).toEqual([0, 0]); // Moves to previous sibling
  });
});

describe('Insert Operations', () => {
  test('insertAfter at top level', () => {
    const exprs = [sym('a'), sym('b')];
    const result = Ops.insertAfter(exprs, [0], sym('x'));

    expect(result.exprs).toEqual([sym('a'), sym('x'), sym('b')]);
    expect(result.cursor).toEqual([1]);
  });

  test('insertBefore at top level', () => {
    const exprs = [sym('a'), sym('b')];
    const result = Ops.insertBefore(exprs, [1], sym('x'));

    expect(result.exprs).toEqual([sym('a'), sym('x'), sym('b')]);
    expect(result.cursor).toEqual([1]);
  });

  test('insertAfter in list', () => {
    const exprs = [Ops.list(sym('a'), sym('b'), sym('c'))];
    const result = Ops.insertAfter(exprs, [0, 0], sym('x'));

    expect(Ops.listToArray(result.exprs[0])).toEqual([
      sym('a'),
      sym('x'),
      sym('b'),
      sym('c'),
    ]);
    expect(result.cursor).toEqual([0, 1]);
  });

  test('insertAfter with no selection adds at end', () => {
    const exprs = [sym('a')];
    const result = Ops.insertAfter(exprs, [], sym('x'));

    expect(result.exprs).toEqual([sym('a'), sym('x')]);
    expect(result.cursor).toEqual([1]);
  });
});

describe('Wrap Operation', () => {
  test('wrap creates list with one child', () => {
    const exprs = [sym('a'), sym('b')];
    const result = Ops.wrapInList(exprs, [0]);

    expect(result.exprs[0].kind).toBe('list');
    expect(Ops.listToArray(result.exprs[0])).toEqual([sym('a')]);
    expect(result.cursor).toEqual([0, 0]); // Cursor on wrapped element
  });

  test('wrapWithSymbol creates (sym child)', () => {
    const exprs = [sym('x')];
    const result = Ops.wrapWithSymbol(exprs, [0], 'lambda');

    const items = Ops.listToArray(result.exprs[0]);
    expect(items).toEqual([sym('lambda'), sym('x')]);
    expect(result.cursor).toEqual([0, 1]); // Cursor on original element
  });
});

describe('Unwrap/Splice Operation', () => {
  test('unwrap single child replaces list', () => {
    const exprs = [Ops.list(sym('x'))];
    const result = Ops.unwrap(exprs, [0]);

    expect(result.exprs).toEqual([sym('x')]);
    expect(result.cursor).toEqual([0]);
  });

  test('unwrap multiple children splices into parent', () => {
    const inner = Ops.list(sym('a'), sym('b'));
    const exprs = [Ops.list(sym('x'), inner, sym('y'))];
    const result = Ops.unwrap(exprs, [0, 1]);

    const items = Ops.listToArray(result.exprs[0]);
    expect(items).toEqual([sym('x'), sym('a'), sym('b'), sym('y')]);
    expect(result.cursor).toEqual([0, 1]);
  });

  test('unwrap empty list deletes it', () => {
    const exprs = [Ops.list(sym('x'), Ops.list(), sym('y'))];
    const result = Ops.unwrap(exprs, [0, 1]);

    const items = Ops.listToArray(result.exprs[0]);
    expect(items).toEqual([sym('x'), sym('y')]);
  });
});

describe('Raise Operation', () => {
  test('raise replaces parent with child', () => {
    const exprs = [Ops.list(sym('a'), sym('b'), sym('c'))];
    const result = Ops.raise(exprs, [0, 1]);

    expect(result.exprs).toEqual([sym('b')]);
    expect(result.cursor).toEqual([0]);
  });

  test('raise at top level does nothing', () => {
    const exprs = [sym('a')];
    const result = Ops.raise(exprs, [0]);

    expect(result.exprs).toEqual([sym('a')]);
    expect(result.cursor).toEqual([0]);
  });
});

describe('AST to Text', () => {
  test('converts atoms', () => {
    expect(Ops.exprToString(sym('foo'))).toBe('foo');
    expect(Ops.exprToString(num(42))).toBe('42');
    expect(Ops.exprToString(Ops.string('hi'))).toBe('"hi"');
  });

  test('converts lists', () => {
    const lst = Ops.list(sym('define'), sym('x'), num(1));
    expect(Ops.exprToString(lst)).toBe('(define x 1)');
  });

  test('converts nested lists', () => {
    const lst = Ops.list(
      sym('define'),
      sym('f'),
      Ops.list(sym('lambda'), Ops.list(sym('x')), sym('x'))
    );
    expect(Ops.exprToString(lst)).toBe('(define f (lambda (x) x))');
  });

  test('converts multiple expressions', () => {
    const exprs = [
      Ops.list(sym('define'), sym('x'), num(1)),
      Ops.list(sym('print'), sym('x')),
    ];
    expect(Ops.exprsToString(exprs)).toBe('(define x 1)\n\n(print x)');
  });
});
