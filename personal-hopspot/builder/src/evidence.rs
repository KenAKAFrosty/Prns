use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct FirmwareEvidence {
    elf: PathBuf,
    linker_map: Option<PathBuf>,
    toolchain: Option<ToolchainEvidence>,
}

impl FirmwareEvidence {
    pub(crate) const fn new(
        elf: PathBuf,
        linker_map: Option<PathBuf>,
        toolchain: Option<ToolchainEvidence>,
    ) -> Self {
        Self {
            elf,
            linker_map,
            toolchain,
        }
    }

    #[must_use]
    pub fn elf(&self) -> &Path {
        &self.elf
    }

    #[must_use]
    pub fn linker_map(&self) -> Option<&Path> {
        self.linker_map.as_deref()
    }

    #[must_use]
    pub const fn toolchain(&self) -> Option<&ToolchainEvidence> {
        self.toolchain.as_ref()
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
