// LSP go-to-definition handler

import type { Location, Position, Range } from '../protocol';
import type { AnalysisResult } from '../analyzer';
import { getDefinitionLocation, sourceLocToRange } from '../analyzer';

// Definition request params
export type DefinitionParams = {
  textDocument: { uri: string };
  position: Position;
};

// Handle go-to-definition request
export const handleDefinition = (
  result: AnalysisResult,
  uri: string,
  position: Position
): Location | null => {
  const loc = getDefinitionLocation(result, position.line, position.character);
  if (!loc) return null;

  return {
    uri,
    range: sourceLocToRange(loc),
  };
};
