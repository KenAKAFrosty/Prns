use core::fmt;

use super::super::{
    AddressRange, AddressSpace, AddressSpaceGeometry, AddressSpaceId, Alignment, ArtifactError,
    BackingStoreId, MemoryRegionId, RegionOwner, RegionRetention, RegionRole,
    ReservationAccounting, ReservationId, ReservationPoolId, TransportCompatibility,
};
use super::MemoryProfile;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationError {
    DuplicateAddressSpace {
        id: AddressSpaceId,
    },
    DuplicateRegion {
        id: MemoryRegionId,
    },
    DuplicateReservation {
        id: ReservationId,
    },
    UnknownRegionAddressSpace {
        region: MemoryRegionId,
        address_space: AddressSpaceId,
    },
    EmptyAddressSpace {
        address_space: AddressSpaceId,
    },
    RegionOutsideAddressSpace {
        region: MemoryRegionId,
        address_space: AddressSpaceId,
    },
    MisalignedRegion {
        region: MemoryRegionId,
        alignment: Alignment,
    },
    OverlappingRegions {
        first: MemoryRegionId,
        second: MemoryRegionId,
        address_space: AddressSpaceId,
    },
    OverlappingAliasedRegions {
        first: MemoryRegionId,
        second: MemoryRegionId,
        backing_store: BackingStoreId,
    },
    BackingAddressOverflow {
        backing_store: BackingStoreId,
    },
    UnknownFirmwareAddressSpace {
        address_space: AddressSpaceId,
    },
    UnknownFirmwareRegion {
        region: MemoryRegionId,
    },
    InvalidFirmwareRegion {
        region: MemoryRegionId,
    },
    TransportOutsideAddressSpace {
        address_space: AddressSpaceId,
    },
    TransportDoesNotContainFirmware {
        region: MemoryRegionId,
    },
    TransportStartsBeforeFirmware {
        region: MemoryRegionId,
    },
    MisalignedTransportEnvelope {
        region: MemoryRegionId,
        alignment: Alignment,
    },
    InexactCurrentTransportEnvelope {
        region: MemoryRegionId,
    },
    RedundantLegacyTransportEnvelope {
        region: MemoryRegionId,
    },
    UnknownJournalRegion {
        region: MemoryRegionId,
    },
    InvalidJournalRegion {
        region: MemoryRegionId,
    },
    InvalidJournalLayout {
        region: MemoryRegionId,
    },
    UnknownReservationAddressSpace {
        reservation: ReservationId,
        address_space: AddressSpaceId,
    },
    InvalidReservation {
        reservation: ReservationId,
    },
    InvalidExternalAccounting {
        reservation: ReservationId,
        address_space: AddressSpaceId,
    },
    InconsistentSharedPool {
        pool: ReservationPoolId,
    },
    ReservationTotalOverflow {
        address_space: AddressSpaceId,
    },
    ReservationsExceedCapacity {
        address_space: AddressSpaceId,
        reserved_bytes: u64,
        capacity_bytes: u64,
    },
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid embedded memory profile: {self:?}")
    }
}

impl MemoryProfile {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.validate_unique_ids()?;
        self.validate_spaces_and_regions()?;
        self.validate_region_overlaps()?;
        self.validate_firmware_placement()?;
        self.validate_journals()?;
        self.validate_reservations()
    }

    pub fn validate_firmware_image(&self, image: AddressRange) -> Result<(), ArtifactError> {
        if self.validate().is_err() {
            return Err(ArtifactError::InvalidProfile);
        }
        let Some(region) = self.region(self.firmware.firmware_owned_region) else {
            return Err(ArtifactError::InvalidProfile);
        };
        if region.range.contains(image) {
            Ok(())
        } else {
            Err(ArtifactError::OutsideFirmwareOwnedRegion {
                image,
                firmware_owned: region.range,
            })
        }
    }

    fn validate_unique_ids(&self) -> Result<(), ValidationError> {
        for (index, space) in self.address_spaces.iter().enumerate() {
            if self.address_spaces[..index]
                .iter()
                .any(|prior| prior.id == space.id)
            {
                return Err(ValidationError::DuplicateAddressSpace { id: space.id });
            }
        }
        for (index, region) in self.regions.iter().enumerate() {
            if self.regions[..index]
                .iter()
                .any(|prior| prior.id == region.id)
            {
                return Err(ValidationError::DuplicateRegion { id: region.id });
            }
        }
        for (index, reservation) in self.runtime_reservations.iter().enumerate() {
            if self.runtime_reservations[..index]
                .iter()
                .any(|prior| prior.id == reservation.id)
            {
                return Err(ValidationError::DuplicateReservation { id: reservation.id });
            }
        }
        Ok(())
    }

    fn validate_spaces_and_regions(&self) -> Result<(), ValidationError> {
        for space in self.address_spaces {
            if matches!(
                space.geometry,
                AddressSpaceGeometry::FixedCapacity { bytes: 0 }
            ) {
                return Err(ValidationError::EmptyAddressSpace {
                    address_space: space.id,
                });
            }
        }
        for region in self.regions {
            let Some(space) = self.address_space(region.address_space) else {
                return Err(ValidationError::UnknownRegionAddressSpace {
                    region: region.id,
                    address_space: region.address_space,
                });
            };
            if !space.geometry.contains(region.range) {
                return Err(ValidationError::RegionOutsideAddressSpace {
                    region: region.id,
                    address_space: space.id,
                });
            }
            if !region.range.is_aligned(region.alignment) {
                return Err(ValidationError::MisalignedRegion {
                    region: region.id,
                    alignment: region.alignment,
                });
            }
        }
        Ok(())
    }

    fn validate_region_overlaps(&self) -> Result<(), ValidationError> {
        for (index, first) in self.regions.iter().enumerate() {
            let Some(first_space) = self.address_space(first.address_space) else {
                return Err(ValidationError::UnknownRegionAddressSpace {
                    region: first.id,
                    address_space: first.address_space,
                });
            };
            for second in &self.regions[index + 1..] {
                let Some(second_space) = self.address_space(second.address_space) else {
                    return Err(ValidationError::UnknownRegionAddressSpace {
                        region: second.id,
                        address_space: second.address_space,
                    });
                };
                if first.address_space == second.address_space && first.range.overlaps(second.range)
                {
                    return Err(ValidationError::OverlappingRegions {
                        first: first.id,
                        second: second.id,
                        address_space: first.address_space,
                    });
                }
                if first.address_space != second.address_space
                    && first_space.backing_store == second_space.backing_store
                    && physical_ranges_overlap(
                        first_space,
                        first.range,
                        second_space,
                        second.range,
                    )?
                {
                    return Err(ValidationError::OverlappingAliasedRegions {
                        first: first.id,
                        second: second.id,
                        backing_store: first_space.backing_store,
                    });
                }
            }
        }
        Ok(())
    }

    fn validate_firmware_placement(&self) -> Result<(), ValidationError> {
        let Some(space) = self.address_space(self.firmware.address_space) else {
            return Err(ValidationError::UnknownFirmwareAddressSpace {
                address_space: self.firmware.address_space,
            });
        };
        let Some(region) = self.region(self.firmware.firmware_owned_region) else {
            return Err(ValidationError::UnknownFirmwareRegion {
                region: self.firmware.firmware_owned_region,
            });
        };
        if region.address_space != space.id
            || region.owner != RegionOwner::FirmwareImage
            || region.retention != RegionRetention::ReplaceWithFirmware
            || region.role != RegionRole::FirmwareImage
        {
            return Err(ValidationError::InvalidFirmwareRegion { region: region.id });
        }
        if !space
            .geometry
            .contains(self.firmware.transport_envelope.range)
        {
            return Err(ValidationError::TransportOutsideAddressSpace {
                address_space: space.id,
            });
        }
        if !self
            .firmware
            .transport_envelope
            .range
            .contains(region.range)
        {
            return Err(ValidationError::TransportDoesNotContainFirmware { region: region.id });
        }
        if self.firmware.transport_envelope.range.start() != region.range.start() {
            return Err(ValidationError::TransportStartsBeforeFirmware { region: region.id });
        }
        if !self
            .firmware
            .transport_envelope
            .range
            .is_aligned(region.alignment)
        {
            return Err(ValidationError::MisalignedTransportEnvelope {
                region: region.id,
                alignment: region.alignment,
            });
        }
        match self.firmware.transport_envelope.compatibility {
            TransportCompatibility::ExactFirmwareRegion
                if self.firmware.transport_envelope.range != region.range =>
            {
                Err(ValidationError::InexactCurrentTransportEnvelope { region: region.id })
            }
            TransportCompatibility::LegacyEnvelope
                if self.firmware.transport_envelope.range == region.range =>
            {
                Err(ValidationError::RedundantLegacyTransportEnvelope { region: region.id })
            }
            _ => Ok(()),
        }
    }

    fn validate_journals(&self) -> Result<(), ValidationError> {
        for journal in self.journals {
            let Some(region) = self.region(journal.region) else {
                return Err(ValidationError::UnknownJournalRegion {
                    region: journal.region,
                });
            };
            if region.role != RegionRole::Journal
                || region.owner != RegionOwner::LearnedState
                || region.retention != RegionRetention::PreserveAcrossFirmwareUpdate
            {
                return Err(ValidationError::InvalidJournalRegion {
                    region: journal.region,
                });
            }
            let [timebase_a, timebase_b] = journal.timebase_pages;
            let [arena_a, arena_b] = journal.arenas;
            if journal.page_bytes == 0
                || timebase_a.byte_len() != journal.page_bytes
                || timebase_b.byte_len() != journal.page_bytes
                || timebase_a.end() != timebase_b.start()
                || timebase_b.end() != arena_a.start()
                || arena_a.end() != arena_b.start()
                || region.range.start() != timebase_a.start()
                || region.range.end() != arena_b.end()
                || !region.range.contains(timebase_a)
                || !region.range.contains(timebase_b)
                || !region.range.contains(arena_a)
                || !region.range.contains(arena_b)
                || [timebase_a, timebase_b, arena_a, arena_b]
                    .iter()
                    .any(|range| {
                        !range.start().is_multiple_of(journal.page_bytes)
                            || !range.end().is_multiple_of(journal.page_bytes)
                    })
            {
                return Err(ValidationError::InvalidJournalLayout {
                    region: journal.region,
                });
            }
        }
        Ok(())
    }

    fn validate_reservations(&self) -> Result<(), ValidationError> {
        for (index, reservation) in self.runtime_reservations.iter().enumerate() {
            let Some(space) = self.address_space(reservation.address_space) else {
                return Err(ValidationError::UnknownReservationAddressSpace {
                    reservation: reservation.id,
                    address_space: reservation.address_space,
                });
            };
            if reservation.bytes == 0
                || (!space.kind.is_ram()
                    && !matches!(reservation.accounting, ReservationAccounting::External))
            {
                return Err(ValidationError::InvalidReservation {
                    reservation: reservation.id,
                });
            }
            if matches!(reservation.accounting, ReservationAccounting::External)
                != space.kind.is_external()
            {
                return Err(ValidationError::InvalidExternalAccounting {
                    reservation: reservation.id,
                    address_space: space.id,
                });
            }
            if let ReservationAccounting::SharedPool { pool, charge } = reservation.accounting {
                for prior in &self.runtime_reservations[..index] {
                    if let ReservationAccounting::SharedPool {
                        pool: prior_pool,
                        charge: prior_charge,
                    } = prior.accounting
                    {
                        if prior_pool == pool
                            && (prior.address_space != reservation.address_space
                                || prior.bytes != reservation.bytes
                                || prior_charge != charge)
                        {
                            return Err(ValidationError::InconsistentSharedPool { pool });
                        }
                    }
                }
            }
        }
        for space in self.address_spaces {
            let Some(capacity) = space.geometry.fixed_capacity() else {
                continue;
            };
            let totals = self.reservation_totals(space.id)?;
            let charged = if space.kind.is_external() {
                totals.external_bytes
            } else {
                totals
                    .additional_bytes
                    .checked_add(totals.linker_counted_bytes)
                    .ok_or(ValidationError::ReservationTotalOverflow {
                        address_space: space.id,
                    })?
            };
            if charged > capacity {
                return Err(ValidationError::ReservationsExceedCapacity {
                    address_space: space.id,
                    reserved_bytes: charged,
                    capacity_bytes: capacity,
                });
            }
        }
        Ok(())
    }
}

fn physical_ranges_overlap(
    first_space: &AddressSpace,
    first: AddressRange,
    second_space: &AddressSpace,
    second: AddressRange,
) -> Result<bool, ValidationError> {
    let Some(first_start) = first_space.geometry.physical_offset(first.start()) else {
        return Ok(false);
    };
    let Some(first_end) = first_space.geometry.physical_offset(first.end()) else {
        return Ok(false);
    };
    let Some(second_start) = second_space.geometry.physical_offset(second.start()) else {
        return Ok(false);
    };
    let Some(second_end) = second_space.geometry.physical_offset(second.end()) else {
        return Ok(false);
    };
    let first_physical = AddressRange::from_start_and_size(
        first_space.backing_offset.checked_add(first_start).ok_or(
            ValidationError::BackingAddressOverflow {
                backing_store: first_space.backing_store,
            },
        )?,
        first_end - first_start,
    )
    .map_err(|_| ValidationError::BackingAddressOverflow {
        backing_store: first_space.backing_store,
    })?;
    let second_physical = AddressRange::from_start_and_size(
        second_space
            .backing_offset
            .checked_add(second_start)
            .ok_or(ValidationError::BackingAddressOverflow {
                backing_store: second_space.backing_store,
            })?,
        second_end - second_start,
    )
    .map_err(|_| ValidationError::BackingAddressOverflow {
        backing_store: second_space.backing_store,
    })?;
    Ok(first_physical.overlaps(second_physical))
}
