use alloc::string::String;
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq)]
pub struct ExecutionRequest {
    pub command: String,
    pub timeout_seconds: Option<f64>,
    pub stdout_limit: Option<u64>,
    pub stderr_limit: Option<u64>,
    pub stdin: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ExecutionConclusion {
    CompletedAt(f64),
    TimedOut,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExecutedCommand {
    pub return_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub total_stdout: u64,
    pub total_stderr: u64,
    pub started_at: f64,
    pub conclusion: ExecutionConclusion,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExecutionResult {
    NotExecuted { started_at: f64 },
    Executed(ExecutedCommand),
}
