use std::path::{Path, PathBuf};

mod host;
mod implementations;
mod rows;
mod schema;

pub use host::{load_host, load_or_create_submitter_id, write_host};
pub use implementations::load_implementations;
pub use rows::{load_all_rows, write_rows};
pub use schema::{
    Axis, Comparability, DeviceId, HostDescriptor, ImplementationDescriptor, ImplementationRole,
    ParticipantDescriptor, ResultRow, Subject, SubmitterId, RESULT_SCHEMA_VERSION,
};

pub fn results_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("BENCHMARK_RESULTS_DIR") {
        return PathBuf::from(path);
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("results")
}
