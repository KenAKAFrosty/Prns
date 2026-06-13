//! The benchmark substrate the live-scenario harness stands on: the result-row schema
//! and its on-disk layout (`results`), plus the scenario locator. A scenario is a
//! versioned *traffic profile* on disk (`scenarios/<name>/manifest.json`); a run is an
//! assignment of implementations to its roles, carried out by the `orchestrate` bin
//! spawning one `scenario_node`-shaped participation binary per role. The corpus-replay
//! workloads that used to live here retired with the announce-energy scenario; the
//! engine-direct microbenches return as targeted bottleneck microscopes once the
//! scenario numbers point somewhere.

use std::path::{Path, PathBuf};

mod energy;
pub mod microscope;
mod results;
pub use energy::{unavailable_hint as energy_unavailable_hint, PowerMeter};
pub use results::{
    load_all_rows, load_host, load_implementations, load_or_create_submitter_id, results_dir,
    write_host, write_rows, Axis, Comparability, DeviceId, HostDescriptor,
    ImplementationDescriptor, ImplementationRole, ResultRow, SubmitterId,
};

/// The on-disk home of scenario `name` (e.g. "link-firehose"), relative to this crate.
pub fn scenario_dir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("scenarios")
        .join(name)
}
