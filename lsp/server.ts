// Marlang LSP Server

import {
  ProtocolReader,
  ProtocolWriter,
  createResponse,
  createErrorResponse,
  isRequest,
  isNotification,
  ErrorCodes,
  type Message,
  type RequestMessage,
  type NotificationMessage,
} from './protocol';
import { DocumentManager } from './document';
import { analyze, type AnalysisResult } from './analyzer';
import { publishDiagnostics } from './handlers/diagnostics';
import { handleHover, type HoverParams } from './handlers/hover';
import { handleDefinition, type DefinitionParams } from './handlers/definition';
import { handleCompletion, type CompletionParams } from './handlers/completion';

// Server state
const documents = new DocumentManager();
const analysisCache = new Map<string, AnalysisResult>();
const reader = new ProtocolReader();
const writer = new ProtocolWriter();

let initialized = false;
let shutdownRequested = false;

// Write a message to stdout
const send = (msg: Message): void => {
  const output = writer.write(msg);
  process.stdout.write(output);
};

// Analyze a document and publish diagnostics
const analyzeDocument = (uri: string, content: string, version?: number): void => {
  const result = analyze(content);
  analysisCache.set(uri, result);
  send(publishDiagnostics(uri, result.diagnostics, version));
};

// Handle initialize request
const handleInitialize = (_params: unknown): unknown => {
  initialized = true;
  return {
    capabilities: {
      textDocumentSync: {
        openClose: true,
        change: 1, // Full document sync
      },
      hoverProvider: true,
      definitionProvider: true,
      completionProvider: {
        triggerCharacters: ['(', ' '],
      },
    },
    serverInfo: {
      name: 'marlang-lsp',
      version: '0.1.0',
    },
  };
};

// Handle initialized notification
const handleInitialized = (): void => {
  // Server is fully initialized
};

// Handle shutdown request
const handleShutdown = (): null => {
  shutdownRequested = true;
  return null;
};

// Handle exit notification
const handleExit = (): void => {
  process.exit(shutdownRequested ? 0 : 1);
};

// Handle textDocument/didOpen
const handleDidOpen = (params: {
  textDocument: { uri: string; languageId: string; version: number; text: string };
}): void => {
  const { uri, version, text } = params.textDocument;
  documents.open(uri, version, text);
  analyzeDocument(uri, text, version);
};

// Handle textDocument/didChange
const handleDidChange = (params: {
  textDocument: { uri: string; version: number };
  contentChanges: Array<{ text: string }>;
}): void => {
  const { uri, version } = params.textDocument;
  const changes = params.contentChanges;

  if (changes.length > 0) {
    const text = changes[changes.length - 1]!.text;
    documents.update(uri, version, text);
    analyzeDocument(uri, text, version);
  }
};

// Handle textDocument/didClose
const handleDidClose = (params: { textDocument: { uri: string } }): void => {
  const { uri } = params.textDocument;
  documents.close(uri);
  analysisCache.delete(uri);
};

// Handle textDocument/hover
const handleHoverRequest = (params: HoverParams): unknown => {
  const { uri } = params.textDocument;
  const result = analysisCache.get(uri);
  if (!result) return null;

  return handleHover(result, params.position);
};

// Handle textDocument/definition
const handleDefinitionRequest = (params: DefinitionParams): unknown => {
  const { uri } = params.textDocument;
  const result = analysisCache.get(uri);
  if (!result) return null;

  return handleDefinition(result, uri, params.position);
};

// Handle textDocument/completion
const handleCompletionRequest = (params: CompletionParams): unknown => {
  const { uri } = params.textDocument;
  const result = analysisCache.get(uri);
  if (!result) {
    return { isIncomplete: false, items: [] };
  }

  return handleCompletion(result, params.position);
};

// Handle a request message
const handleRequest = (msg: RequestMessage): void => {
  if (!initialized && msg.method !== 'initialize') {
    send(createErrorResponse(msg.id, ErrorCodes.ServerNotInitialized, 'Server not initialized'));
    return;
  }

  try {
    let result: unknown;

    switch (msg.method) {
      case 'initialize':
        result = handleInitialize(msg.params);
        break;
      case 'shutdown':
        result = handleShutdown();
        break;
      case 'textDocument/hover':
        result = handleHoverRequest(msg.params as HoverParams);
        break;
      case 'textDocument/definition':
        result = handleDefinitionRequest(msg.params as DefinitionParams);
        break;
      case 'textDocument/completion':
        result = handleCompletionRequest(msg.params as CompletionParams);
        break;
      default:
        send(
          createErrorResponse(msg.id, ErrorCodes.MethodNotFound, `Method not found: ${msg.method}`)
        );
        return;
    }

    send(createResponse(msg.id, result));
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e);
    send(createErrorResponse(msg.id, ErrorCodes.InternalError, message));
  }
};

// Handle a notification message
const handleNotification = (msg: NotificationMessage): void => {
  if (!initialized && msg.method !== 'exit') {
    return;
  }

  try {
    switch (msg.method) {
      case 'initialized':
        handleInitialized();
        break;
      case 'exit':
        handleExit();
        break;
      case 'textDocument/didOpen':
        handleDidOpen(msg.params as Parameters<typeof handleDidOpen>[0]);
        break;
      case 'textDocument/didChange':
        handleDidChange(msg.params as Parameters<typeof handleDidChange>[0]);
        break;
      case 'textDocument/didClose':
        handleDidClose(msg.params as Parameters<typeof handleDidClose>[0]);
        break;
      // Ignore other notifications
    }
  } catch (e) {
    // Notifications don't get responses, but we can log the error
    console.error(`Error handling notification ${msg.method}:`, e);
  }
};

// Process incoming messages
const processMessage = (msg: Message): void => {
  if (isRequest(msg)) {
    handleRequest(msg);
  } else if (isNotification(msg)) {
    handleNotification(msg);
  }
  // Ignore responses (we don't send requests to the client)
};

// Main entry point
const main = async (): Promise<void> => {
  // Read from stdin
  process.stdin.setEncoding('utf8');

  for await (const chunk of process.stdin) {
    try {
      const messages = reader.feed(chunk as string);
      for (const msg of messages) {
        processMessage(msg);
      }
    } catch (e) {
      console.error('Error processing input:', e);
    }
  }
};

// Start the server
main().catch((e) => {
  console.error('LSP server error:', e);
  process.exit(1);
});
