use std::path::{Path, PathBuf};

use super::schema::ImplementationDescriptor;

fn implementations_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("implementations")
}

pub fn load_implementations() -> Vec<ImplementationDescriptor> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(implementations_dir()) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Some(descriptor) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
        {
            out.push(descriptor);
        }
    }
    out
}
