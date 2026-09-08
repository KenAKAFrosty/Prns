use std::fs;
use std::path::Path;

use crate::BuildError;

pub fn publish(path: &Path, bytes: &[u8]) -> Result<(), BuildError> {
    let parent = path
        .parent()
        .ok_or_else(|| BuildError::Artifact(format!("path has no parent: {}", path.display())))?;
    fs::create_dir_all(parent).map_err(|error| {
        BuildError::Artifact(format!("could not create {}: {error}", parent.display()))
    })?;
    let temporary = path.with_extension(format!("part-{}", std::process::id()));
    fs::write(&temporary, bytes).map_err(|error| {
        BuildError::Artifact(format!("could not write {}: {error}", temporary.display()))
    })?;
    fs::rename(&temporary, path).map_err(|error| {
        BuildError::Artifact(format!("could not publish {}: {error}", path.display()))
    })
}
