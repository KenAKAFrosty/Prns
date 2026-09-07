use std::path::{Path, PathBuf};

use crate::architecture::MemoryOverflows;

#[derive(Clone, Debug)]
pub struct FirmwareEvidence {
    elf: PathBuf,
    kind: FirmwareEvidenceKind,
}

#[derive(Clone, Debug)]
enum FirmwareEvidenceKind {
    Firmware,
    ResourceReport(ResourceBuildEvidence),
}

#[derive(Clone, Debug)]
pub struct ResourceBuildEvidence {
    linker_map: PathBuf,
    toolchain: ToolchainEvidence,
}

#[derive(Debug)]
pub struct LinkOverflowEvidence {
    target: String,
    linker_map: PathBuf,
    toolchain: ToolchainEvidence,
    overflows: MemoryOverflows,
    diagnostics: String,
    exit_status: String,
}

impl FirmwareEvidence {
    pub(crate) const fn firmware(elf: PathBuf) -> Self {
        Self {
            elf,
            kind: FirmwareEvidenceKind::Firmware,
        }
    }

    pub(crate) const fn resource_report(
        elf: PathBuf,
        linker_map: PathBuf,
        toolchain: ToolchainEvidence,
    ) -> Self {
        Self {
            elf,
            kind: FirmwareEvidenceKind::ResourceReport(ResourceBuildEvidence {
                linker_map,
                toolchain,
            }),
        }
    }

    #[must_use]
    pub fn elf(&self) -> &Path {
        &self.elf
    }

    #[must_use]
    pub const fn resource_build(&self) -> Option<&ResourceBuildEvidence> {
        match &self.kind {
            FirmwareEvidenceKind::Firmware => None,
            FirmwareEvidenceKind::ResourceReport(evidence) => Some(evidence),
        }
    }
}

impl ResourceBuildEvidence {
    #[must_use]
    pub fn linker_map(&self) -> &Path {
        &self.linker_map
    }

    #[must_use]
    pub const fn toolchain(&self) -> &ToolchainEvidence {
        &self.toolchain
    }
}

impl LinkOverflowEvidence {
    pub(crate) const fn new(
        target: String,
        linker_map: PathBuf,
        toolchain: ToolchainEvidence,
        overflows: MemoryOverflows,
        diagnostics: String,
        exit_status: String,
    ) -> Self {
        Self {
            target,
            linker_map,
            toolchain,
            overflows,
            diagnostics,
            exit_status,
        }
    }

    #[must_use]
    pub fn target(&self) -> &str {
        &self.target
    }

    #[must_use]
    pub fn linker_map(&self) -> &Path {
        &self.linker_map
    }

    #[must_use]
    pub const fn toolchain(&self) -> &ToolchainEvidence {
        &self.toolchain
    }

    #[must_use]
    pub const fn overflows(&self) -> &MemoryOverflows {
        &self.overflows
    }

    #[must_use]
    pub fn diagnostics(&self) -> &str {
        &self.diagnostics
    }

    #[must_use]
    pub fn exit_status(&self) -> &str {
        &self.exit_status
    }
}

impl std::fmt::Display for LinkOverflowEvidence {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let primary = self.overflows.primary();
        write!(
            formatter,
            "{} firmware link overflowed region {:?} by {} bytes",
            self.target,
            primary.region(),
            primary.bytes()
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolchainEvidence {
    rustc_version: String,
    cargo_version: String,
    linker_version: String,
}

impl ToolchainEvidence {
    pub(crate) const fn new(
        rustc_version: String,
        cargo_version: String,
        linker_version: String,
    ) -> Self {
        Self {
            rustc_version,
            cargo_version,
            linker_version,
        }
    }

    #[must_use]
    pub fn rustc_version(&self) -> &str {
        &self.rustc_version
    }

    #[must_use]
    pub fn cargo_version(&self) -> &str {
        &self.cargo_version
    }

    #[must_use]
    pub fn linker_version(&self) -> &str {
        &self.linker_version
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firmware_and_resource_report_evidence_are_distinct() {
        let firmware = FirmwareEvidence::firmware(PathBuf::from("firmware.elf"));
        assert!(firmware.resource_build().is_none());

        let resource = FirmwareEvidence::resource_report(
            PathBuf::from("firmware.elf"),
            PathBuf::from("linker.map"),
            ToolchainEvidence::new(
                "rustc version".to_string(),
                "cargo version".to_string(),
                "linker version".to_string(),
            ),
        );
        let resource = resource.resource_build().expect("resource evidence");
        assert_eq!(resource.linker_map(), Path::new("linker.map"));
        assert_eq!(resource.toolchain().rustc_version(), "rustc version");
    }
}
