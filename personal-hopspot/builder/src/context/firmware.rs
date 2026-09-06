use std::path::PathBuf;
use std::process::Command;

use crate::architecture::Adapter;
use crate::BuildError;

use super::BuildContext;

pub(crate) struct LinkerMapCapture {
    pending: PathBuf,
    published: PathBuf,
}

impl BuildContext<'_> {
    pub(crate) const fn cargo_subcommand(&self) -> &'static str {
        if self.configuration.captures_linker_map() {
            "rustc"
        } else {
            "build"
        }
    }

    pub(crate) fn configure_firmware_cargo(
        &self,
        target_id: &str,
        adapter: &Adapter,
        command: &mut Command,
    ) -> Result<Option<LinkerMapCapture>, BuildError> {
        if let Some(lto) = self.configuration.lto().cargo_value() {
            command.env("CARGO_PROFILE_RELEASE_LTO", lto);
        }
        adapter.configure_cargo(command)?;
        let Some(published) = self.linker_map_path(target_id) else {
            return Ok(None);
        };
        let pending = self.pending_linker_map_path(target_id).ok_or_else(|| {
            BuildError::Artifact(format!("missing pending linker map path for {target_id:?}"))
        })?;
        let parent = pending.parent().ok_or_else(|| {
            BuildError::Artifact(format!(
                "linker map path {} has no parent",
                pending.display()
            ))
        })?;
        std::fs::create_dir_all(parent).map_err(|error| {
            BuildError::Artifact(format!(
                "could not create linker map directory {}: {error}",
                parent.display()
            ))
        })?;
        command
            .arg("--")
            .arg("-C")
            .arg(adapter.linker_map_argument(&pending));
        Ok(Some(LinkerMapCapture { pending, published }))
    }

    pub fn linker_map_path(&self, target_id: &str) -> Option<PathBuf> {
        self.configuration
            .captures_linker_map()
            .then(|| self.evidence_work_output(target_id).join("linker.map"))
    }

    pub fn cargo_target_directory(&self, target_id: &str) -> Option<PathBuf> {
        self.configuration
            .isolates_artifacts()
            .then(|| self.evidence_work_output(target_id).join("cargo"))
    }

    pub fn pending_linker_map_path(&self, target_id: &str) -> Option<PathBuf> {
        self.configuration.captures_linker_map().then(|| {
            self.evidence_work_output(target_id)
                .join(format!("linker.{}.map", self.evidence_run_id))
        })
    }

    pub(crate) fn publish_linker_map(
        &self,
        capture: Option<LinkerMapCapture>,
    ) -> Result<Option<PathBuf>, BuildError> {
        let Some(capture) = capture else {
            return Ok(None);
        };
        let metadata = std::fs::metadata(&capture.pending).map_err(|error| {
            BuildError::Artifact(format!(
                "linker did not produce map {}: {error}",
                capture.pending.display()
            ))
        })?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err(BuildError::Artifact(format!(
                "linker produced an empty or invalid map at {}",
                capture.pending.display()
            )));
        }
        match std::fs::remove_file(&capture.published) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(BuildError::Artifact(format!(
                    "could not replace linker map {}: {error}",
                    capture.published.display()
                )));
            }
        }
        std::fs::rename(&capture.pending, &capture.published).map_err(|error| {
            BuildError::Artifact(format!(
                "could not publish linker map {}: {error}",
                capture.published.display()
            ))
        })?;
        Ok(Some(capture.published))
    }

    fn evidence_work_output(&self, target_id: &str) -> PathBuf {
        self.build_output_root().join("work").join(target_id)
    }
}
