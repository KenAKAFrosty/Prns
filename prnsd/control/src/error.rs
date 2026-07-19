use std::fmt;
use std::io;
use std::path::PathBuf;

#[derive(Debug)]
pub enum ServiceError {
    Io {
        operation: &'static str,
        source: io::Error,
    },
    ControlBusy,
    InvalidRecord,
    IncompleteRecord,
    InvalidManagedEnvironment,
    ManagedInstanceAlreadyRunning,
    ProcessExited {
        log: PathBuf,
    },
    StartupTimedOut {
        pid: u32,
        log: PathBuf,
    },
    StopTimedOut {
        pid: u32,
    },
}

impl fmt::Display for ServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { operation, source } => write!(formatter, "{operation}: {source}"),
            Self::ControlBusy => {
                formatter.write_str("another prnsd lifecycle command is still in progress")
            }
            Self::InvalidRecord => formatter.write_str("the prnsd session record is invalid"),
            Self::IncompleteRecord => {
                formatter.write_str("prnsd is running without a complete session record")
            }
            Self::InvalidManagedEnvironment => {
                formatter.write_str("the internal prnsd managed environment is invalid")
            }
            Self::ManagedInstanceAlreadyRunning => {
                formatter.write_str("another managed prnsd instance already owns the session")
            }
            Self::ProcessExited { log } => write!(
                formatter,
                "prnsd exited during startup; inspect {}",
                log.display()
            ),
            Self::StartupTimedOut { pid, log } => write!(
                formatter,
                "prnsd process {pid} is still starting after 30 seconds; inspect {}",
                log.display()
            ),
            Self::StopTimedOut { pid } => write!(
                formatter,
                "prnsd process {pid} did not stop within 30 seconds"
            ),
        }
    }
}

impl std::error::Error for ServiceError {}
