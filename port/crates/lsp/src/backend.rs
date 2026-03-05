use crate::diagnostics::check_document;
use crate::document::DocumentState;
use dashmap::DashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

const DEBOUNCE_MS: u64 = 300;

/// Per-document debounce state. We cancel the previous check when a new change arrives.
struct PendingCheck {
    /// Handle to the spawned debounce task so we can abort it.
    handle: JoinHandle<()>,
    /// Notify used to cancel the sleep inside the debounce task.
    cancel: Arc<Notify>,
}

pub struct MarlangBackend {
    client: Client,
    documents: DocumentState,
    /// Tracks in-flight debounced checks per document URI.
    pending: DashMap<Url, PendingCheck>,
}

impl MarlangBackend {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            documents: DocumentState::new(),
            pending: DashMap::new(),
        }
    }

    /// Schedule a debounced diagnostics check for the given document.
    fn schedule_check(&self, uri: Url, source: String, version: i32) {
        // Cancel any existing pending check for this URI
        if let Some((_, prev)) = self.pending.remove(&uri) {
            prev.cancel.notify_one();
            prev.handle.abort();
        }

        let client = self.client.clone();
        let check_uri = uri.clone();
        let cancel = Arc::new(Notify::new());
        let cancel_clone = cancel.clone();

        let handle = tokio::spawn(async move {
            // Wait for the debounce period, but bail if cancelled
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(DEBOUNCE_MS)) => {}
                _ = cancel_clone.notified() => return,
            }

            // Run the check on a blocking thread (the pipeline is synchronous)
            let file_path = uri_to_path(&check_uri);
            let diagnostics =
                tokio::task::spawn_blocking(move || check_document(&source, file_path.as_deref()))
                    .await
                    .unwrap_or_default();

            client
                .publish_diagnostics(check_uri, diagnostics, Some(version))
                .await;
        });

        self.pending.insert(uri, PendingCheck { handle, cancel });
    }

    /// Run diagnostics immediately (no debounce), used for did_open and did_save.
    fn check_now(&self, uri: Url, source: String, version: i32) {
        // Cancel any pending debounced check
        if let Some((_, prev)) = self.pending.remove(&uri) {
            prev.cancel.notify_one();
            prev.handle.abort();
        }

        let client = self.client.clone();
        let check_uri = uri.clone();

        tokio::spawn(async move {
            let file_path = uri_to_path(&check_uri);
            let diagnostics =
                tokio::task::spawn_blocking(move || check_document(&source, file_path.as_deref()))
                    .await
                    .unwrap_or_default();

            client
                .publish_diagnostics(check_uri, diagnostics, Some(version))
                .await;
        });
    }
}

/// Convert a file:// URI to a filesystem path.
fn uri_to_path(uri: &Url) -> Option<PathBuf> {
    uri.to_file_path().ok()
}

#[tower_lsp::async_trait]
impl LanguageServer for MarlangBackend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::FULL),
                        save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                            include_text: Some(true),
                        })),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "marlang-lsp".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
            offset_encoding: None,
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        tracing::info!("Marlang LSP server initialized");
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = params.text_document.text;
        let version = params.text_document.version;

        self.documents.open(uri.clone(), text.clone(), version);
        self.check_now(uri, text, version);
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let version = params.text_document.version;

        // We use full sync, so there's exactly one change with the full text
        if let Some(change) = params.content_changes.into_iter().next() {
            self.documents.update(&uri, change.text.clone(), version);
            self.schedule_check(uri, change.text, version);
        }
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri;
        // Re-check on save (picks up changes to imported modules on disk)
        if let Some((source, version)) = self.documents.get(&uri) {
            self.check_now(uri, source, version);
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.documents.close(&uri);

        // Cancel any pending check
        if let Some((_, prev)) = self.pending.remove(&uri) {
            prev.cancel.notify_one();
            prev.handle.abort();
        }

        // Clear diagnostics for the closed document
        self.client.publish_diagnostics(uri, vec![], None).await;
    }
}
