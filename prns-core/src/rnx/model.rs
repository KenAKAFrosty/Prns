#[cfg(feature = "alloc")]
use alloc::string::String;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExecutionRequestRef<'a> {
    pub command: &'a str,
    pub timeout_seconds: Option<f64>,
    pub stdout_limit: Option<u64>,
    pub stderr_limit: Option<u64>,
    pub stdin: Option<&'a [u8]>,
}

#[cfg(feature = "alloc")]
#[derive(Debug, Clone, PartialEq)]
pub struct ExecutionRequest {
    pub command: String,
    pub timeout_seconds: Option<f64>,
    pub stdout_limit: Option<u64>,
    pub stderr_limit: Option<u64>,
    pub stdin: Option<Vec<u8>>,
}

#[cfg(feature = "alloc")]
impl From<ExecutionRequestRef<'_>> for ExecutionRequest {
    fn from(request: ExecutionRequestRef<'_>) -> Self {
        Self {
            command: String::from(request.command),
            timeout_seconds: request.timeout_seconds,
            stdout_limit: request.stdout_limit,
            stderr_limit: request.stderr_limit,
            stdin: request.stdin.map(<[u8]>::to_vec),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ExecutionConclusion {
    CompletedAt(f64),
    TimedOut,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExecutedCommandRef<'a> {
    pub return_code: Option<i32>,
    pub stdout: &'a [u8],
    pub stderr: &'a [u8],
    pub total_stdout: u64,
    pub total_stderr: u64,
    pub started_at: f64,
    pub conclusion: ExecutionConclusion,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ExecutionResultRef<'a> {
    NotExecuted { started_at: f64 },
    Executed(ExecutedCommandRef<'a>),
}

#[cfg(feature = "alloc")]
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

#[cfg(feature = "alloc")]
#[derive(Debug, Clone, PartialEq)]
pub enum ExecutionResult {
    NotExecuted { started_at: f64 },
    Executed(ExecutedCommand),
}

#[cfg(feature = "alloc")]
impl ExecutionResult {
    #[must_use]
    pub fn as_ref(&self) -> ExecutionResultRef<'_> {
        match self {
            Self::NotExecuted { started_at } => ExecutionResultRef::NotExecuted {
                started_at: *started_at,
            },
            Self::Executed(executed) => ExecutionResultRef::Executed(ExecutedCommandRef {
                return_code: executed.return_code,
                stdout: &executed.stdout,
                stderr: &executed.stderr,
                total_stdout: executed.total_stdout,
                total_stderr: executed.total_stderr,
                started_at: executed.started_at,
                conclusion: executed.conclusion,
            }),
        }
    }
}
