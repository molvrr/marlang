// LSP analyzer - runs the full compilation pipeline and collects information

import * as Lexer from '../src/core/lexer';
import * as Parser from '../src/core/parser';
import type { Expr, SourceLocation } from '../src/core/types';
import type { Type, TypeScheme } from '../src/types/type-ast';
import { formatScheme } from '../src/types/type-ast';
import { typeCheck } from '../src/types/type-checker';
import type { TypeError } from '../src/types/type-errors';
import { formatError } from '../src/types/type-errors';
import type { Diagnostic, Range, Position } from './protocol';
import { DiagnosticSeverity } from './protocol';
import type { TypeEnv } from '../src/types/type-env';
import { createBuiltinEnv } from '../src/types/type-builtins';

// Symbol information for hover and go-to-definition
export type SymbolInfo = {
  name: string;
  type: TypeScheme;
  definitionLoc?: SourceLocation;
  usageLocs: SourceLocation[];
};

// Analysis result
export type AnalysisResult = {
  // Parse errors (if parsing failed)
  parseError?: string;
  // Type errors
  typeErrors: TypeError[];
  // LSP diagnostics
  diagnostics: Diagnostic[];
  // Parsed AST (if parsing succeeded)
  ast?: Expr[];
  // Final type environment (if type checking succeeded)
  typeEnv?: TypeEnv;
  // Symbol table mapping names to their info
  symbols: Map<string, SymbolInfo>;
  // All AST nodes with locations for position-based lookup
  nodesByLocation: Array<{ loc: SourceLocation; expr: Expr; type?: Type }>;
};

// Convert SourceLocation to LSP Range (0-indexed)
export const sourceLocToRange = (loc: SourceLocation): Range => ({
  start: {
    line: loc.start.line - 1,
    character: loc.start.column - 1,
  },
  end: {
    line: loc.end.line - 1,
    character: loc.end.column - 1,
  },
});

// Convert TypeError to LSP Diagnostic
const typeErrorToDiagnostic = (err: TypeError): Diagnostic => {
  const range: Range = err.loc
    ? sourceLocToRange(err.loc)
    : { start: { line: 0, character: 0 }, end: { line: 0, character: 0 } };

  return {
    range,
    message: formatError(err),
    severity: DiagnosticSeverity.Error,
    source: 'marlang',
  };
};

// Collect all AST nodes with their locations
const collectNodes = (expr: Expr, nodes: Array<{ loc: SourceLocation; expr: Expr }>): void => {
  if (expr.loc) {
    nodes.push({ loc: expr.loc, expr });
  }

  if (expr.kind === 'list') {
    collectNodes(expr.head, nodes);
    collectNodes(expr.tail, nodes);
  }
};

// Get symbol name from expression
const getSymbolName = (expr: Expr): string | undefined => {
  if (expr.kind === 'atom' && expr.value.kind === 'symbol') {
    return expr.value.value;
  }
  return undefined;
};

// Convert list expression to array
const listToArray = (expr: Expr): Expr[] => {
  const result: Expr[] = [];
  let current = expr;
  while (current.kind === 'list') {
    result.push(current.head);
    current = current.tail;
  }
  return result;
};

// Extract symbol definitions and usages from AST
const collectSymbols = (
  exprs: Expr[],
  symbols: Map<string, SymbolInfo>,
  typeEnv?: TypeEnv
): void => {
  for (const expr of exprs) {
    collectSymbolsFromExpr(expr, symbols, typeEnv);
  }
};

const collectSymbolsFromExpr = (
  expr: Expr,
  symbols: Map<string, SymbolInfo>,
  typeEnv?: TypeEnv
): void => {
  if (expr.kind === 'atom') {
    const name = getSymbolName(expr);
    if (name && expr.loc) {
      // This is a usage of a symbol
      const existing = symbols.get(name);
      if (existing) {
        existing.usageLocs.push(expr.loc);
      } else {
        // Look up type in environment
        const scheme = typeEnv?.bindings.get(name);
        symbols.set(name, {
          name,
          type: scheme || { quantifiedVars: [], qualType: { constraints: [], type: { kind: 'type-var', name: 'unknown' } } },
          usageLocs: [expr.loc],
        });
      }
    }
  } else if (expr.kind === 'list') {
    const elements = listToArray(expr);
    if (elements.length === 0) return;

    const first = elements[0]!;
    const firstSym = getSymbolName(first);

    // Handle define - extracts definition locations
    if (firstSym === 'define' && elements.length >= 3) {
      const nameExpr = elements[1]!;
      let name: string | undefined;
      let nameLoc: SourceLocation | undefined;

      if (nameExpr.kind === 'atom') {
        name = getSymbolName(nameExpr);
        nameLoc = nameExpr.loc;
      } else if (nameExpr.kind === 'list') {
        // Type-annotated: (name : Type)
        const parts = listToArray(nameExpr);
        if (parts.length >= 1) {
          name = getSymbolName(parts[0]!);
          nameLoc = parts[0]!.loc;
        }
      }

      if (name) {
        const existing = symbols.get(name);
        const scheme = typeEnv?.bindings.get(name);
        if (existing) {
          existing.definitionLoc = nameLoc;
          if (scheme) existing.type = scheme;
        } else {
          symbols.set(name, {
            name,
            type: scheme || { quantifiedVars: [], qualType: { constraints: [], type: { kind: 'type-var', name: 'unknown' } } },
            definitionLoc: nameLoc,
            usageLocs: [],
          });
        }
      }

      // Process value expression
      if (elements[2]) {
        collectSymbolsFromExpr(elements[2], symbols, typeEnv);
      }
      return;
    }

    // Handle lambda - process body
    if (firstSym === 'lambda' && elements.length >= 3) {
      for (let i = 2; i < elements.length; i++) {
        collectSymbolsFromExpr(elements[i]!, symbols, typeEnv);
      }
      return;
    }

    // Handle data definitions
    if (firstSym === 'data' && elements.length >= 2) {
      const typeExpr = elements[1]!;
      let typeName: string | undefined;
      let typeLoc: SourceLocation | undefined;

      if (typeExpr.kind === 'atom') {
        typeName = getSymbolName(typeExpr);
        typeLoc = typeExpr.loc;
      } else if (typeExpr.kind === 'list') {
        const parts = listToArray(typeExpr);
        if (parts.length >= 1) {
          typeName = getSymbolName(parts[0]!);
          typeLoc = parts[0]!.loc;
        }
      }

      if (typeName && typeLoc) {
        symbols.set(typeName, {
          name: typeName,
          type: { quantifiedVars: [], qualType: { constraints: [], type: { kind: 'type-const', name: typeName } } },
          definitionLoc: typeLoc,
          usageLocs: [],
        });
      }

      // Process constructors
      for (let i = 2; i < elements.length; i++) {
        const ctorExpr = elements[i]!;
        let ctorName: string | undefined;
        let ctorLoc: SourceLocation | undefined;

        if (ctorExpr.kind === 'atom') {
          ctorName = getSymbolName(ctorExpr);
          ctorLoc = ctorExpr.loc;
        } else if (ctorExpr.kind === 'list') {
          const parts = listToArray(ctorExpr);
          if (parts.length >= 1) {
            ctorName = getSymbolName(parts[0]!);
            ctorLoc = parts[0]!.loc;
          }
        }

        if (ctorName && ctorLoc) {
          const scheme = typeEnv?.bindings.get(ctorName);
          symbols.set(ctorName, {
            name: ctorName,
            type: scheme || { quantifiedVars: [], qualType: { constraints: [], type: { kind: 'type-var', name: 'unknown' } } },
            definitionLoc: ctorLoc,
            usageLocs: [],
          });
        }
      }
      return;
    }

    // Recurse into all elements
    for (const elem of elements) {
      collectSymbolsFromExpr(elem, symbols, typeEnv);
    }
  }
};

// Convert file:// URI to file path
const uriToPath = (uri: string): string | undefined => {
  if (uri.startsWith('file://')) {
    // Handle file:// URIs
    const path = uri.slice(7);
    // On Windows, file:///C:/... needs special handling
    if (path.match(/^\/[a-zA-Z]:\//)) {
      return path.slice(1);
    }
    return path;
  }
  return undefined;
};

// Analyze source code
export const analyze = (source: string, uri?: string): AnalysisResult => {
  const result: AnalysisResult = {
    typeErrors: [],
    diagnostics: [],
    symbols: new Map(),
    nodesByLocation: [],
  };

  // Lex
  let tokens: Lexer.Token[];
  try {
    tokens = Lexer.lex(Lexer.empty(source));
  } catch (e) {
    result.parseError = e instanceof Error ? e.message : String(e);
    result.diagnostics.push({
      range: { start: { line: 0, character: 0 }, end: { line: 0, character: 0 } },
      message: `Lexer error: ${result.parseError}`,
      severity: DiagnosticSeverity.Error,
      source: 'marlang',
    });
    return result;
  }

  // Parse
  let parsed: Parser.T;
  try {
    parsed = Parser.parse(Parser.empty(tokens));
  } catch (e) {
    result.parseError = e instanceof Error ? e.message : String(e);
    result.diagnostics.push({
      range: { start: { line: 0, character: 0 }, end: { line: 0, character: 0 } },
      message: `Parse error: ${result.parseError}`,
      severity: DiagnosticSeverity.Error,
      source: 'marlang',
    });
    return result;
  }

  result.ast = parsed.exprs;

  // Collect AST nodes
  for (const expr of parsed.exprs) {
    collectNodes(expr, result.nodesByLocation);
  }

  // Type check
  const filePath = uri ? uriToPath(uri) : undefined;
  const typeResult = typeCheck(parsed.exprs, filePath);

  if (!typeResult.ok) {
    result.typeErrors.push(typeResult.error);
    result.diagnostics.push(typeErrorToDiagnostic(typeResult.error));
  } else {
    result.typeEnv = typeResult.env;
  }

  // Collect symbols
  collectSymbols(parsed.exprs, result.symbols, result.typeEnv);

  // Add builtin symbols
  const builtinEnv = createBuiltinEnv();
  for (const [name, scheme] of builtinEnv.bindings) {
    if (!result.symbols.has(name)) {
      result.symbols.set(name, {
        name,
        type: scheme,
        usageLocs: [],
      });
    }
  }

  return result;
};

// Find symbol at position
export const findSymbolAtPosition = (
  result: AnalysisResult,
  line: number,
  character: number
): { expr: Expr; name?: string } | undefined => {
  // Line and character are 0-indexed from LSP
  const targetLine = line + 1;
  const targetColumn = character + 1;

  // Find the smallest node containing this position
  let best: { loc: SourceLocation; expr: Expr } | undefined;

  for (const node of result.nodesByLocation) {
    const { loc, expr } = node;

    // Check if position is within this node
    if (
      (loc.start.line < targetLine ||
        (loc.start.line === targetLine && loc.start.column <= targetColumn)) &&
      (loc.end.line > targetLine ||
        (loc.end.line === targetLine && loc.end.column >= targetColumn))
    ) {
      // Prefer smaller (more specific) nodes
      if (!best || nodeSize(loc) < nodeSize(best.loc)) {
        best = node;
      }
    }
  }

  if (!best) return undefined;

  const name = getSymbolName(best.expr);
  return { expr: best.expr, name };
};

const nodeSize = (loc: SourceLocation): number => {
  return loc.end.offset - loc.start.offset;
};

// Get hover info at position
export const getHoverInfo = (
  result: AnalysisResult,
  line: number,
  character: number
): string | undefined => {
  const found = findSymbolAtPosition(result, line, character);
  if (!found || !found.name) return undefined;

  const symbol = result.symbols.get(found.name);
  if (!symbol) return undefined;

  return `${found.name}: ${formatScheme(symbol.type)}`;
};

// Get definition location for symbol at position
export const getDefinitionLocation = (
  result: AnalysisResult,
  line: number,
  character: number
): SourceLocation | undefined => {
  const found = findSymbolAtPosition(result, line, character);
  if (!found || !found.name) return undefined;

  const symbol = result.symbols.get(found.name);
  return symbol?.definitionLoc;
};
