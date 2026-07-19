use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::results_dir;
use super::schema::{HostDescriptor, SubmitterId};

fn host_path(host: &str) -> PathBuf {
    results_dir().join(host).join("host.json")
}

pub fn write_host(descriptor: &HostDescriptor) {
    let path = host_path(&descriptor.host);
    std::fs::create_dir_all(path.parent().expect("host dir")).expect("create host dir");
    let body = serde_json::to_string_pretty(descriptor).expect("serialize host descriptor");
    std::fs::write(&path, body + "\n").unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

pub fn load_host(host: &str) -> Option<HostDescriptor> {
    let text = std::fs::read_to_string(host_path(host)).ok()?;
    serde_json::from_str(&text).ok()
}

#[derive(Serialize, Deserialize)]
struct SubmitterIdentity {
    submitter_id: SubmitterId,
}

fn submitter_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(".submitter.json")
}

pub fn load_or_create_submitter_id() -> SubmitterId {
    let path = submitter_path();
    if let Some(identity) = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<SubmitterIdentity>(&text).ok())
    {
        return identity.submitter_id;
    }
    let submitter_id = SubmitterId::generate();
    let body = serde_json::to_string_pretty(&SubmitterIdentity { submitter_id })
        .expect("serialize submitter identity");
    std::fs::write(&path, body + "\n").unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
    submitter_id
}
