import { describe, it, expect } from 'bun:test';
import { analyze, getHoverInfo, findSymbolAtPosition } from '../lsp/analyzer';
import { handleCompletion } from '../lsp/handlers/completion';
import { handleHover } from '../lsp/handlers/hover';
import { handleDefinition } from '../lsp/handlers/definition';
import { ProtocolReader, ProtocolWriter, createResponse } from '../lsp/protocol';

describe('LSP Protocol', () => {
  it('should parse messages correctly', () => {
    const reader = new ProtocolReader();
    const messages = reader.feed('Content-Length: 75\r\n\r\n{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}');

    expect(messages.length).toBe(1);
    expect(messages[0]).toEqual({
      jsonrpc: '2.0',
      id: 1,
      method: 'initialize',
      params: { capabilities: {} }
    });
  });

  it('should write messages correctly', () => {
    const writer = new ProtocolWriter();
    const output = writer.write(createResponse(1, { test: true }));

    expect(output).toContain('Content-Length:');
    expect(output).toContain('"jsonrpc":"2.0"');
    expect(output).toContain('"id":1');
    expect(output).toContain('"result":{"test":true}');
  });
});

describe('LSP Analyzer', () => {
  it('should analyze valid code', () => {
    const result = analyze('(define x 42)\n(+ x 1)');

    expect(result.diagnostics.length).toBe(0);
    expect(result.parseError).toBeUndefined();
    expect(result.ast).toBeDefined();
    expect(result.ast!.length).toBe(2);
  });

  it('should report type errors', () => {
    const result = analyze('(+ 1 "hello")');

    expect(result.diagnostics.length).toBe(1);
    expect(result.diagnostics[0]!.message).toContain('Type mismatch');
  });

  it('should report parse errors', () => {
    const result = analyze('(define x');

    expect(result.parseError).toBeDefined();
    expect(result.diagnostics.length).toBe(1);
  });

  it('should track symbol definitions', () => {
    const result = analyze('(define foo 42)');
    const symbol = result.symbols.get('foo');

    expect(symbol).toBeDefined();
    expect(symbol!.definitionLoc).toBeDefined();
    expect(symbol!.definitionLoc!.start.line).toBe(1);
    expect(symbol!.definitionLoc!.start.column).toBe(9);
  });

  it('should collect AST node locations', () => {
    const result = analyze('42');

    expect(result.nodesByLocation.length).toBeGreaterThan(0);
    expect(result.nodesByLocation[0]!.loc.start.line).toBe(1);
  });
});

describe('LSP Hover', () => {
  it('should return type info for symbols', () => {
    const result = analyze('(define x 42)\n(+ x 1)');
    const hover = handleHover(result, { line: 0, character: 9 });

    expect(hover).not.toBeNull();
    expect(hover!.contents).toBeDefined();
  });

  it('should return null for non-symbols', () => {
    const result = analyze('42');
    const hover = handleHover(result, { line: 0, character: 5 });

    expect(hover).toBeNull();
  });
});

describe('LSP Definition', () => {
  it('should return definition location for defined symbols', () => {
    const result = analyze('(define foo 42)\n(+ foo 1)');
    // Click on 'foo' in the second line
    const def = handleDefinition(result, 'test.mar', { line: 1, character: 3 });

    expect(def).not.toBeNull();
    expect(def!.range.start.line).toBe(0);
    expect(def!.range.start.character).toBe(8);
  });
});

describe('LSP Completion', () => {
  it('should include keywords', () => {
    const result = analyze('');
    const completions = handleCompletion(result, { line: 0, character: 0 });

    const labels = completions.items.map(i => i.label);
    expect(labels).toContain('define');
    expect(labels).toContain('lambda');
    expect(labels).toContain('if');
    expect(labels).toContain('let');
  });

  it('should include builtins', () => {
    const result = analyze('');
    const completions = handleCompletion(result, { line: 0, character: 0 });

    const labels = completions.items.map(i => i.label);
    expect(labels).toContain('+');
    expect(labels).toContain('-');
    expect(labels).toContain('*');
  });

  it('should include user-defined symbols', () => {
    const result = analyze('(define myFunc (lambda (x) x))');
    const completions = handleCompletion(result, { line: 0, character: 0 });

    const labels = completions.items.map(i => i.label);
    expect(labels).toContain('myFunc');
  });
});
