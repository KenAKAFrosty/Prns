use super::*;
use crate::{AddressSpaceId, ArtifactError, BackingStoreId, MemoryRegionId, ReservationTotals};

#[test]
fn legacy_transport_envelopes_do_not_expand_firmware_ownership() {
    for profile in [&T_ECHO_S140_V6, &T_ECHO_S140_V7, &T096, &T114, &T1000_E] {
        assert_eq!(
            profile.firmware.transport_envelope.compatibility,
            crate::TransportCompatibility::LegacyEnvelope
        );
        let firmware = profile.region(MemoryRegionId("firmware"));
        assert!(firmware.is_some());
        if let Some(firmware) = firmware.map(|region| region.range) {
            assert!(profile.firmware.transport_envelope.range.end() > firmware.end());
            assert_eq!(
                profile.validate_firmware_image(AddressRange::new(
                    firmware.start(),
                    firmware.end() + 1,
                )),
                Err(ArtifactError::OutsideFirmwareOwnedRegion {
                    image: AddressRange::new(firmware.start(), firmware.end() + 1),
                    firmware_owned: firmware,
                })
            );
        }
    }
}

#[test]
fn minimum_runtime_stack_is_additional_to_static_ram() {
    assert_eq!(
        T_ECHO_S140_V6.reservation_totals(RAM),
        Ok(ReservationTotals {
            additional_bytes: 68 * KIB,
            linker_counted_bytes: 0,
            external_bytes: 0,
        })
    );
}

#[test]
fn base_and_rak_topologies_slot_into_the_existing_arm_contract() {
    const EXTERNAL_QSPI: AddressSpaceId = AddressSpaceId("external-qspi");
    const BASE_SPACES: [AddressSpace; 3] = [
        AddressSpace {
            id: FLASH,
            kind: AddressSpaceKind::InternalFlash,
            geometry: AddressSpaceGeometry::Fixed(AddressRange::new(0, MIB)),
            backing_store: FLASH_BACKING,
            backing_offset: 0,
        },
        AddressSpace {
            id: RAM,
            kind: AddressSpaceKind::InternalRam,
            geometry: AddressSpaceGeometry::Fixed(AddressRange::new(0x2000_0000, 0x2004_0000)),
            backing_store: INTERNAL_RAM_BACKING,
            backing_offset: 0,
        },
        AddressSpace {
            id: EXTERNAL_QSPI,
            kind: AddressSpaceKind::ExternalStorage,
            geometry: AddressSpaceGeometry::FixedCapacity { bytes: 8 * MIB },
            backing_store: BackingStoreId("base-qspi"),
            backing_offset: 0,
        },
    ];
    const RAK_SPACES: [AddressSpace; 2] = [BASE_SPACES[0], BASE_SPACES[1]];
    const SYNTHETIC_REGIONS: [MemoryRegion; 1] = [region(
        "firmware",
        FLASH,
        0x26000,
        0xE0000,
        RegionOwner::FirmwareImage,
        RegionRetention::ReplaceWithFirmware,
        RegionRole::FirmwareImage,
    )];
    const BASE_EXTERNAL_RESERVATIONS: [RuntimeReservation; 1] = [RuntimeReservation {
        id: ReservationId("external-state"),
        address_space: EXTERNAL_QSPI,
        bytes: 2 * MIB,
        accounting: ReservationAccounting::External,
    }];
    const BASE: MemoryProfile = MemoryProfile {
        id: MemoryProfileId("synthetic-base-duo"),
        architecture: ProcessorArchitecture::ThumbV7em,
        address_spaces: &BASE_SPACES,
        regions: &SYNTHETIC_REGIONS,
        firmware: firmware_placement(0x26000, 0xE0000, 0xE0000),
        journals: &[],
        runtime_reservations: &BASE_EXTERNAL_RESERVATIONS,
    };
    const RAK: MemoryProfile = MemoryProfile {
        id: MemoryProfileId("synthetic-rak4631"),
        architecture: ProcessorArchitecture::ThumbV7em,
        address_spaces: &RAK_SPACES,
        regions: &SYNTHETIC_REGIONS,
        firmware: firmware_placement(0x26000, 0xE0000, 0xE0000),
        journals: &[],
        runtime_reservations: &[],
    };

    assert_eq!(BASE.validate(), Ok(()));
    assert_eq!(RAK.validate(), Ok(()));
    assert_eq!(BASE.architecture, T_ECHO_S140_V6.architecture);
    assert_eq!(RAK.architecture, T_ECHO_S140_V6.architecture);
    assert_eq!(
        BASE.reservation_totals(EXTERNAL_QSPI),
        Ok(ReservationTotals {
            additional_bytes: 0,
            linker_counted_bytes: 0,
            external_bytes: 2 * MIB,
        })
    );
}
