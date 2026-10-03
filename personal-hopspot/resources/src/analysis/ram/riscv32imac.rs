use personal_hopspot_memory::{AddressRange, AddressSpaceKind, MemoryProfile};

use super::{
    arena_allocation, required_section_kind, required_space, BackingAllocation, RamAnalysisError,
};
use crate::analysis::{AllocatedSection, SectionKind};

pub(super) fn allocations(
    profile: &MemoryProfile,
    sections: &[AllocatedSection],
) -> Result<Vec<BackingAllocation>, RamAnalysisError> {
    let instruction = required_space(profile, AddressSpaceKind::InstructionRam)?;
    let data = required_space(profile, AddressSpaceKind::DataRam)?;
    if instruction.backing_store != data.backing_store {
        return Err(RamAnalysisError::MismatchedBackingStores {
            profile: profile.id.as_str(),
            first: instruction.id,
            second: data.id,
        });
    }
    let trap = required_section_kind(profile.architecture, sections, ".trap", SectionKind::Code)?;
    let stack = required_section_kind(
        profile.architecture,
        sections,
        ".stack",
        SectionKind::ZeroFill,
    )?;
    let capacity_range = AddressRange::from_start_and_size(
        trap.run_range().start(),
        stack
            .run_range()
            .end()
            .checked_sub(trap.run_range().start())
            .ok_or(RamAnalysisError::InvalidArena {
                architecture: profile.architecture,
            })?,
    )
    .map_err(|_| RamAnalysisError::InvalidArena {
        architecture: profile.architecture,
    })?;
    let static_range = AddressRange::from_start_and_size(
        capacity_range.start(),
        stack
            .run_range()
            .start()
            .checked_sub(capacity_range.start())
            .ok_or(RamAnalysisError::InvalidArena {
                architecture: profile.architecture,
            })?,
    )
    .map_err(|_| RamAnalysisError::InvalidArena {
        architecture: profile.architecture,
    })?;
    Ok(vec![arena_allocation(
        profile.architecture,
        instruction.backing_store,
        static_range,
        capacity_range.byte_len(),
        sections,
    )?])
}
