#![forbid(unsafe_code)]

mod active_config;
mod error;
mod logs;
mod paths;
mod process;
mod record;
mod reload;
mod state;

pub use error::ServiceError;
pub use logs::{follow, print_recent_log, stop_and_follow};
pub use paths::{ServicePaths, StateDirectoryError};
pub use process::{
    active_config_dir, launch_signature, running, start, stop, wait_until_ready, ForegroundSpec,
    LaunchSpec, ManagedProcess, StartOutcome,
};
pub use record::{LogLane, ServiceKind, ServiceRecord, ServiceState};
pub use reload::{config_digest, request_reload, ReloadRequest, ReloadResult};
