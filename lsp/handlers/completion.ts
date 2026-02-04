// LSP completion handler

import type { CompletionItem, CompletionList, Position } from '../protocol';
import { CompletionItemKind } from '../protocol';
import type { AnalysisResult } from '../analyzer';
import { formatScheme } from '../../type-ast';

// Completion request params
export type CompletionParams = {
  textDocument: { uri: string };
  position: Position;
};

// Marlang keywords
const KEYWORDS = [
  'define',
  'lambda',
  'if',
  'let',
  'cond',
  'begin',
  'match',
  'data',
  'class',
  'instance',
  'type',
  'else',
];

// Handle completion request
export const handleCompletion = (
  result: AnalysisResult,
  _position: Position
): CompletionList => {
  const items: CompletionItem[] = [];

  // Add keywords
  for (const kw of KEYWORDS) {
    items.push({
      label: kw,
      kind: CompletionItemKind.Keyword,
      detail: 'keyword',
    });
  }

  // Add symbols from the analysis
  for (const [name, info] of result.symbols) {
    // Skip if it looks like a type variable (starts with lowercase t followed by numbers)
    if (/^t\d+$/.test(name)) continue;

    const typeStr = formatScheme(info.type);
    const isConstructor =
      name[0] !== undefined && name[0] === name[0].toUpperCase() && /^[A-Z]/.test(name);
    const isFunction = typeStr.includes('->');

    let kind: number;
    if (isConstructor) {
      kind = CompletionItemKind.Constructor;
    } else if (isFunction) {
      kind = CompletionItemKind.Function;
    } else {
      kind = CompletionItemKind.Variable;
    }

    items.push({
      label: name,
      kind,
      detail: typeStr,
    });
  }

  return {
    isIncomplete: false,
    items,
  };
};
