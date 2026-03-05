mod backend;
mod definition;
mod diagnostics;
mod document;
mod hover;

use crate::backend::MarlangBackend;
use tower_lsp::{LspService, Server};

/// Start the Marlang LSP server on stdin/stdout.
///
/// This function creates a tokio runtime and blocks until the client disconnects.
pub fn run_server() {
    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    rt.block_on(async {
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();

        let (service, socket) = LspService::new(MarlangBackend::new);
        Server::new(stdin, stdout, socket).serve(service).await;
    });
}
