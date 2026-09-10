use std::path::Path;

use personal_hopspot_memory::{AddressRange, MemoryProfile};

use super::{
    direct_operand, entry_section, executable_section, parse_instruction, require_symbol,
    validate_windowed_placement, AssuranceAdapter, CallTarget, DecodedInstruction, StackLimit,
};
use crate::analysis::executable::{
    ExecutableError, ExecutableSection, StartupAnchor, StartupAnchorRole, StartupStructure,
};
use personal_hopspot_memory::ProcessorArchitecture;

const ENTRY_SYMBOL: &str = "Reset";
const VECTOR_SECTION: &str = ".vectors";
const WINDOWS: [AddressRange; 6] = [
    AddressRange::new(0x3C00_0000, 0x3E00_0000),
    AddressRange::new(0x3FC0_0000, 0x4000_0000),
    AddressRange::new(0x4037_0000, 0x4040_0000),
    AddressRange::new(0x4200_0000, 0x4400_0000),
    AddressRange::new(0x5000_0000, 0x5000_4000),
    AddressRange::new(0x600F_E000, 0x6010_0000),
];

pub(super) static ADAPTER: AssuranceAdapter = AssuranceAdapter {
    id: ProcessorArchitecture::XtensaEsp32S3.id(),
    object_architecture: object::Architecture::Xtensa,
    normalize_code_address: normalize,
    validate_allocated_sections: validate,
    startup,
    decoded_instruction,
    call_target,
    stack_limit: StackLimit::Undeclared,
    dwarf_cfa_registers: &[1, 7],
};

fn normalize(address: u64) -> u64 {
    address
}

fn validate(
    path: &Path,
    _profile: &MemoryProfile,
    object: &object::File<'_>,
    _bytes: &[u8],
) -> Result<(), ExecutableError> {
    validate_windowed_placement(path, object, &WINDOWS)
}

fn startup(
    _path: &Path,
    _profile: &MemoryProfile,
    object: &object::File<'_>,
    sections: &[ExecutableSection],
    entry: u64,
) -> Result<StartupStructure, ExecutableError> {
    let entry_section = entry_section(sections, entry, normalize)?;
    require_symbol(object, ENTRY_SYMBOL, entry, normalize)?;
    let vectors = executable_section(sections, VECTOR_SECTION)?;
    Ok(StartupStructure {
        entry_section: entry_section.clone(),
        entry_symbol: ENTRY_SYMBOL.to_string(),
        anchors: vec![
            StartupAnchor {
                role: StartupAnchorRole::EntryPoint,
                address: entry,
                section: entry_section,
            },
            StartupAnchor {
                role: StartupAnchorRole::ExceptionVectors,
                address: vectors.range.start(),
                section: vectors.name.clone(),
            },
        ],
    })
}

fn decoded_instruction(line: &str) -> Option<DecodedInstruction> {
    parse_instruction(line, &[2, 3])
}

fn call_target(instructions: &[DecodedInstruction], index: usize) -> CallTarget {
    let instruction = &instructions[index];
    match instruction.mnemonic.as_str() {
        "call0" | "call4" | "call8" | "call12" => direct_operand(&instruction.operands)
            .map(CallTarget::Direct)
            .unwrap_or(CallTarget::UnresolvedDirect),
        "callx0" | "callx4" | "callx8" | "callx12" => CallTarget::Indirect,
        _ => CallTarget::NotCall,
    }
}
