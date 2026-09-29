use personal_hopspot_memory::{AddressSpaceGeometry, AddressSpaceKind, MemoryProfile};

use super::{fixed_space_allocation, required_space, BackingAllocation, RamAnalysisError};
use crate::analysis::AllocatedSection;

pub(super) fn allocations(
    profile: &MemoryProfile,
    sections: &[AllocatedSection],
) -> Result<Vec<BackingAllocation>, RamAnalysisError> {
    let ram = required_space(profile, AddressSpaceKind::InternalRam)?;
    if !matches!(ram.geometry, AddressSpaceGeometry::Fixed(_)) {
        return Err(RamAnalysisError::InvalidAddressSpaceTopology {
            profile: profile.id.as_str(),
            kind: ram.kind,
        });
    }
    let allocation = fixed_space_allocation(profile.architecture, profile, ram, sections)?;
    if allocation.static_section_bytes == 0 {
        return Err(RamAnalysisError::InvalidArena {
            architecture: profile.architecture,
        });
    }
    Ok(vec![allocation])
}
