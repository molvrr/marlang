use dashmap::DashMap;
use tower_lsp::lsp_types::Url;

/// Tracks the state of all open documents in the editor.
pub struct DocumentState {
    documents: DashMap<Url, DocumentData>,
}

struct DocumentData {
    source: String,
    version: i32,
}

impl DocumentState {
    pub fn new() -> Self {
        Self {
            documents: DashMap::new(),
        }
    }

    /// Record a newly opened document.
    pub fn open(&self, uri: Url, text: String, version: i32) {
        self.documents.insert(
            uri,
            DocumentData {
                source: text,
                version,
            },
        );
    }

    /// Update the full text of an open document.
    pub fn update(&self, uri: &Url, text: String, version: i32) {
        if let Some(mut entry) = self.documents.get_mut(uri) {
            entry.source = text;
            entry.version = version;
        }
    }

    /// Remove a closed document.
    pub fn close(&self, uri: &Url) {
        self.documents.remove(uri);
    }

    /// Get a snapshot of the document's source text and version.
    pub fn get(&self, uri: &Url) -> Option<(String, i32)> {
        self.documents
            .get(uri)
            .map(|entry| (entry.source.clone(), entry.version))
    }
}
