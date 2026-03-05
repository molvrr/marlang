use marlang_core::intern::Interner;
use marlang_vm::builtins::register_builtins;
use marlang_vm::modules::{ModuleLoader, run_program};
use marlang_vm::value::format_value;
use marlang_vm::vm::VM;
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;
use std::path::PathBuf;
use std::process::ExitCode;

pub fn cmd_repl() -> ExitCode {
    println!("Marlang REPL v{}", env!("CARGO_PKG_VERSION"));
    println!("Type .exit or Ctrl+D to exit\n");

    let mut rl = match DefaultEditor::new() {
        Ok(editor) => editor,
        Err(e) => {
            eprintln!("Failed to initialize REPL: {}", e);
            return ExitCode::FAILURE;
        }
    };

    // Try to load history
    let history_path = dirs::home_dir()
        .map(|h| h.join(".marlang_history"))
        .unwrap_or_else(|| PathBuf::from(".marlang_history"));
    let _ = rl.load_history(&history_path);

    let interner = Interner::new();
    let mut vm = VM::new();
    register_builtins(&mut vm);
    let mut loader = ModuleLoader::new();

    loop {
        match rl.readline("mar> ") {
            Ok(line) => {
                let trimmed = line.trim();

                // Handle special commands
                if trimmed == ".exit" || trimmed == "(quit)" {
                    println!("Goodbye!");
                    break;
                }

                if trimmed.is_empty() {
                    continue;
                }

                let _ = rl.add_history_entry(&line);

                // Run the expression
                match run_program(&mut vm, &line, None, &mut loader, &interner) {
                    Ok(value) => {
                        println!("=> {}", format_value(&value));
                    }
                    Err(e) => {
                        eprintln!("Error: {}", e);
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                println!("^C");
                continue;
            }
            Err(ReadlineError::Eof) => {
                println!("Goodbye!");
                break;
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                break;
            }
        }
    }

    // Save history
    let _ = rl.save_history(&history_path);

    ExitCode::SUCCESS
}
