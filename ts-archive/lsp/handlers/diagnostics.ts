// LSP diagnostics handler

import type { NotificationMessage, Diagnostic } from '../protocol';
import { createNotification } from '../protocol';

// Publish diagnostics notification params
export type PublishDiagnosticsParams = {
  uri: string;
  version?: number;
  diagnostics: Diagnostic[];
};

// Create a publishDiagnostics notification
export const publishDiagnostics = (
  uri: string,
  diagnostics: Diagnostic[],
  version?: number
): NotificationMessage => {
  const params: PublishDiagnosticsParams = {
    uri,
    diagnostics,
  };
  if (version !== undefined) {
    params.version = version;
  }
  return createNotification('textDocument/publishDiagnostics', params);
};

// Clear diagnostics for a document
export const clearDiagnostics = (uri: string): NotificationMessage => {
  return publishDiagnostics(uri, []);
};
