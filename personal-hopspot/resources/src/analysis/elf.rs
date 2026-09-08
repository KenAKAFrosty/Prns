use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use object::{BinaryFormat, Object, ObjectSection, SectionFlags};
use personal_hopspot_memory::AddressRange;
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SectionKind {
    Code,
    ReadOnlyData,
    InitializedData,
    ZeroFill,
    Other,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct AllocatedSection {
    name: String,
    kind: SectionKind,
    run_range: AddressRange,
    load_bytes: u64,
    alignment: u64,
}

impl AllocatedSection {
    pub(super) fn from_parts(
        name: String,
        kind: SectionKind,
        run_range: AddressRange,
        load_bytes: u64,
        alignment: u64,
    ) -> Self {
        Self {
            name,
            kind,
            run_range,
            load_bytes,
            alignment,
        }
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) const fn kind(&self) -> SectionKind {
        self.kind
    }

    pub(crate) const fn run_range(&self) -> AddressRange {
        self.run_range
    }

    pub(crate) const fn load_bytes(&self) -> u64 {
        self.load_bytes
    }

    pub(crate) const fn alignment(&self) -> u64 {
        self.alignment
    }
}

#[derive(Debug, Error)]
pub(crate) enum AnalysisError {
    #[error("could not read firmware ELF {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not parse firmware ELF {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: object::Error,
    },
    #[error("firmware object {path} is {format:?}, not ELF")]
    NonElf { path: PathBuf, format: BinaryFormat },
    #[error("could not read section name at index {index} in {path}: {source}")]
    SectionName {
        path: PathBuf,
        index: usize,
        #[source]
        source: object::Error,
    },
    #[error("allocated section {section:?} has invalid range {start:#x} + {bytes}")]
    InvalidSectionRange {
        section: String,
        start: u64,
        bytes: u64,
    },
    #[error("firmware ELF {path} contains no allocated sections")]
    MissingAllocatedSections { path: PathBuf },
}

pub(crate) fn read_allocated_sections(path: &Path) -> Result<Vec<AllocatedSection>, AnalysisError> {
    let bytes = fs::read(path).map_err(|source| AnalysisError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    allocated_sections(path, &bytes)
}

fn allocated_sections(path: &Path, bytes: &[u8]) -> Result<Vec<AllocatedSection>, AnalysisError> {
    let object = object::File::parse(bytes).map_err(|source| AnalysisError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    if object.format() != BinaryFormat::Elf {
        return Err(AnalysisError::NonElf {
            path: path.to_path_buf(),
            format: object.format(),
        });
    }

    let sections = object
        .sections()
        .filter(|section| section.size() != 0 && is_allocated(section.flags()))
        .map(|section| {
            let name = section
                .name()
                .map_err(|source| AnalysisError::SectionName {
                    path: path.to_path_buf(),
                    index: section.index().0,
                    source,
                })?
                .to_string();
            let run_address = section.address();
            let run_bytes = section.size();
            let run_range =
                AddressRange::from_start_and_size(run_address, run_bytes).map_err(|_| {
                    AnalysisError::InvalidSectionRange {
                        section: name.clone(),
                        start: run_address,
                        bytes: run_bytes,
                    }
                })?;
            let load_bytes = section.file_range().map_or(0, |(_, bytes)| bytes);
            Ok(AllocatedSection::from_parts(
                name,
                section_kind(section.kind()),
                run_range,
                load_bytes,
                section.align(),
            ))
        })
        .collect::<Result<Vec<_>, AnalysisError>>()?;
    if sections.is_empty() {
        return Err(AnalysisError::MissingAllocatedSections {
            path: path.to_path_buf(),
        });
    }
    Ok(sections)
}

fn is_allocated(flags: SectionFlags) -> bool {
    matches!(
        flags,
        SectionFlags::Elf { sh_flags } if sh_flags & u64::from(object::elf::SHF_ALLOC) != 0
    )
}

const fn section_kind(kind: object::SectionKind) -> SectionKind {
    match kind {
        object::SectionKind::Text => SectionKind::Code,
        object::SectionKind::ReadOnlyData | object::SectionKind::ReadOnlyString => {
            SectionKind::ReadOnlyData
        }
        object::SectionKind::Data | object::SectionKind::Tls => SectionKind::InitializedData,
        object::SectionKind::UninitializedData
        | object::SectionKind::UninitializedTls
        | object::SectionKind::Common => SectionKind::ZeroFill,
        _ => SectionKind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_elf_is_rejected() {
        assert!(matches!(
            allocated_sections(Path::new("firmware.elf"), b"not an elf"),
            Err(AnalysisError::Parse { .. })
        ));
    }

    #[test]
    fn section_kinds_have_stable_resource_categories() {
        assert_eq!(section_kind(object::SectionKind::Text), SectionKind::Code);
        assert_eq!(
            section_kind(object::SectionKind::ReadOnlyData),
            SectionKind::ReadOnlyData
        );
        assert_eq!(
            section_kind(object::SectionKind::Data),
            SectionKind::InitializedData
        );
        assert_eq!(
            section_kind(object::SectionKind::UninitializedData),
            SectionKind::ZeroFill
        );
        assert_eq!(
            section_kind(object::SectionKind::Elf(0x7000_0000)),
            SectionKind::Other
        );
    }

    #[test]
    fn only_elf_allocations_are_selected() {
        assert!(is_allocated(SectionFlags::Elf {
            sh_flags: u64::from(object::elf::SHF_ALLOC)
        }));
        assert!(!is_allocated(SectionFlags::Elf { sh_flags: 0 }));
        assert!(!is_allocated(SectionFlags::None));
    }
}
