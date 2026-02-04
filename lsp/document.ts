// Document manager for open files

export type Document = {
  uri: string;
  version: number;
  content: string;
};

export class DocumentManager {
  private documents = new Map<string, Document>();

  open(uri: string, version: number, content: string): Document {
    const doc: Document = { uri, version, content };
    this.documents.set(uri, doc);
    return doc;
  }

  update(uri: string, version: number, content: string): Document | undefined {
    const existing = this.documents.get(uri);
    if (existing) {
      existing.version = version;
      existing.content = content;
      return existing;
    }
    return undefined;
  }

  close(uri: string): boolean {
    return this.documents.delete(uri);
  }

  get(uri: string): Document | undefined {
    return this.documents.get(uri);
  }

  all(): Document[] {
    return Array.from(this.documents.values());
  }

  has(uri: string): boolean {
    return this.documents.has(uri);
  }
}

// Convert file:// URI to filesystem path
export const uriToPath = (uri: string): string => {
  if (uri.startsWith('file://')) {
    // Handle file URIs
    let path = uri.slice(7);
    // Decode percent-encoded characters
    path = decodeURIComponent(path);
    // On Windows, remove leading slash before drive letter
    if (path.match(/^\/[a-zA-Z]:/)) {
      path = path.slice(1);
    }
    return path;
  }
  return uri;
};

// Convert filesystem path to file:// URI
export const pathToUri = (path: string): string => {
  // Normalize path separators
  path = path.replace(/\\/g, '/');
  // Add leading slash if not present (for Windows paths)
  if (!path.startsWith('/')) {
    path = '/' + path;
  }
  // Encode special characters
  path = encodeURI(path);
  return 'file://' + path;
};
