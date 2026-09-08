use std::path::Path;

use object::{Object, ObjectSection};
use personal_hopspot_memory::{AddressSpaceGeometry, AddressSpaceKind, MemoryProfile};

use super::{
    entry_section, parse_instruction, require_symbol, validate_fixed_placement,
    validate_flash_loads, AssuranceAdapter, DecodedInstruction,
};
use crate::analysis::executable::{
    ExecutableError, ExecutableSection, StartupAnchor, StartupAnchorRole, StartupStructure,
};

const VECTOR_SECTION: &str = ".vector_table";
const ENTRY_SYMBOL: &str = "__stext";

pub(super) static ADAPTER: AssuranceAdapter = AssuranceAdapter::new(
    "thumbv7em",
    object::Architecture::Arm,
    normalize,
    validate,
    startup,
    decoded_instruction,
);

fn normalize(address: u64) -> u64 {
    address & !1
}

fn validate(
    path: &Path,
    profile: &MemoryProfile,
    object: &object::File<'_>,
    bytes: &[u8],
) -> Result<(), ExecutableError> {
    validate_fixed_placement(path, profile, object)?;
    validate_flash_loads(path, profile, object, bytes)
}

fn startup(
    _path: &Path,
    profile: &MemoryProfile,
    object: &object::File<'_>,
    sections: &[ExecutableSection],
    entry: u64,
) -> Result<StartupStructure, ExecutableError> {
    let entry_section = entry_section(sections, entry, normalize)?;
    require_symbol(object, ENTRY_SYMBOL, entry, normalize)?;
    let vector =
        object
            .section_by_name(VECTOR_SECTION)
            .ok_or(ExecutableError::InvalidVectorTable {
                section: VECTOR_SECTION,
            })?;
    let data = vector
        .data()
        .map_err(|_| ExecutableError::InvalidVectorTable {
            section: VECTOR_SECTION,
        })?;
    let words = data.get(..8).ok_or(ExecutableError::InvalidVectorTable {
        section: VECTOR_SECTION,
    })?;
    let initial_stack = u64::from(u32::from_le_bytes(words[..4].try_into().map_err(|_| {
        ExecutableError::InvalidVectorTable {
            section: VECTOR_SECTION,
        }
    })?));
    let reset = u64::from(u32::from_le_bytes(words[4..].try_into().map_err(|_| {
        ExecutableError::InvalidVectorTable {
            section: VECTOR_SECTION,
        }
    })?));
    let stack_valid = profile.address_spaces.iter().any(|space| {
        space.kind == AddressSpaceKind::InternalRam
            && matches!(space.geometry, AddressSpaceGeometry::Fixed(range) if range.start() <= initial_stack && initial_stack <= range.end())
    });
    if !stack_valid {
        return Err(ExecutableError::InvalidInitialStackPointer {
            address: initial_stack,
        });
    }
    if reset != entry {
        return Err(ExecutableError::InvalidResetVector {
            expected: entry,
            actual: reset,
        });
    }

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
                role: StartupAnchorRole::InitialStackPointer,
                address: initial_stack,
                section: VECTOR_SECTION.to_string(),
            },
            StartupAnchor {
                role: StartupAnchorRole::ResetVector,
                address: reset,
                section: VECTOR_SECTION.to_string(),
            },
            StartupAnchor {
                role: StartupAnchorRole::ExceptionVectors,
                address: vector.address(),
                section: VECTOR_SECTION.to_string(),
            },
        ],
    })
}

fn decoded_instruction(line: &str) -> Option<DecodedInstruction> {
    parse_instruction(line, &[2, 4])
}
