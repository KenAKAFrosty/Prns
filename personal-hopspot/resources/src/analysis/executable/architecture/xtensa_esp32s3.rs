use std::path::Path;

use personal_hopspot_memory::MemoryProfile;

use super::{
    direct_operand, entry_section, executable_section, parse_instruction, require_symbol,
    AssuranceAdapter, CallTarget, DecodedInstruction, StackReservation,
};
use crate::analysis::executable::{
    ExecutableError, ExecutableSection, StartupAnchor, StartupAnchorRole, StartupStructure,
};
use personal_hopspot_memory::ProcessorArchitecture;

const ENTRY_SYMBOL: &str = "Reset";
const VECTOR_SECTION: &str = ".vectors";
pub(super) static ADAPTER: AssuranceAdapter = AssuranceAdapter {
    id: ProcessorArchitecture::XtensaEsp32S3.id(),
    object_architecture: object::Architecture::Xtensa,
    normalize_code_address: normalize,
    startup,
    decoded_instruction,
    call_target,
    stack_reservation: StackReservation::Undeclared,
    dwarf_cfa_registers: &[1, 7],
};

fn normalize(address: u64) -> u64 {
    address
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
