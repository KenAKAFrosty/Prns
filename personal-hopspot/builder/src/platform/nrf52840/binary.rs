use std::fs;
use std::path::Path;
use std::process::Command;

use crate::{llvm_objcopy, run_status, BuildError};

pub(super) fn extract(elf: &Path, output: &Path) -> Result<u64, BuildError> {
    let parent = output.parent().ok_or_else(|| {
        BuildError::Artifact(format!("binary output has no parent: {}", output.display()))
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        BuildError::Artifact(format!(
            "could not create binary output directory {}: {error}",
            parent.display()
        ))
    })?;
    run_status(
        Command::new(llvm_objcopy()?.as_os_str())
            .arg("-O")
            .arg("binary")
            .arg(elf)
            .arg(output),
        "llvm-objcopy",
    )?;
    let bytes = fs::metadata(output)
        .map_err(|error| {
            BuildError::Artifact(format!(
                "could not inspect extracted binary {}: {error}",
                output.display()
            ))
        })?
        .len();
    if bytes == 0 {
        return Err(BuildError::Artifact(format!(
            "extracted binary is empty: {}",
            output.display()
        )));
    }
    Ok(bytes)
}
