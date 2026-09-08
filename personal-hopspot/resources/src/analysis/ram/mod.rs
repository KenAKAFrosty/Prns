mod riscv32imac;
mod thumbv7em;
mod xtensa;

#[cfg(test)]
mod tests;

use personal_hopspot_memory::{
    AddressRange, AddressSpace, AddressSpaceGeometry, AddressSpaceId, AddressSpaceKind,
    BackingStoreId, MemoryProfile, ProcessorArchitecture, ReservationTotals, ValidationError,
};
use thiserror::Error;

use super::{AllocatedSection, SectionKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RamCapacity {
    Known { bytes: u64, headroom_bytes: u64 },
    RuntimeDetected,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RamBackingUsage {
    pub(crate) backing_store: BackingStoreId,
    pub(crate) address_spaces: Vec<AddressSpaceId>,
    pub(crate) capacity: RamCapacity,
    pub(crate) static_section_bytes: u64,
    pub(crate) linker_padding_bytes: u64,
    pub(crate) additional_reservation_bytes: u64,
    pub(crate) included_reservation_bytes: u64,
    pub(crate) external_reservation_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CapacityEvidence {
    Known(u64),
    RuntimeDetected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct BackingAllocation {
    backing_store: BackingStoreId,
    capacity: CapacityEvidence,
    static_section_bytes: u64,
    linker_padding_bytes: u64,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum RamAnalysisError {
    #[error("memory profile {profile:?} is invalid: {error}")]
    InvalidProfile {
        profile: &'static str,
        error: ValidationError,
    },
    #[error("memory profile {profile:?} has invalid {kind:?} topology")]
    InvalidAddressSpaceTopology {
        profile: &'static str,
        kind: AddressSpaceKind,
    },
    #[error("memory profile {profile:?} aliases {first:?} and {second:?} to different stores")]
    MismatchedBackingStores {
        profile: &'static str,
        first: AddressSpaceId,
        second: AddressSpaceId,
    },
    #[error("{architecture:?} evidence is missing section {section:?}")]
    MissingSection {
        architecture: ProcessorArchitecture,
        section: &'static str,
    },
    #[error("{architecture:?} evidence contains duplicate section {section:?}")]
    DuplicateSection {
        architecture: ProcessorArchitecture,
        section: &'static str,
    },
    #[error("{architecture:?} section {section:?} has an invalid memory role")]
    InvalidSectionRole {
        architecture: ProcessorArchitecture,
        section: String,
    },
    #[error("{architecture:?} evidence has an invalid RAM arena")]
    InvalidArena { architecture: ProcessorArchitecture },
    #[error("RAM accounting overflowed for backing store {backing_store:?}")]
    ArithmeticOverflow { backing_store: BackingStoreId },
    #[error("RAM evidence contains duplicate backing store {backing_store:?}")]
    DuplicateBackingEvidence { backing_store: BackingStoreId },
    #[error("RAM evidence references unknown backing store {backing_store:?}")]
    UnknownBackingEvidence { backing_store: BackingStoreId },
    #[error("RAM evidence is missing backing store {backing_store:?}")]
    MissingBackingEvidence { backing_store: BackingStoreId },
    #[error(
        "backing store {backing_store:?} has {included_bytes} linker-counted reservation bytes but only {static_bytes} static bytes"
    )]
    IncludedReservationExceedsStatic {
        backing_store: BackingStoreId,
        included_bytes: u64,
        static_bytes: u64,
    },
    #[error(
        "backing store {backing_store:?} uses {used_bytes} bytes but only {capacity_bytes} bytes are available"
    )]
    RamOverflow {
        backing_store: BackingStoreId,
        used_bytes: u64,
        capacity_bytes: u64,
    },
}

pub(crate) fn analyze(
    profile: &MemoryProfile,
    sections: &[AllocatedSection],
) -> Result<Vec<RamBackingUsage>, RamAnalysisError> {
    profile
        .validate()
        .map_err(|error| RamAnalysisError::InvalidProfile {
            profile: profile.id.as_str(),
            error,
        })?;
    let allocations = match profile.architecture {
        ProcessorArchitecture::ThumbV7em => thumbv7em::allocations(profile, sections)?,
        ProcessorArchitecture::RiscV32Imac => riscv32imac::allocations(profile, sections)?,
        ProcessorArchitecture::XtensaEsp32S3 => xtensa::allocations(profile, sections)?,
    };
    validate_allocation_set(profile, &allocations)?;
    allocations
        .into_iter()
        .map(|allocation| usage(profile, allocation))
        .collect()
}

fn usage(
    profile: &MemoryProfile,
    allocation: BackingAllocation,
) -> Result<RamBackingUsage, RamAnalysisError> {
    let address_spaces = profile
        .address_spaces
        .iter()
        .filter(|space| is_ram(space.kind) && space.backing_store == allocation.backing_store)
        .map(|space| space.id)
        .collect::<Vec<_>>();
    if address_spaces.is_empty() {
        return Err(RamAnalysisError::UnknownBackingEvidence {
            backing_store: allocation.backing_store,
        });
    }
    let reservations = reservation_totals(profile, allocation.backing_store, &address_spaces)?;
    if reservations.linker_counted_bytes > allocation.static_section_bytes {
        return Err(RamAnalysisError::IncludedReservationExceedsStatic {
            backing_store: allocation.backing_store,
            included_bytes: reservations.linker_counted_bytes,
            static_bytes: allocation.static_section_bytes,
        });
    }
    let capacity = match allocation.capacity {
        CapacityEvidence::Known(bytes) => {
            let used_bytes = allocation
                .static_section_bytes
                .checked_add(allocation.linker_padding_bytes)
                .and_then(|bytes| bytes.checked_add(reservations.additional_bytes))
                .and_then(|bytes| bytes.checked_add(reservations.external_bytes))
                .ok_or(RamAnalysisError::ArithmeticOverflow {
                    backing_store: allocation.backing_store,
                })?;
            let headroom_bytes =
                bytes
                    .checked_sub(used_bytes)
                    .ok_or(RamAnalysisError::RamOverflow {
                        backing_store: allocation.backing_store,
                        used_bytes,
                        capacity_bytes: bytes,
                    })?;
            RamCapacity::Known {
                bytes,
                headroom_bytes,
            }
        }
        CapacityEvidence::RuntimeDetected => RamCapacity::RuntimeDetected,
    };
    Ok(RamBackingUsage {
        backing_store: allocation.backing_store,
        address_spaces,
        capacity,
        static_section_bytes: allocation.static_section_bytes,
        linker_padding_bytes: allocation.linker_padding_bytes,
        additional_reservation_bytes: reservations.additional_bytes,
        included_reservation_bytes: reservations.linker_counted_bytes,
        external_reservation_bytes: reservations.external_bytes,
    })
}

fn reservation_totals(
    profile: &MemoryProfile,
    backing_store: BackingStoreId,
    address_spaces: &[AddressSpaceId],
) -> Result<ReservationTotals, RamAnalysisError> {
    let mut combined = ReservationTotals::default();
    for address_space in address_spaces {
        let totals = profile
            .reservation_totals(*address_space)
            .map_err(|error| RamAnalysisError::InvalidProfile {
                profile: profile.id.as_str(),
                error,
            })?;
        combined.additional_bytes = combined
            .additional_bytes
            .checked_add(totals.additional_bytes)
            .ok_or(RamAnalysisError::ArithmeticOverflow { backing_store })?;
        combined.linker_counted_bytes = combined
            .linker_counted_bytes
            .checked_add(totals.linker_counted_bytes)
            .ok_or(RamAnalysisError::ArithmeticOverflow { backing_store })?;
        combined.external_bytes = combined
            .external_bytes
            .checked_add(totals.external_bytes)
            .ok_or(RamAnalysisError::ArithmeticOverflow { backing_store })?;
    }
    Ok(combined)
}

fn validate_allocation_set(
    profile: &MemoryProfile,
    allocations: &[BackingAllocation],
) -> Result<(), RamAnalysisError> {
    for (index, allocation) in allocations.iter().enumerate() {
        if allocations[..index]
            .iter()
            .any(|prior| prior.backing_store == allocation.backing_store)
        {
            return Err(RamAnalysisError::DuplicateBackingEvidence {
                backing_store: allocation.backing_store,
            });
        }
    }
    for (index, space) in profile.address_spaces.iter().enumerate() {
        if !is_ram(space.kind)
            || profile.address_spaces[..index]
                .iter()
                .any(|prior| is_ram(prior.kind) && prior.backing_store == space.backing_store)
        {
            continue;
        }
        if !allocations
            .iter()
            .any(|allocation| allocation.backing_store == space.backing_store)
        {
            return Err(RamAnalysisError::MissingBackingEvidence {
                backing_store: space.backing_store,
            });
        }
    }
    Ok(())
}

pub(super) fn required_space(
    profile: &MemoryProfile,
    kind: AddressSpaceKind,
) -> Result<&AddressSpace, RamAnalysisError> {
    profile.unique_address_space_for_kind(kind).map_err(|_| {
        RamAnalysisError::InvalidAddressSpaceTopology {
            profile: profile.id.as_str(),
            kind,
        }
    })
}

pub(super) fn optional_space(
    profile: &MemoryProfile,
    kind: AddressSpaceKind,
) -> Result<Option<&AddressSpace>, RamAnalysisError> {
    let mut matches = profile
        .address_spaces
        .iter()
        .filter(|space| space.kind == kind);
    let Some(space) = matches.next() else {
        return Ok(None);
    };
    if matches.next().is_some() {
        return Err(RamAnalysisError::InvalidAddressSpaceTopology {
            profile: profile.id.as_str(),
            kind,
        });
    }
    Ok(Some(space))
}

pub(super) fn required_section<'a>(
    architecture: ProcessorArchitecture,
    sections: &'a [AllocatedSection],
    name: &'static str,
) -> Result<&'a AllocatedSection, RamAnalysisError> {
    let mut matches = sections.iter().filter(|section| section.name() == name);
    let section = matches.next().ok_or(RamAnalysisError::MissingSection {
        architecture,
        section: name,
    })?;
    if matches.next().is_some() {
        return Err(RamAnalysisError::DuplicateSection {
            architecture,
            section: name,
        });
    }
    Ok(section)
}

pub(super) fn required_section_kind<'a>(
    architecture: ProcessorArchitecture,
    sections: &'a [AllocatedSection],
    name: &'static str,
    kind: SectionKind,
) -> Result<&'a AllocatedSection, RamAnalysisError> {
    let section = required_section(architecture, sections, name)?;
    if section.kind() != kind || (kind == SectionKind::ZeroFill && section.load_bytes() != 0) {
        return Err(RamAnalysisError::InvalidSectionRole {
            architecture,
            section: name.to_string(),
        });
    }
    Ok(section)
}

pub(super) fn optional_section_kind<'a>(
    architecture: ProcessorArchitecture,
    sections: &'a [AllocatedSection],
    name: &'static str,
    kind: SectionKind,
) -> Result<Option<&'a AllocatedSection>, RamAnalysisError> {
    let mut matches = sections.iter().filter(|section| section.name() == name);
    let Some(section) = matches.next() else {
        return Ok(None);
    };
    if matches.next().is_some() {
        return Err(RamAnalysisError::DuplicateSection {
            architecture,
            section: name,
        });
    }
    if section.kind() != kind || (kind == SectionKind::ZeroFill && section.load_bytes() != 0) {
        return Err(RamAnalysisError::InvalidSectionRole {
            architecture,
            section: name.to_string(),
        });
    }
    Ok(Some(section))
}

pub(super) fn fixed_capacity(
    profile: &MemoryProfile,
    space: &AddressSpace,
) -> Result<u64, RamAnalysisError> {
    match space.geometry {
        AddressSpaceGeometry::Fixed(range) => Ok(range.byte_len()),
        AddressSpaceGeometry::FixedCapacity { bytes } => Ok(bytes),
        AddressSpaceGeometry::LinkerDefined | AddressSpaceGeometry::RuntimeDetected => {
            Err(RamAnalysisError::InvalidAddressSpaceTopology {
                profile: profile.id.as_str(),
                kind: space.kind,
            })
        }
    }
}

pub(super) fn fixed_space_allocation(
    architecture: ProcessorArchitecture,
    profile: &MemoryProfile,
    space: &AddressSpace,
    sections: &[AllocatedSection],
) -> Result<BackingAllocation, RamAnalysisError> {
    let AddressSpaceGeometry::Fixed(bounds) = space.geometry else {
        return Ok(BackingAllocation::new(
            space.backing_store,
            CapacityEvidence::Known(fixed_capacity(profile, space)?),
            0,
            0,
        ));
    };
    let matching = sections
        .iter()
        .filter(|section| bounds.contains(section.run_range()))
        .collect::<Vec<_>>();
    for section in sections {
        if bounds.overlaps(section.run_range()) && !bounds.contains(section.run_range()) {
            return Err(RamAnalysisError::InvalidSectionRole {
                architecture,
                section: section.name().to_string(),
            });
        }
    }
    let static_section_bytes = sum_section_bytes(architecture, &matching)?;
    let linker_padding_bytes = if let Some(end) = matching
        .iter()
        .map(|section| section.run_range().end())
        .max()
    {
        end.checked_sub(bounds.start())
            .and_then(|extent| extent.checked_sub(static_section_bytes))
            .ok_or(RamAnalysisError::InvalidArena { architecture })?
    } else {
        0
    };
    Ok(BackingAllocation::new(
        space.backing_store,
        CapacityEvidence::Known(bounds.byte_len()),
        static_section_bytes,
        linker_padding_bytes,
    ))
}

pub(super) fn arena_allocation(
    architecture: ProcessorArchitecture,
    backing_store: BackingStoreId,
    static_range: AddressRange,
    capacity_bytes: u64,
    sections: &[AllocatedSection],
) -> Result<BackingAllocation, RamAnalysisError> {
    let matching = sections
        .iter()
        .filter(|section| static_range.contains(section.run_range()))
        .collect::<Vec<_>>();
    if matching.is_empty() {
        return Err(RamAnalysisError::InvalidArena { architecture });
    }
    for section in sections {
        if static_range.overlaps(section.run_range()) && !static_range.contains(section.run_range())
        {
            return Err(RamAnalysisError::InvalidSectionRole {
                architecture,
                section: section.name().to_string(),
            });
        }
    }
    let static_section_bytes = sum_section_bytes(architecture, &matching)?;
    let linker_padding_bytes = static_range
        .byte_len()
        .checked_sub(static_section_bytes)
        .ok_or(RamAnalysisError::InvalidArena { architecture })?;
    Ok(BackingAllocation::new(
        backing_store,
        CapacityEvidence::Known(capacity_bytes),
        static_section_bytes,
        linker_padding_bytes,
    ))
}

fn sum_section_bytes(
    architecture: ProcessorArchitecture,
    sections: &[&AllocatedSection],
) -> Result<u64, RamAnalysisError> {
    sections.iter().try_fold(0_u64, |total, section| {
        total
            .checked_add(section.run_range().byte_len())
            .ok_or(RamAnalysisError::InvalidArena { architecture })
    })
}

const fn is_ram(kind: AddressSpaceKind) -> bool {
    matches!(
        kind,
        AddressSpaceKind::InternalRam
            | AddressSpaceKind::InstructionRam
            | AddressSpaceKind::DataRam
            | AddressSpaceKind::ReclaimedRam
            | AddressSpaceKind::DataCacheRam
            | AddressSpaceKind::RetentionRam
            | AddressSpaceKind::ExternalPsram
    )
}

impl BackingAllocation {
    pub(super) const fn new(
        backing_store: BackingStoreId,
        capacity: CapacityEvidence,
        static_section_bytes: u64,
        linker_padding_bytes: u64,
    ) -> Self {
        Self {
            backing_store,
            capacity,
            static_section_bytes,
            linker_padding_bytes,
        }
    }
}
