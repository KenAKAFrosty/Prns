mod riscv32imac;
mod thumbv7em;
mod xtensa_esp32s3;

use std::path::Path;

use object::read::elf::ProgramHeader;
use object::{Object, ObjectSection, ObjectSymbol, SectionFlags};
use personal_hopspot_memory::{
    linker_address_profile, AddressRange, AddressSpaceKind, LinkerAddressProfile, MemoryProfile,
    ProcessorArchitecture,
};

use super::{ExecutableError, ExecutableSection, StartupStructure};

type AddressNormalizer = fn(u64) -> u64;
type StartupAnalyzer = for<'data> fn(
    &Path,
    &MemoryProfile,
    &object::File<'data>,
    &[ExecutableSection],
    u64,
) -> Result<StartupStructure, ExecutableError>;
type InstructionDecoder = fn(&str) -> Option<DecodedInstruction>;
type CallDecoder = fn(&[DecodedInstruction], usize) -> CallTarget;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StackReservation {
    RuntimeReservation(&'static str),
    Undeclared,
}

pub(super) struct AssuranceAdapter {
    pub(super) id: &'static str,
    pub(super) object_architecture: object::Architecture,
    pub(super) normalize_code_address: AddressNormalizer,
    pub(super) startup: StartupAnalyzer,
    pub(super) decoded_instruction: InstructionDecoder,
    pub(super) call_target: CallDecoder,
    pub(super) stack_reservation: StackReservation,
    pub(super) dwarf_cfa_registers: &'static [u16],
}

impl AssuranceAdapter {
    pub(super) const fn id(&self) -> &'static str {
        self.id
    }

    pub(super) const fn object_architecture(&self) -> object::Architecture {
        self.object_architecture
    }

    pub(super) fn normalize_code_address(&self, address: u64) -> u64 {
        (self.normalize_code_address)(address)
    }

    pub(super) const fn code_address_normalizer(&self) -> AddressNormalizer {
        self.normalize_code_address
    }

    pub(super) fn startup(
        &self,
        path: &Path,
        profile: &MemoryProfile,
        object: &object::File<'_>,
        sections: &[ExecutableSection],
        entry: u64,
    ) -> Result<StartupStructure, ExecutableError> {
        (self.startup)(path, profile, object, sections, entry)
    }

    pub(super) fn decoded_instruction(&self, line: &str) -> Option<DecodedInstruction> {
        (self.decoded_instruction)(line)
    }

    pub(super) fn call_target(
        &self,
        instructions: &[DecodedInstruction],
        index: usize,
    ) -> CallTarget {
        (self.call_target)(instructions, index)
    }

    pub(super) const fn stack_reservation(&self) -> StackReservation {
        self.stack_reservation
    }

    pub(super) const fn dwarf_cfa_registers(&self) -> &'static [u16] {
        self.dwarf_cfa_registers
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DecodedInstruction {
    pub(super) address: u64,
    pub(super) bytes: u64,
    pub(super) mnemonic: String,
    pub(super) operands: String,
}

impl DecodedInstruction {
    pub(super) fn end(&self) -> u64 {
        self.address + self.bytes
    }

    pub(super) fn range(&self) -> AddressRange {
        AddressRange::new(self.address, self.end())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CallTarget {
    NotCall,
    Direct(u64),
    UnresolvedDirect,
    Indirect,
}

pub(super) const fn adapter_for(architecture: ProcessorArchitecture) -> &'static AssuranceAdapter {
    match architecture {
        ProcessorArchitecture::ThumbV7em => &thumbv7em::ADAPTER,
        ProcessorArchitecture::RiscV32Imac => &riscv32imac::ADAPTER,
        ProcessorArchitecture::XtensaEsp32S3 => &xtensa_esp32s3::ADAPTER,
    }
}

pub(super) fn validate_profile_placement(
    path: &Path,
    profile: &MemoryProfile,
    object: &object::File<'_>,
    bytes: &[u8],
) -> Result<(), ExecutableError> {
    let linker_profile =
        linker_address_profile(profile.id).ok_or(ExecutableError::MissingLinkerAddressProfile {
            profile: profile.id.0,
        })?;
    linker_profile.validate(profile).map_err(|reason| {
        ExecutableError::InvalidLinkerAddressProfile {
            profile: profile.id.0,
            reason,
        }
    })?;
    for section in object
        .sections()
        .filter(|section| section.size() != 0 && is_allocated(section.flags()))
    {
        let name = section_name(path, &section)?;
        let range = section_range(&name, section.address(), section.size())?;
        let mut mapped = false;
        for space in linker_profile.address_spaces {
            let ranges = space.ranges(profile).map_err(|reason| {
                ExecutableError::InvalidLinkerAddressProfile {
                    profile: profile.id.0,
                    reason,
                }
            })?;
            if ranges.into_iter().any(|bounds| bounds.contains(range)) {
                mapped = true;
                break;
            }
        }
        if !mapped {
            return Err(ExecutableError::UnmappedSection {
                section: name,
                start: range.start(),
                end: range.end(),
            });
        }
    }
    let elf =
        object::read::elf::ElfFile32::<object::Endianness>::parse(bytes).map_err(|source| {
            ExecutableError::Parse {
                path: path.to_path_buf(),
                source,
            }
        })?;
    let endian = elf.endian();
    let firmware = profile
        .region(profile.firmware.firmware_owned_region)
        .ok_or_else(|| ExecutableError::FirmwareOwnership {
            start: 0,
            bytes: 0,
            region: profile.firmware.firmware_owned_region.0.to_string(),
        })?;
    for section in object
        .sections()
        .filter(|section| section.size() != 0 && is_allocated(section.flags()))
    {
        let Some((section_offset, section_bytes)) = section.file_range() else {
            continue;
        };
        if section_bytes == 0 {
            continue;
        }
        let name = section_name(path, &section)?;
        let Some(segment) = elf.elf_program_headers().iter().find(|segment| {
            if segment.p_type(endian) != object::elf::PT_LOAD {
                return false;
            }
            let start = u64::from(segment.p_offset(endian));
            let end = start.saturating_add(u64::from(segment.p_filesz(endian)));
            start <= section_offset && section_offset.saturating_add(section_bytes) <= end
        }) else {
            return Err(ExecutableError::UnmappedSection {
                section: name,
                start: section.address(),
                end: section.address().saturating_add(section.size()),
            });
        };
        let load_start = u64::from(segment.p_paddr(endian))
            .saturating_add(section_offset - u64::from(segment.p_offset(endian)));
        let load_range = section_range(&name, load_start, section_bytes)?;
        let run_range = section_range(&name, section.address(), section.size())?;
        if load_range == run_range {
            continue;
        }
        let mapped_flash = mapped_to_kind(
            profile,
            linker_profile,
            AddressSpaceKind::InternalFlash,
            load_range,
        )?;
        if !mapped_flash && !firmware.range.contains(load_range) {
            let region = profile
                .regions
                .iter()
                .find(|region| {
                    region.address_space == firmware.address_space
                        && region.range.overlaps(load_range)
                })
                .map_or("outside-declared-flash", |region| region.id.0);
            return Err(ExecutableError::ProtectedRegion {
                section: name,
                start: load_range.start(),
                end: load_range.end(),
                region: region.to_string(),
            });
        }
    }
    Ok(())
}

fn mapped_to_kind(
    profile: &MemoryProfile,
    linker_profile: &LinkerAddressProfile,
    kind: AddressSpaceKind,
    range: AddressRange,
) -> Result<bool, ExecutableError> {
    for space in profile
        .address_spaces
        .iter()
        .filter(|space| space.kind == kind)
    {
        let Some(linker_space) = linker_profile.address_space(space.id) else {
            continue;
        };
        let ranges = linker_space.ranges(profile).map_err(|reason| {
            ExecutableError::InvalidLinkerAddressProfile {
                profile: profile.id.0,
                reason,
            }
        })?;
        if ranges.into_iter().any(|bounds| bounds.contains(range)) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn entry_section(
    sections: &[ExecutableSection],
    entry: u64,
    normalize: fn(u64) -> u64,
) -> Result<String, ExecutableError> {
    let normalized = normalize(entry);
    sections
        .iter()
        .find(|section| section.range.start() <= normalized && normalized < section.range.end())
        .map(|section| section.name.clone())
        .ok_or(ExecutableError::EntryOutsideExecutableCode { entry })
}

pub(super) fn require_symbol(
    object: &object::File<'_>,
    symbol_name: &'static str,
    expected: u64,
    normalize: fn(u64) -> u64,
) -> Result<(), ExecutableError> {
    let found = object.symbols().any(|symbol| {
        symbol.is_definition()
            && symbol.name().is_ok_and(|name| name == symbol_name)
            && normalize(symbol.address()) == normalize(expected)
    });
    if found {
        Ok(())
    } else {
        Err(ExecutableError::InvalidStartupSymbol {
            symbol: symbol_name,
            expected,
        })
    }
}

pub(super) fn executable_section<'a>(
    sections: &'a [ExecutableSection],
    name: &'static str,
) -> Result<&'a ExecutableSection, ExecutableError> {
    sections
        .iter()
        .find(|section| section.name == name)
        .ok_or(ExecutableError::MissingStartupSection { section: name })
}

pub(super) fn parse_instruction(line: &str, widths: &[u64]) -> Option<DecodedInstruction> {
    let (address, body) = line.trim().split_once(':')?;
    let address = u64::from_str_radix(address.trim(), 16).ok()?;
    let mut tokens = body.split_whitespace();
    let mut bytes = 0_u64;
    let mut mnemonic = None;
    for token in tokens.by_ref() {
        if token.len() % 2 == 0
            && !token.is_empty()
            && token.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            bytes = bytes.checked_add(u64::try_from(token.len() / 2).ok()?)?;
        } else {
            mnemonic = Some(token);
            break;
        }
    }
    let mnemonic = mnemonic?;
    if !widths.contains(&bytes) || mnemonic.starts_with('.') || mnemonic.starts_with('<') {
        return None;
    }
    address.checked_add(bytes)?;
    Some(DecodedInstruction {
        address,
        bytes,
        mnemonic: mnemonic.to_ascii_lowercase(),
        operands: tokens.collect::<Vec<_>>().join(" "),
    })
}

pub(super) fn direct_operand(operands: &str) -> Option<u64> {
    operands
        .split(['<', '@'])
        .next()?
        .split([',', ' ', '\t'])
        .filter(|token| !token.is_empty())
        .rev()
        .find_map(|token| {
            let token = token.trim_matches(['#', '<', '>', '[', ']']);
            let token = token.strip_prefix("0x").unwrap_or(token);
            u64::from_str_radix(token, 16).ok()
        })
}

fn section_name<'data>(
    path: &Path,
    section: &impl ObjectSection<'data>,
) -> Result<String, ExecutableError> {
    section
        .name()
        .map(str::to_string)
        .map_err(|source| ExecutableError::SectionName {
            path: path.to_path_buf(),
            index: section.index().0,
            source,
        })
}

fn section_range(section: &str, start: u64, bytes: u64) -> Result<AddressRange, ExecutableError> {
    AddressRange::from_start_and_size(start, bytes).map_err(|_| ExecutableError::UnmappedSection {
        section: section.to_string(),
        start,
        end: start.saturating_add(bytes),
    })
}

fn is_allocated(flags: SectionFlags) -> bool {
    matches!(
        flags,
        SectionFlags::Elf { sh_flags } if sh_flags & u64::from(object::elf::SHF_ALLOC) != 0
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_resolves_every_memory_architecture() {
        let expected = [
            (ProcessorArchitecture::ThumbV7em, "thumbv7em"),
            (ProcessorArchitecture::RiscV32Imac, "riscv32imac"),
            (ProcessorArchitecture::XtensaEsp32S3, "xtensa-esp32s3"),
        ];
        for (architecture, id) in expected {
            assert_eq!(adapter_for(architecture).id(), id);
        }
    }

    #[test]
    fn architecture_disassembly_formats_have_exact_widths() {
        assert_eq!(
            thumbv7em::ADAPTER.decoded_instruction("26100: f007 fb04  bl 0x2d70c"),
            Some(DecodedInstruction {
                address: 0x26100,
                bytes: 4,
                mnemonic: "bl".to_string(),
                operands: "0x2d70c".to_string(),
            })
        );
        assert_eq!(
            riscv32imac::ADAPTER.decoded_instruction("42040036: c114  sw a3, 0x0(a0)"),
            Some(DecodedInstruction {
                address: 0x42040036,
                bytes: 2,
                mnemonic: "sw".to_string(),
                operands: "a3, 0x0(a0)".to_string(),
            })
        );
        assert_eq!(
            riscv32imac::ADAPTER.decoded_instruction("42040020: 0dfc0517  auipc a0, 0xdfc0"),
            Some(DecodedInstruction {
                address: 0x42040020,
                bytes: 4,
                mnemonic: "auipc".to_string(),
                operands: "a0, 0xdfc0".to_string(),
            })
        );
        assert_eq!(
            xtensa_esp32s3::ADAPTER.decoded_instruction("40378878: 002136  entry a1, 16"),
            Some(DecodedInstruction {
                address: 0x40378878,
                bytes: 3,
                mnemonic: "entry".to_string(),
                operands: "a1, 16".to_string(),
            })
        );
        assert_eq!(
            xtensa_esp32s3::ADAPTER.decoded_instruction("403788ae: 81  .byte 0x81"),
            None
        );
    }

    #[test]
    fn instruction_parser_rejects_malformed_evidence() {
        assert_eq!(parse_instruction("not disassembly", &[2, 4]), None);
        assert_eq!(parse_instruction("1000: xyz  add", &[2, 4]), None);
        assert_eq!(parse_instruction("1000: 001122  add", &[2, 4]), None);
        assert_eq!(parse_instruction("1000: 0011  .word 0", &[2, 4]), None);
    }

    #[test]
    fn architecture_call_decoders_distinguish_direct_and_indirect_calls() {
        let arm = [thumbv7em::ADAPTER
            .decoded_instruction("26100: f007 fb04  bl 0x2d70c <callee> @ imm = #0x7610")
            .expect("Arm instruction")];
        assert_eq!(
            thumbv7em::ADAPTER.call_target(&arm, 0),
            CallTarget::Direct(0x2d70c)
        );

        let riscv = [
            riscv32imac::ADAPTER
                .decoded_instruction("42040020: 0dfc0517  auipc a0, 0xdfc0")
                .expect("RISC-V upper address"),
            riscv32imac::ADAPTER
                .decoded_instruction("42040024: fe0500e7  jalr -32(a0) <callee>")
                .expect("RISC-V indirect jump"),
        ];
        assert_eq!(
            riscv32imac::ADAPTER.call_target(&riscv, 1),
            CallTarget::Direct(0x50000000)
        );

        let xtensa = [xtensa_esp32s3::ADAPTER
            .decoded_instruction("40378878: 0021c5  call8 0x40379094")
            .expect("Xtensa instruction")];
        assert_eq!(
            xtensa_esp32s3::ADAPTER.call_target(&xtensa, 0),
            CallTarget::Direct(0x40379094)
        );
    }
}
