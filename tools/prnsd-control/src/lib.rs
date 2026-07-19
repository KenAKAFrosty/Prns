#![forbid(unsafe_code)]

mod error;
mod logs;
mod paths;
mod process;
mod record;
mod state;

pub use error::ServiceError;
pub use logs::{follow, print_recent_log, stop_and_follow};
pub use paths::{ServicePaths, StateDirectoryError};
pub use process::{
    launch_signature, running, start, stop, wait_until_ready, LaunchSpec, ManagedProcess,
    StartOutcome,
};
pub use record::{LogLane, ServiceRecord, ServiceState};
