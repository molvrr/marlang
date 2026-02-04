// LSP hover handler

import type { Hover, Range, Position } from '../protocol';
import type { AnalysisResult } from '../analyzer';
import { getHoverInfo, findSymbolAtPosition, sourceLocToRange } from '../analyzer';

// Hover request params
export type HoverParams = {
  textDocument: { uri: string };
  position: Position;
};

// Handle hover request
export const handleHover = (
  result: AnalysisResult,
  position: Position
): Hover | null => {
  const info = getHoverInfo(result, position.line, position.character);
  if (!info) return null;

  // Find the symbol to get its range
  const found = findSymbolAtPosition(result, position.line, position.character);
  const range: Range | undefined = found?.expr.loc
    ? sourceLocToRange(found.expr.loc)
    : undefined;

  return {
    contents: {
      kind: 'markdown',
      value: '```marlang\n' + info + '\n```',
    },
    range,
  };
};
