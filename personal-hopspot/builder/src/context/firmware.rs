use std::path::PathBuf;
use std::process::Command;

use crate::architecture::Adapter;
use crate::toolchain::capture_toolchain_evidence;
use crate::{BuildError, FirmwareEvidence, ToolchainEvidence};

use super::BuildContext;

pub(crate) struct LinkerMapCapture {
    pending: PathBuf,
    published: PathBuf,
}

pub(crate) enum FirmwareBuildCapture {
    Firmware,
    ResourceReport {
        linker_map: LinkerMapCapture,
        toolchain: ToolchainEvidence,
    },
}

impl BuildContext<'_> {
    pub(crate) const fn cargo_subcommand(&self) -> &'static str {
        if self.intent.is_resource_report() {
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
    ) -> Result<FirmwareBuildCapture, BuildError> {
        if let Some(lto) = self.intent.lto().cargo_value() {
            command.env("CARGO_PROFILE_RELEASE_LTO", lto);
        }
        let linker = adapter.configure_cargo(command)?;
        match self.intent {
            crate::BuildIntent::Firmware => Ok(FirmwareBuildCapture::Firmware),
            crate::BuildIntent::ResourceReport { .. } => {
                let toolchain = capture_toolchain_evidence(command, adapter, &linker)?;
                let linker_map = self.prepare_linker_map(target_id, adapter, command)?;
                Ok(FirmwareBuildCapture::ResourceReport {
                    linker_map,
                    toolchain,
                })
            }
        }
    }

    pub fn linker_map_path(&self, target_id: &str) -> Option<PathBuf> {
        self.intent
            .is_resource_report()
            .then(|| self.evidence_work_output(target_id).join("linker.map"))
    }

    pub fn cargo_target_directory(&self, target_id: &str) -> Option<PathBuf> {
        self.intent
            .is_resource_report()
            .then(|| self.evidence_work_output(target_id).join("cargo"))
    }

    pub fn pending_linker_map_path(&self, target_id: &str) -> Option<PathBuf> {
        self.intent.is_resource_report().then(|| {
            self.evidence_work_output(target_id)
                .join(format!("linker.{}.map", self.evidence_run_id))
        })
    }

    pub(crate) fn finish_firmware_build(
        &self,
        elf: PathBuf,
        capture: FirmwareBuildCapture,
    ) -> Result<FirmwareEvidence, BuildError> {
        match capture {
            FirmwareBuildCapture::Firmware => Ok(FirmwareEvidence::firmware(elf)),
            FirmwareBuildCapture::ResourceReport {
                linker_map,
                toolchain,
            } => {
                let linker_map = self.publish_linker_map(linker_map)?;
                Ok(FirmwareEvidence::resource_report(
                    elf, linker_map, toolchain,
                ))
            }
        }
    }

    fn prepare_linker_map(
        &self,
        target_id: &str,
        adapter: &Adapter,
        command: &mut Command,
    ) -> Result<LinkerMapCapture, BuildError> {
        let output = self.evidence_work_output(target_id);
        let published = output.join("linker.map");
        let pending = output.join(format!("linker.{}.map", self.evidence_run_id));
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
        Ok(LinkerMapCapture { pending, published })
    }

    fn publish_linker_map(&self, capture: LinkerMapCapture) -> Result<PathBuf, BuildError> {
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
        Ok(capture.published)
    }

    fn evidence_work_output(&self, target_id: &str) -> PathBuf {
        self.configured_output_root().join("work").join(target_id)
    }
}
