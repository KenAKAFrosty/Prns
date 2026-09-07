use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::architecture::Adapter;
use crate::toolchain::capture_toolchain_evidence;
use crate::{run_status, BuildError, FirmwareEvidence, LinkOverflowEvidence, ToolchainEvidence};

use super::BuildContext;

pub(crate) struct LinkerMapCapture {
    pending: PathBuf,
    published: PathBuf,
}

pub(crate) enum FirmwareBuildCapture {
    Firmware,
    ResourceReport(ResourceBuildCapture),
}

pub(crate) struct ResourceBuildCapture {
    linker_map: LinkerMapCapture,
    toolchain: ToolchainEvidence,
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
                Ok(FirmwareBuildCapture::ResourceReport(ResourceBuildCapture {
                    linker_map,
                    toolchain,
                }))
            }
        }
    }

    pub fn linker_map_path(&self, target_id: &str) -> Option<PathBuf> {
        self.intent
            .is_resource_report()
            .then(|| self.work_output(target_id).join("linker.map"))
    }

    pub fn cargo_target_directory(&self, target_id: &str) -> Option<PathBuf> {
        self.intent
            .is_resource_report()
            .then(|| self.work_output(target_id).join("cargo"))
    }

    pub fn pending_linker_map_path(&self, target_id: &str) -> Option<PathBuf> {
        self.intent.is_resource_report().then(|| {
            self.work_output(target_id)
                .join(format!("linker.{}.map", self.evidence_run_id))
        })
    }

    pub(crate) fn run_firmware_build(
        &self,
        command: &mut Command,
        label: &str,
        target: &str,
        adapter: &Adapter,
        elf: PathBuf,
        capture: FirmwareBuildCapture,
    ) -> Result<FirmwareEvidence, BuildError> {
        match capture {
            FirmwareBuildCapture::Firmware => {
                run_status(command, label)?;
                Ok(FirmwareEvidence::firmware(elf))
            }
            FirmwareBuildCapture::ResourceReport(capture) => {
                self.run_resource_build(command, label, target, adapter, elf, capture)
            }
        }
    }

    fn run_resource_build(
        &self,
        command: &mut Command,
        label: &str,
        target: &str,
        adapter: &Adapter,
        elf: PathBuf,
        capture: ResourceBuildCapture,
    ) -> Result<FirmwareEvidence, BuildError> {
        command
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::piped());
        let output = command
            .output()
            .map_err(|error| BuildError::Toolchain(format!("failed to run {label}: {error}")))?;
        std::io::stderr()
            .write_all(&output.stderr)
            .map_err(|error| {
                BuildError::Toolchain(format!("could not relay {label} diagnostics: {error}"))
            })?;
        if output.status.success() {
            let linker_map = self.publish_linker_map(capture.linker_map)?;
            return Ok(FirmwareEvidence::resource_report(
                elf,
                linker_map,
                capture.toolchain,
            ));
        }
        let diagnostics = String::from_utf8_lossy(&output.stderr).into_owned();
        let Some(overflows) = adapter.detect_memory_overflow(&diagnostics) else {
            return Err(BuildError::Toolchain(format!(
                "{label} exited with {}",
                output.status
            )));
        };
        let linker_map = self.publish_linker_map(capture.linker_map)?;
        Err(BuildError::LinkOverflow(Box::new(
            LinkOverflowEvidence::new(
                target.to_string(),
                linker_map,
                capture.toolchain,
                overflows,
                diagnostics,
                output.status.to_string(),
            ),
        )))
    }

    fn prepare_linker_map(
        &self,
        target_id: &str,
        adapter: &Adapter,
        command: &mut Command,
    ) -> Result<LinkerMapCapture, BuildError> {
        let output = self.work_output(target_id);
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
}
