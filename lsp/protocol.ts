// LSP JSON-RPC protocol implementation over stdin/stdout

// LSP Message types
export type RequestMessage = {
  jsonrpc: '2.0';
  id: number | string;
  method: string;
  params?: unknown;
};

export type ResponseMessage = {
  jsonrpc: '2.0';
  id: number | string | null;
  result?: unknown;
  error?: ResponseError;
};

export type NotificationMessage = {
  jsonrpc: '2.0';
  method: string;
  params?: unknown;
};

export type ResponseError = {
  code: number;
  message: string;
  data?: unknown;
};

export type Message = RequestMessage | ResponseMessage | NotificationMessage;

// LSP Error codes
export const ErrorCodes = {
  ParseError: -32700,
  InvalidRequest: -32600,
  MethodNotFound: -32601,
  InvalidParams: -32602,
  InternalError: -32603,
  ServerNotInitialized: -32002,
  UnknownErrorCode: -32001,
} as const;

// LSP Position and Range types
export type Position = {
  line: number;
  character: number;
};

export type Range = {
  start: Position;
  end: Position;
};

export type Location = {
  uri: string;
  range: Range;
};

// Diagnostic types
export const DiagnosticSeverity = {
  Error: 1,
  Warning: 2,
  Information: 3,
  Hint: 4,
} as const;

export type Diagnostic = {
  range: Range;
  message: string;
  severity?: number;
  code?: number | string;
  source?: string;
};

// TextDocument types
export type TextDocumentIdentifier = {
  uri: string;
};

export type VersionedTextDocumentIdentifier = TextDocumentIdentifier & {
  version: number;
};

export type TextDocumentContentChangeEvent = {
  text: string;
};

export type TextDocumentItem = {
  uri: string;
  languageId: string;
  version: number;
  text: string;
};

// Completion types
export const CompletionItemKind = {
  Text: 1,
  Method: 2,
  Function: 3,
  Constructor: 4,
  Field: 5,
  Variable: 6,
  Class: 7,
  Interface: 8,
  Module: 9,
  Property: 10,
  Unit: 11,
  Value: 12,
  Enum: 13,
  Keyword: 14,
  Snippet: 15,
  Color: 16,
  File: 17,
  Reference: 18,
  Folder: 19,
  EnumMember: 20,
  Constant: 21,
  Struct: 22,
  Event: 23,
  Operator: 24,
  TypeParameter: 25,
} as const;

export type CompletionItem = {
  label: string;
  kind?: number;
  detail?: string;
  documentation?: string;
  insertText?: string;
};

export type CompletionList = {
  isIncomplete: boolean;
  items: CompletionItem[];
};

// Hover types
export type Hover = {
  contents: string | { kind: 'markdown' | 'plaintext'; value: string };
  range?: Range;
};

// Protocol reader/writer
export class ProtocolReader {
  private buffer = '';
  private contentLength: number | null = null;

  // Parse incoming data and return complete messages
  feed(data: string): Message[] {
    this.buffer += data;
    const messages: Message[] = [];

    while (true) {
      if (this.contentLength === null) {
        // Look for Content-Length header
        const headerEnd = this.buffer.indexOf('\r\n\r\n');
        if (headerEnd === -1) break;

        const headers = this.buffer.slice(0, headerEnd);
        const match = headers.match(/Content-Length:\s*(\d+)/i);
        if (!match) {
          throw new Error('Missing Content-Length header');
        }

        this.contentLength = parseInt(match[1]!, 10);
        this.buffer = this.buffer.slice(headerEnd + 4);
      }

      if (this.buffer.length < this.contentLength) break;

      const content = this.buffer.slice(0, this.contentLength);
      this.buffer = this.buffer.slice(this.contentLength);
      this.contentLength = null;

      try {
        messages.push(JSON.parse(content));
      } catch (e) {
        throw new Error(`Failed to parse JSON: ${e}`);
      }
    }

    return messages;
  }
}

export class ProtocolWriter {
  write(message: Message): string {
    const content = JSON.stringify(message);
    const header = `Content-Length: ${Buffer.byteLength(content, 'utf8')}\r\n\r\n`;
    return header + content;
  }
}

// Helpers for creating responses
export const createResponse = (id: number | string, result: unknown): ResponseMessage => ({
  jsonrpc: '2.0',
  id,
  result,
});

export const createErrorResponse = (
  id: number | string | null,
  code: number,
  message: string,
  data?: unknown
): ResponseMessage => ({
  jsonrpc: '2.0',
  id,
  error: { code, message, data },
});

export const createNotification = (method: string, params?: unknown): NotificationMessage => ({
  jsonrpc: '2.0',
  method,
  params,
});

// Check if a message is a request
export const isRequest = (msg: Message): msg is RequestMessage => {
  return 'id' in msg && 'method' in msg;
};

// Check if a message is a notification
export const isNotification = (msg: Message): msg is NotificationMessage => {
  return !('id' in msg) && 'method' in msg;
};
