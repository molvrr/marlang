use clap::{Parser, Subcommand};
use marlang_cli::cmd;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "mar", about = "The Marlang programming language", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run a Marlang file
    Run {
        /// Path to the .mar file
        file: PathBuf,
    },
    /// Start interactive REPL
    Repl,
    /// Type check a file without running
    Typecheck {
        /// Path to the .mar file
        file: PathBuf,
    },
    /// Disassemble bytecode
    Disasm {
        /// Path to the .mar file
        file: PathBuf,
    },
    /// Show macro-expanded source
    Expand {
        /// Path to the .mar file
        file: PathBuf,
        /// Strip hygiene marks for cleaner output
        #[arg(long)]
        clean: bool,
    },
    /// Start Language Server Protocol server
    Lsp,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Command::Run { file } => cmd::run::cmd_run(&file),
        Command::Repl => cmd::repl::cmd_repl(),
        Command::Typecheck { file } => cmd::typecheck::cmd_typecheck(&file),
        Command::Disasm { file } => cmd::disasm::cmd_disasm(&file),
        Command::Expand { file, clean } => cmd::expand::cmd_expand(&file, clean),
        Command::Lsp => cmd::lsp::cmd_lsp(),
    }
}
