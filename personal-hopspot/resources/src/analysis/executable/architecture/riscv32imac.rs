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

const ENTRY_SYMBOL: &str = "_start";
const TRAP_SECTION: &str = ".trap";
pub(super) static ADAPTER: AssuranceAdapter = AssuranceAdapter {
    id: ProcessorArchitecture::RiscV32Imac.id(),
    object_architecture: object::Architecture::Riscv32,
    normalize_code_address: normalize,
    startup,
    decoded_instruction,
    call_target,
    stack_reservation: StackReservation::Undeclared,
    dwarf_cfa_registers: &[2],
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

fn call_target(instructions: &[DecodedInstruction], index: usize) -> CallTarget {
    let instruction = &instructions[index];
    match instruction.mnemonic.as_str() {
        "jal" | "c.jal" | "call" => direct_operand(&instruction.operands)
            .map(CallTarget::Direct)
            .unwrap_or(CallTarget::UnresolvedDirect),
        "jalr" | "c.jalr" => resolve_jalr(instructions, index)
            .map(CallTarget::Direct)
            .unwrap_or(CallTarget::Indirect),
        _ => CallTarget::NotCall,
    }
}

fn resolve_jalr(instructions: &[DecodedInstruction], index: usize) -> Option<u64> {
    let instruction = &instructions[index];
    let previous = index
        .checked_sub(1)
        .and_then(|index| instructions.get(index))?;
    if previous.mnemonic != "auipc" {
        return None;
    }
    let (base_register, upper) = previous.operands.split_once(',')?;
    let (offset, call_register) = jalr_operands(&instruction.operands)?;
    if base_register.trim() != call_register {
        return None;
    }
    let upper = parse_signed_immediate(upper.trim())?;
    let base = i128::from(previous.address) + (upper << 12);
    u64::try_from(base + i128::from(offset)).ok()
}

fn jalr_operands(operands: &str) -> Option<(i64, &str)> {
    let operands = operands.split(['<', '@']).next().unwrap_or(operands).trim();
    let address = operands.rsplit(',').next()?.trim();
    let (offset, register) = address.split_once('(')?;
    let register = register.strip_suffix(')')?.trim();
    Some((
        i64::try_from(parse_signed_immediate(offset.trim())?).ok()?,
        register,
    ))
}

fn parse_signed_immediate(value: &str) -> Option<i128> {
    if let Some(value) = value.strip_prefix("-0x") {
        return i128::from_str_radix(value, 16).ok().map(|value| -value);
    }
    if let Some(value) = value.strip_prefix("0x") {
        return i128::from_str_radix(value, 16).ok();
    }
    value.parse().ok()
}
