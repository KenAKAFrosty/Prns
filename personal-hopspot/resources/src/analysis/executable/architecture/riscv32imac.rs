use std::path::Path;

use personal_hopspot_memory::{AddressRange, MemoryProfile};

use super::{
    entry_section, executable_section, parse_instruction, require_symbol,
    validate_windowed_placement, AssuranceAdapter, DecodedInstruction,
};
use crate::analysis::executable::{
    ExecutableError, ExecutableSection, StartupAnchor, StartupAnchorRole, StartupStructure,
};

const ENTRY_SYMBOL: &str = "_start";
const TRAP_SECTION: &str = ".trap";
const WINDOWS: [AddressRange; 2] = [
    AddressRange::new(0x4080_0000, 0x4090_0000),
    AddressRange::new(0x4200_0000, 0x4240_0000),
];

pub(super) static ADAPTER: AssuranceAdapter = AssuranceAdapter::new(
    "riscv32imac",
    object::Architecture::Riscv32,
    normalize,
    validate,
    startup,
    decoded_instruction,
);

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
    let trap = executable_section(sections, TRAP_SECTION)?;
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
                role: StartupAnchorRole::TrapVector,
                address: trap.range.start(),
                section: trap.name.clone(),
            },
        ],
    })
}

fn decoded_instruction(line: &str) -> Option<DecodedInstruction> {
    parse_instruction(line, &[2, 4])
}
