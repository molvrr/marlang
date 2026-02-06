use std::fmt;

#[derive(Debug, Clone)]
pub struct CompileError {
    pub message: String,
    pub line: u32,
}

impl CompileError {
    pub fn new(message: impl Into<String>, line: u32) -> Self {
        CompileError {
            message: message.into(),
            line,
        }
    }
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line > 0 {
            write!(f, "[line {}] Compile error: {}", self.line, self.message)
        } else {
            write!(f, "Compile error: {}", self.message)
        }
    }
}

impl std::error::Error for CompileError {}
