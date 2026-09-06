use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct FirmwareEvidence {
    elf: PathBuf,
    linker_map: Option<PathBuf>,
}

impl FirmwareEvidence {
    pub(crate) const fn new(elf: PathBuf, linker_map: Option<PathBuf>) -> Self {
        Self { elf, linker_map }
    }

    #[must_use]
    pub fn elf(&self) -> &Path {
        &self.elf
    }

    #[must_use]
    pub fn linker_map(&self) -> Option<&Path> {
        self.linker_map.as_deref()
    }
}
