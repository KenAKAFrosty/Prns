#![forbid(unsafe_code)]

mod paths;
mod record;
mod service;

pub use paths::{ServicePaths, StateDirectoryError};
pub use record::{LogLane, ServiceRecord, ServiceState};
pub use service::{
    follow, launch_signature, print_recent_log, running, start, stop, stop_and_follow,
    wait_until_ready, LaunchSpec, ManagedProcess, ServiceError, StartOutcome,
};
