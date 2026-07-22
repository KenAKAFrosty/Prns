use std::path::{Path, PathBuf};

mod catalog;
mod energy;
pub mod microscope;
mod results;
pub use catalog::{
    deterministic_payload, load_catalog, load_manifest, scenarios_dir, CatalogError,
    ConformanceRule, ScenarioId, ScenarioManifest, SizeSequence, WorkloadProfile,
    DEFAULT_SIZE_SEED, IMPLEMENTATIONS,
};
pub use energy::{unavailable_hint as energy_unavailable_hint, PowerMeter};
pub use results::{
    load_all_rows, load_host, load_implementations, load_or_create_submitter_id, results_dir,
    write_host, write_rows, Axis, Comparability, DeviceId, HostDescriptor,
    ImplementationDescriptor, ImplementationRole, ParticipantDescriptor, ResultRow, Subject,
    SubmitterId, RESULT_SCHEMA_VERSION,
};

pub fn scenario_dir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("scenarios")
        .join(name)
}
