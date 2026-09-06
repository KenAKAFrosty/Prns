use personal_hopspot_memory::{
    AddressRange, AddressSpaceGeometry, AddressSpaceKind, MemoryProfile,
};

use super::{
    arena_allocation, fixed_capacity, fixed_space_allocation, required_section_kind,
    required_space, BackingAllocation, CapacityEvidence, RamAnalysisError,
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
    let dummy = required_section_kind(
        profile.architecture,
        sections,
        ".rwdata_dummy",
        SectionKind::ZeroFill,
    )?;
    let stack = required_section_kind(
        profile.architecture,
        sections,
        ".stack",
        SectionKind::ZeroFill,
    )?;
    let static_range = AddressRange::from_start_and_size(
        dummy.run_range().start(),
        stack
            .run_range()
            .start()
            .checked_sub(dummy.run_range().start())
            .ok_or(RamAnalysisError::InvalidArena {
                architecture: profile.architecture,
            })?,
    )
    .map_err(|_| RamAnalysisError::InvalidArena {
        architecture: profile.architecture,
    })?;
    let capacity_bytes = stack
        .run_range()
        .end()
        .checked_sub(dummy.run_range().start())
        .ok_or(RamAnalysisError::InvalidArena {
            architecture: profile.architecture,
        })?;
    let mut allocations = vec![arena_allocation(
        profile.architecture,
        instruction.backing_store,
        static_range,
        capacity_bytes,
        sections,
    )?];

    let reclaimed = required_space(profile, AddressSpaceKind::ReclaimedRam)?;
    let reclaimed_section = required_section_kind(
        profile.architecture,
        sections,
        ".dram2_uninit",
        SectionKind::ZeroFill,
    )?;
    allocations.push(BackingAllocation::new(
        reclaimed.backing_store,
        CapacityEvidence::Known(fixed_capacity(profile, reclaimed)?),
        reclaimed_section.run_range().byte_len(),
        0,
    ));

    let data_cache = required_space(profile, AddressSpaceKind::DataCacheRam)?;
    allocations.push(BackingAllocation::new(
        data_cache.backing_store,
        CapacityEvidence::Known(fixed_capacity(profile, data_cache)?),
        0,
        0,
    ));

    for retention in profile
        .address_spaces
        .iter()
        .filter(|space| space.kind == AddressSpaceKind::RetentionRam)
    {
        allocations.push(fixed_space_allocation(
            profile.architecture,
            profile,
            retention,
            sections,
        )?);
    }

    let external = required_space(profile, AddressSpaceKind::ExternalPsram)?;
    let capacity = match external.geometry {
        AddressSpaceGeometry::Fixed(range) => CapacityEvidence::Known(range.byte_len()),
        AddressSpaceGeometry::FixedCapacity { bytes } => CapacityEvidence::Known(bytes),
        AddressSpaceGeometry::RuntimeDetected => CapacityEvidence::RuntimeDetected,
        AddressSpaceGeometry::LinkerDefined => {
            return Err(RamAnalysisError::InvalidAddressSpaceTopology {
                profile: profile.id.as_str(),
                kind: external.kind,
            });
        }
    };
    allocations.push(BackingAllocation::new(
        external.backing_store,
        capacity,
        0,
        0,
    ));
    Ok(allocations)
}
