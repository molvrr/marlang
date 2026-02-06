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

#[derive(Debug, Clone)]
pub struct StackTraceEntry {
    pub function_name: String,
    pub line: u32,
}

#[derive(Debug, Clone)]
pub struct VMError {
    pub message: String,
    pub line: u32,
    pub stack_trace: Vec<StackTraceEntry>,
}

impl VMError {
    pub fn new(message: impl Into<String>, line: u32, stack_trace: Vec<StackTraceEntry>) -> Self {
        VMError {
            message: message.into(),
            line,
            stack_trace,
        }
    }

    pub fn runtime(message: impl Into<String>) -> Self {
        VMError {
            message: message.into(),
            line: 0,
            stack_trace: Vec::new(),
        }
    }
}

impl fmt::Display for VMError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line > 0 {
            write!(f, "[line {}] Runtime error: {}", self.line, self.message)?;
        } else {
            write!(f, "Runtime error: {}", self.message)?;
        }
        for entry in &self.stack_trace {
            write!(f, "\n  at {} (line {})", entry.function_name, entry.line)?;
        }
        Ok(())
    }
}

impl std::error::Error for VMError {}
