use std::process::ExitCode;

pub fn cmd_lsp() -> ExitCode {
    marlang_lsp::run_server();
    ExitCode::SUCCESS
}
