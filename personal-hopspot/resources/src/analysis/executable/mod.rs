mod architecture;
mod coverage;
mod disassembly;
mod functions;
mod stack;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use object::read::elf::{ElfFile32, ProgramHeader};
use object::{BinaryFormat, Endianness, Object, ObjectSection, SectionFlags};
use personal_hopspot_builder::architecture::{Adapter, DisassemblerFlavor};
use personal_hopspot_memory::{AddressRange, MemoryProfile, ProcessorArchitecture};
use thiserror::Error;

pub(crate) use functions::{display_symbol, FunctionAnalysis, FunctionBoundary};
pub(crate) use stack::{
    KnownCallPath, StackAnalysis, StackAnalysisGapKind, StackFrame, StackLimitAnalysis, StackRoot,
    StackRootRole,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ByteOrder {
    Little,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LoadPermission {
    Read,
    Write,
    Execute,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StartupAnchorRole {
    EntryPoint,
    InitialStackPointer,
    ResetVector,
    TrapVector,
    ExceptionVectors,
}

#[derive(Debug)]
pub(crate) struct ExecutableAnalysis {
    pub(crate) architecture: ProcessorArchitecture,
    pub(crate) byte_order: ByteOrder,
    pub(crate) entry_point: u64,
    pub(crate) load_segments: Vec<LoadSegment>,
    pub(crate) executable_sections: Vec<ExecutableSection>,
    pub(crate) startup: StartupStructure,
    pub(crate) functions: FunctionAnalysis,
    pub(crate) disassembly: DisassemblyAnalysis,
    pub(crate) stack: StackAnalysis,
}

#[derive(Debug)]
pub(crate) struct LoadSegment {
    pub(crate) file_offset: u64,
    pub(crate) run_range: AddressRange,
    pub(crate) load_range: AddressRange,
    pub(crate) file_bytes: u64,
    pub(crate) memory_bytes: u64,
    pub(crate) alignment: u64,
    pub(crate) permissions: Vec<LoadPermission>,
}

#[derive(Debug)]
pub(crate) struct ExecutableSection {
    pub(crate) name: String,
    pub(crate) range: AddressRange,
    pub(crate) alignment: u64,
    pub(crate) fingerprint: String,
    data: Vec<u8>,
}

#[derive(Debug)]
pub(crate) struct StartupStructure {
    pub(crate) entry_section: String,
    pub(crate) entry_symbol: String,
    pub(crate) anchors: Vec<StartupAnchor>,
}

#[derive(Debug)]
pub(crate) struct StartupAnchor {
    pub(crate) role: StartupAnchorRole,
    pub(crate) address: u64,
    pub(crate) section: String,
}

#[derive(Debug)]
pub(crate) struct DisassemblyAnalysis {
    pub(crate) adapter: ProcessorArchitecture,
    pub(crate) flavor: DisassemblerFlavor,
    pub(crate) program: &'static str,
    pub(crate) version: String,
    pub(crate) executable_bytes: u64,
    pub(crate) decoded_bytes: u64,
    pub(crate) undecoded_bytes: u64,
    pub(crate) instruction_count: u64,
    instructions: Vec<architecture::DecodedInstruction>,
}

#[derive(Debug, Error)]
pub(crate) enum ExecutableError {
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
    #[error("firmware ELF {path} is not a 32-bit ELF")]
    UnsupportedElfClass { path: PathBuf },
    #[error("firmware ELF {path} uses unsupported byte order {byte_order:?}")]
    UnsupportedByteOrder {
        path: PathBuf,
        byte_order: Endianness,
    },
    #[error("firmware ELF {path} has architecture {actual:?}, expected {expected}")]
    ArchitectureMismatch {
        path: PathBuf,
        actual: object::Architecture,
        expected: &'static str,
    },
    #[error("could not read section name at index {index} in {path}: {source}")]
    SectionName {
        path: PathBuf,
        index: usize,
        #[source]
        source: object::Error,
    },
    #[error("could not read section {section:?} from {path}: {source}")]
    SectionData {
        path: PathBuf,
        section: String,
        #[source]
        source: object::Error,
    },
    #[error("firmware ELF {path} has no executable sections")]
    MissingExecutableSections { path: PathBuf },
    #[error("{kind} range {start:#x} + {bytes} overflows the address space")]
    InvalidRange {
        kind: &'static str,
        start: u64,
        bytes: u64,
    },
    #[error("load segment {index} in {path} has file size {file_bytes} larger than memory size {memory_bytes}")]
    InvalidLoadSegment {
        path: PathBuf,
        index: usize,
        file_bytes: u64,
        memory_bytes: u64,
    },
    #[error("firmware image range {start:#x} + {bytes} is outside owned region {region:?}")]
    FirmwareOwnership {
        start: u64,
        bytes: u64,
        region: String,
    },
    #[error(
        "loaded section {section:?} at {start:#x}..{end:#x} enters protected region {region:?}"
    )]
    ProtectedRegion {
        section: String,
        start: u64,
        end: u64,
        region: String,
    },
    #[error(
        "allocated section {section:?} at {start:#x}..{end:#x} has no valid architecture placement"
    )]
    UnmappedSection {
        section: String,
        start: u64,
        end: u64,
    },
    #[error("entry point {entry:#x} is not inside executable code")]
    EntryOutsideExecutableCode { entry: u64 },
    #[error("required startup section {section:?} is missing or not executable")]
    MissingStartupSection { section: &'static str },
    #[error("required startup symbol {symbol:?} does not resolve to {expected:#x}")]
    InvalidStartupSymbol { symbol: &'static str, expected: u64 },
    #[error("Arm vector table {section:?} is malformed")]
    InvalidVectorTable { section: &'static str },
    #[error("initial stack pointer {address:#x} is outside declared RAM")]
    InvalidInitialStackPointer { address: u64 },
    #[error("reset vector {actual:#x} does not match ELF entry point {expected:#x}")]
    InvalidResetVector { expected: u64, actual: u64 },
    #[error("could not resolve disassembler for {architecture}: {source}")]
    ResolveDisassembler {
        architecture: &'static str,
        #[source]
        source: personal_hopspot_builder::BuildError,
    },
    #[error("failed to run {program} for {path}: {source}")]
    RunDisassembler {
        program: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{program} exited with {status} for {path}: {stderr}")]
    DisassemblerFailed {
        program: &'static str,
        path: PathBuf,
        status: std::process::ExitStatus,
        stderr: String,
    },
    #[error("{program} produced invalid UTF-8 for {path}: {source}")]
    InvalidDisassembly {
        program: &'static str,
        path: PathBuf,
        #[source]
        source: std::string::FromUtf8Error,
    },
    #[error("{program} decoded no instructions from {path}")]
    EmptyDisassembly {
        program: &'static str,
        path: PathBuf,
    },
    #[error("{program} did not decode entry point {entry:#x} in {path}")]
    UndecodedEntry {
        program: &'static str,
        path: PathBuf,
        entry: u64,
    },
    #[error("{evidence} cannot be represented in the report schema")]
    CountOverflow { evidence: &'static str },
    #[error(transparent)]
    Stack(#[from] stack::StackMetadataError),
    #[error("classified function bytes {classified} exceed executable bytes {executable}")]
    InvalidFunctionCoverage { classified: u64, executable: u64 },
}

pub(crate) fn analyze(
    path: &Path,
    profile: &MemoryProfile,
    build_adapter: &Adapter,
    firmware_image_bytes: u64,
) -> Result<ExecutableAnalysis, ExecutableError> {
    let bytes = fs::read(path).map_err(|source| ExecutableError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let object =
        object::File::parse(bytes.as_slice()).map_err(|source| ExecutableError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
    if object.format() != BinaryFormat::Elf {
        return Err(ExecutableError::NonElf {
            path: path.to_path_buf(),
            format: object.format(),
        });
    }
    if object.is_64() {
        return Err(ExecutableError::UnsupportedElfClass {
            path: path.to_path_buf(),
        });
    }
    if object.endianness() != Endianness::Little {
        return Err(ExecutableError::UnsupportedByteOrder {
            path: path.to_path_buf(),
            byte_order: object.endianness(),
        });
    }

    let adapter = architecture::adapter_for(profile.architecture);
    if object.architecture() != adapter.object_architecture() {
        return Err(ExecutableError::ArchitectureMismatch {
            path: path.to_path_buf(),
            actual: object.architecture(),
            expected: adapter.id(),
        });
    }
    validate_firmware_ownership(profile, firmware_image_bytes)?;
    adapter.validate_allocated_sections(path, profile, &object, &bytes)?;

    let executable_sections = executable_sections(path, &object)?;
    let entry_point = object.entry();
    let startup = adapter.startup(path, profile, &object, &executable_sections, entry_point)?;
    let load_segments = load_segments(path, &bytes)?;
    let functions = functions::analyze(
        &object,
        &executable_sections,
        adapter.code_address_normalizer(),
    )?;
    let disassembly = disassembly::analyze(
        path,
        build_adapter,
        adapter,
        &executable_sections,
        entry_point,
    )?;
    let stack = stack::analyze(stack::StackAnalysisInput {
        path,
        profile,
        adapter,
        object: &object,
        startup: &startup,
        functions: &functions,
        instructions: &disassembly.instructions,
        load_segments: &load_segments,
        frame_evidence: build_adapter.stack_frame_evidence(),
    })?;

    Ok(ExecutableAnalysis {
        architecture: profile.architecture,
        byte_order: ByteOrder::Little,
        entry_point,
        load_segments,
        executable_sections,
        startup,
        functions,
        disassembly,
        stack,
    })
}

fn validate_firmware_ownership(
    profile: &MemoryProfile,
    firmware_image_bytes: u64,
) -> Result<(), ExecutableError> {
    let owned = profile
        .region(profile.firmware.firmware_owned_region)
        .ok_or_else(|| ExecutableError::FirmwareOwnership {
            start: 0,
            bytes: firmware_image_bytes,
            region: profile.firmware.firmware_owned_region.0.to_string(),
        })?;
    let image = AddressRange::from_start_and_size(owned.range.start(), firmware_image_bytes)
        .map_err(|_| ExecutableError::FirmwareOwnership {
            start: owned.range.start(),
            bytes: firmware_image_bytes,
            region: owned.id.0.to_string(),
        })?;
    if !owned.range.contains(image) {
        return Err(ExecutableError::FirmwareOwnership {
            start: image.start(),
            bytes: image.byte_len(),
            region: owned.id.0.to_string(),
        });
    }
    if let Some(region) = profile.regions.iter().find(|region| {
        region.id != owned.id
            && region.address_space == owned.address_space
            && region.range.overlaps(image)
    }) {
        return Err(ExecutableError::ProtectedRegion {
            section: "packaged-firmware".to_string(),
            start: image.start(),
            end: image.end(),
            region: region.id.0.to_string(),
        });
    }
    Ok(())
}

fn load_segments(path: &Path, bytes: &[u8]) -> Result<Vec<LoadSegment>, ExecutableError> {
    let object =
        ElfFile32::<Endianness>::parse(bytes).map_err(|source| ExecutableError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
    let endian = object.endian();
    object
        .elf_program_headers()
        .iter()
        .enumerate()
        .filter(|(_, segment)| {
            segment.p_type(endian) == object::elf::PT_LOAD
                && u64::from(segment.p_memsz(endian)) != 0
        })
        .map(|(index, segment)| {
            let file_bytes = u64::from(segment.p_filesz(endian));
            let memory_bytes = u64::from(segment.p_memsz(endian));
            if file_bytes > memory_bytes {
                return Err(ExecutableError::InvalidLoadSegment {
                    path: path.to_path_buf(),
                    index,
                    file_bytes,
                    memory_bytes,
                });
            }
            let run_range = range(
                "load-segment run",
                u64::from(segment.p_vaddr(endian)),
                memory_bytes,
            )?;
            let load_range = range(
                "load-segment load",
                u64::from(segment.p_paddr(endian)),
                memory_bytes,
            )?;
            let flags = segment.p_flags(endian);
            let mut permissions = Vec::new();
            if flags & object::elf::PF_R != 0 {
                permissions.push(LoadPermission::Read);
            }
            if flags & object::elf::PF_W != 0 {
                permissions.push(LoadPermission::Write);
            }
            if flags & object::elf::PF_X != 0 {
                permissions.push(LoadPermission::Execute);
            }
            Ok(LoadSegment {
                file_offset: u64::from(segment.p_offset(endian)),
                run_range,
                load_range,
                file_bytes,
                memory_bytes,
                alignment: u64::from(segment.p_align(endian)),
                permissions,
            })
        })
        .collect()
}

fn executable_sections(
    path: &Path,
    object: &object::File<'_>,
) -> Result<Vec<ExecutableSection>, ExecutableError> {
    let sections = object
        .sections()
        .filter(|section| section.size() != 0 && is_executable(section.flags()))
        .map(|section| {
            let name = section
                .name()
                .map_err(|source| ExecutableError::SectionName {
                    path: path.to_path_buf(),
                    index: section.index().0,
                    source,
                })?
                .to_string();
            let data = section
                .uncompressed_data()
                .map_err(|source| ExecutableError::SectionData {
                    path: path.to_path_buf(),
                    section: name.clone(),
                    source,
                })?
                .into_owned();
            let range = range("executable section", section.address(), section.size())?;
            Ok(ExecutableSection {
                name,
                range,
                alignment: section.align(),
                fingerprint: prns_flash_manifest::sha256_hex(&data),
                data,
            })
        })
        .collect::<Result<Vec<_>, ExecutableError>>()?;
    if sections.is_empty() {
        Err(ExecutableError::MissingExecutableSections {
            path: path.to_path_buf(),
        })
    } else {
        Ok(sections)
    }
}

fn range(kind: &'static str, start: u64, bytes: u64) -> Result<AddressRange, ExecutableError> {
    AddressRange::from_start_and_size(start, bytes).map_err(|_| ExecutableError::InvalidRange {
        kind,
        start,
        bytes,
    })
}

fn is_executable(flags: SectionFlags) -> bool {
    matches!(
        flags,
        SectionFlags::Elf { sh_flags } if sh_flags & u64::from(object::elf::SHF_EXECINSTR) != 0
    )
}
