use super::{MemoryProfile, MemoryProfileId, ValidationError};
use crate::{
    AddressRange, AddressSpace, AddressSpaceGeometry, AddressSpaceId, AddressSpaceKind, Alignment,
    ArtifactError, BackingStoreId, FirmwarePlacement, MemoryRegion, MemoryRegionId,
    ProcessorArchitecture, RegionOwner, RegionRetention, RegionRole, ReservationAccounting,
    ReservationCharge, ReservationId, ReservationPoolId, ReservationTotals, RuntimeReservation,
    TransportCompatibility, TransportEnvelope,
};

const ONE_BYTE: Alignment = match Alignment::try_new(1) {
    Ok(value) => value,
    Err(_) => unreachable!(),
};
const FLASH: AddressSpaceId = AddressSpaceId("flash");
const RAM: AddressSpaceId = AddressSpaceId("ram");
const FIRMWARE: MemoryRegionId = MemoryRegionId("firmware");

const fn region(
    id: &'static str,
    address_space: AddressSpaceId,
    range: AddressRange,
    owner: RegionOwner,
    retention: RegionRetention,
    role: RegionRole,
) -> MemoryRegion {
    MemoryRegion {
        id: MemoryRegionId(id),
        address_space,
        range,
        alignment: ONE_BYTE,
        owner,
        retention,
        role,
    }
}

fn profile(
    address_spaces: &'static [AddressSpace],
    regions: &'static [MemoryRegion],
    reservations: &'static [RuntimeReservation],
) -> MemoryProfile {
    MemoryProfile {
        id: MemoryProfileId("test"),
        architecture: ProcessorArchitecture::ThumbV7em,
        address_spaces,
        regions,
        firmware: FirmwarePlacement {
            address_space: FLASH,
            firmware_owned_region: FIRMWARE,
            transport_envelope: TransportEnvelope {
                range: AddressRange::new(0x1000, 0x2000),
                compatibility: TransportCompatibility::ExactFirmwareRegion,
            },
        },
        journals: &[],
        runtime_reservations: reservations,
    }
}

#[test]
fn overlapping_regions_in_one_space_are_rejected() {
    static SPACES: [AddressSpace; 1] = [AddressSpace {
        id: FLASH,
        kind: AddressSpaceKind::InternalFlash,
        geometry: AddressSpaceGeometry::Fixed(AddressRange::new(0, 0x4000)),
        backing_store: BackingStoreId("flash-chip"),
        backing_offset: 0,
    }];
    static REGIONS: [MemoryRegion; 2] = [
        region(
            "firmware",
            FLASH,
            AddressRange::new(0x1000, 0x2000),
            RegionOwner::FirmwareImage,
            RegionRetention::ReplaceWithFirmware,
            RegionRole::FirmwareImage,
        ),
        region(
            "identity",
            FLASH,
            AddressRange::new(0x1F00, 0x2100),
            RegionOwner::DeviceIdentity,
            RegionRetention::PreserveAcrossFirmwareUpdate,
            RegionRole::NodeIdentity,
        ),
    ];
    assert_eq!(
        profile(&SPACES, &REGIONS, &[]).validate(),
        Err(ValidationError::OverlappingRegions {
            first: FIRMWARE,
            second: MemoryRegionId("identity"),
            address_space: FLASH,
        })
    );
}

#[test]
fn semantic_region_lookup_rejects_missing_and_ambiguous_roles() {
    static SPACES: [AddressSpace; 1] = [AddressSpace {
        id: FLASH,
        kind: AddressSpaceKind::InternalFlash,
        geometry: AddressSpaceGeometry::Fixed(AddressRange::new(0, 0x4000)),
        backing_store: BackingStoreId("flash-chip"),
        backing_offset: 0,
    }];
    static REGIONS: [MemoryRegion; 4] = [
        region(
            "firmware",
            FLASH,
            AddressRange::new(0x1000, 0x2000),
            RegionOwner::FirmwareImage,
            RegionRetention::ReplaceWithFirmware,
            RegionRole::FirmwareImage,
        ),
        region(
            "node-a",
            FLASH,
            AddressRange::new(0x2000, 0x2800),
            RegionOwner::DeviceIdentity,
            RegionRetention::PreserveAcrossFirmwareUpdate,
            RegionRole::NodeIdentity,
        ),
        region(
            "node-b",
            FLASH,
            AddressRange::new(0x2800, 0x3000),
            RegionOwner::DeviceIdentity,
            RegionRetention::PreserveAcrossFirmwareUpdate,
            RegionRole::NodeIdentity,
        ),
        region(
            "firmware-update",
            FLASH,
            AddressRange::new(0x3000, 0x4000),
            RegionOwner::FirmwareImage,
            RegionRetention::ReplaceWithFirmware,
            RegionRole::FirmwareUpdateSlot,
        ),
    ];
    let profile = profile(&SPACES, &REGIONS, &[]);

    assert_eq!(
        profile.unique_region_for_role(RegionRole::FirmwareImage),
        Ok(&REGIONS[0])
    );
    assert_eq!(
        profile.unique_region_for_role(RegionRole::FirmwareUpdateSlot),
        Ok(&REGIONS[3])
    );
    assert_eq!(
        profile.unique_region_for_role(RegionRole::BleIdentity),
        Err(crate::RegionRoleLookupError::Missing {
            role: RegionRole::BleIdentity,
        })
    );
    assert_eq!(
        profile.unique_region_for_role(RegionRole::NodeIdentity),
        Err(crate::RegionRoleLookupError::Ambiguous {
            role: RegionRole::NodeIdentity,
            first: MemoryRegionId("node-a"),
            second: MemoryRegionId("node-b"),
        })
    );
}

#[test]
fn address_space_kind_lookup_rejects_missing_and_ambiguous_kinds() {
    const OTHER_FLASH: AddressSpaceId = AddressSpaceId("other-flash");
    static NO_INTERNAL_FLASH: [AddressSpace; 1] = [AddressSpace {
        id: RAM,
        kind: AddressSpaceKind::InternalRam,
        geometry: AddressSpaceGeometry::FixedCapacity { bytes: 0x4000 },
        backing_store: BackingStoreId("sram"),
        backing_offset: 0,
    }];
    static TWO_INTERNAL_FLASH: [AddressSpace; 2] = [
        AddressSpace {
            id: FLASH,
            kind: AddressSpaceKind::InternalFlash,
            geometry: AddressSpaceGeometry::Fixed(AddressRange::new(0, 0x4000)),
            backing_store: BackingStoreId("flash-chip"),
            backing_offset: 0,
        },
        AddressSpace {
            id: OTHER_FLASH,
            kind: AddressSpaceKind::InternalFlash,
            geometry: AddressSpaceGeometry::Fixed(AddressRange::new(0x4000, 0x8000)),
            backing_store: BackingStoreId("flash-chip"),
            backing_offset: 0x4000,
        },
    ];

    assert_eq!(
        profile(&NO_INTERNAL_FLASH, &[], &[])
            .unique_address_space_for_kind(AddressSpaceKind::InternalFlash),
        Err(crate::AddressSpaceKindLookupError::Missing {
            kind: AddressSpaceKind::InternalFlash,
        })
    );
    assert_eq!(
        profile(&TWO_INTERNAL_FLASH, &[], &[])
            .unique_address_space_for_kind(AddressSpaceKind::InternalFlash),
        Err(crate::AddressSpaceKindLookupError::Ambiguous {
            kind: AddressSpaceKind::InternalFlash,
            first: FLASH,
            second: OTHER_FLASH,
        })
    );
}

#[test]
fn region_boundaries_must_satisfy_the_declared_alignment() {
    const ALIGN_256: Alignment = match Alignment::try_new(256) {
        Ok(value) => value,
        Err(_) => unreachable!(),
    };
    static SPACES: [AddressSpace; 1] = [AddressSpace {
        id: FLASH,
        kind: AddressSpaceKind::InternalFlash,
        geometry: AddressSpaceGeometry::Fixed(AddressRange::new(0, 0x4000)),
        backing_store: BackingStoreId("flash-chip"),
        backing_offset: 0,
    }];
    static REGIONS: [MemoryRegion; 1] = [MemoryRegion {
        id: FIRMWARE,
        address_space: FLASH,
        range: AddressRange::new(0x1001, 0x2000),
        alignment: ALIGN_256,
        owner: RegionOwner::FirmwareImage,
        retention: RegionRetention::ReplaceWithFirmware,
        role: RegionRole::FirmwareImage,
    }];
    assert_eq!(
        profile(&SPACES, &REGIONS, &[]).validate(),
        Err(ValidationError::MisalignedRegion {
            region: FIRMWARE,
            alignment: ALIGN_256,
        })
    );
}

#[test]
fn aliased_address_spaces_detect_physical_overlap() {
    const IRAM: AddressSpaceId = AddressSpaceId("iram");
    const DRAM: AddressSpaceId = AddressSpaceId("dram");
    static SPACES: [AddressSpace; 3] = [
        AddressSpace {
            id: FLASH,
            kind: AddressSpaceKind::InternalFlash,
            geometry: AddressSpaceGeometry::Fixed(AddressRange::new(0, 0x4000)),
            backing_store: BackingStoreId("flash-chip"),
            backing_offset: 0,
        },
        AddressSpace {
            id: IRAM,
            kind: AddressSpaceKind::InstructionRam,
            geometry: AddressSpaceGeometry::Fixed(AddressRange::new(0x4000_0000, 0x4000_1000)),
            backing_store: BackingStoreId("sram"),
            backing_offset: 0,
        },
        AddressSpace {
            id: DRAM,
            kind: AddressSpaceKind::DataRam,
            geometry: AddressSpaceGeometry::Fixed(AddressRange::new(0x3FC8_0000, 0x3FC8_1000)),
            backing_store: BackingStoreId("sram"),
            backing_offset: 0,
        },
    ];
    static REGIONS: [MemoryRegion; 3] = [
        region(
            "firmware",
            FLASH,
            AddressRange::new(0x1000, 0x2000),
            RegionOwner::FirmwareImage,
            RegionRetention::ReplaceWithFirmware,
            RegionRole::FirmwareImage,
        ),
        region(
            "instruction",
            IRAM,
            AddressRange::new(0x4000_0000, 0x4000_0800),
            RegionOwner::FirmwareImage,
            RegionRetention::ReplaceWithFirmware,
            RegionRole::FirmwareImage,
        ),
        region(
            "data",
            DRAM,
            AddressRange::new(0x3FC8_0400, 0x3FC8_0C00),
            RegionOwner::FirmwareImage,
            RegionRetention::ReplaceWithFirmware,
            RegionRole::FirmwareImage,
        ),
    ];
    assert!(matches!(
        profile(&SPACES, &REGIONS, &[]).validate(),
        Err(ValidationError::OverlappingAliasedRegions { .. })
    ));
}

#[test]
fn shared_pool_capacity_is_counted_once() {
    const POOL: ReservationPoolId = ReservationPoolId("runtime-heap");
    static SPACES: [AddressSpace; 2] = [
        AddressSpace {
            id: FLASH,
            kind: AddressSpaceKind::InternalFlash,
            geometry: AddressSpaceGeometry::Fixed(AddressRange::new(0, 0x4000)),
            backing_store: BackingStoreId("flash-chip"),
            backing_offset: 0,
        },
        AddressSpace {
            id: RAM,
            kind: AddressSpaceKind::InternalRam,
            geometry: AddressSpaceGeometry::FixedCapacity { bytes: 0x4000 },
            backing_store: BackingStoreId("sram"),
            backing_offset: 0,
        },
    ];
    static REGIONS: [MemoryRegion; 1] = [region(
        "firmware",
        FLASH,
        AddressRange::new(0x1000, 0x2000),
        RegionOwner::FirmwareImage,
        RegionRetention::ReplaceWithFirmware,
        RegionRole::FirmwareImage,
    )];
    static RESERVATIONS: [RuntimeReservation; 3] = [
        RuntimeReservation {
            id: ReservationId("consumer-a"),
            address_space: RAM,
            bytes: 0x1000,
            accounting: ReservationAccounting::SharedPool {
                pool: POOL,
                charge: ReservationCharge::AdditionalToStatic,
            },
        },
        RuntimeReservation {
            id: ReservationId("consumer-b"),
            address_space: RAM,
            bytes: 0x1000,
            accounting: ReservationAccounting::SharedPool {
                pool: POOL,
                charge: ReservationCharge::AdditionalToStatic,
            },
        },
        RuntimeReservation {
            id: ReservationId("stack"),
            address_space: RAM,
            bytes: 0x800,
            accounting: ReservationAccounting::Dedicated {
                charge: ReservationCharge::AdditionalToStatic,
            },
        },
    ];
    let profile = profile(&SPACES, &REGIONS, &RESERVATIONS);
    assert_eq!(profile.validate(), Ok(()));
    assert_eq!(
        profile.reservation_totals(RAM),
        Ok(ReservationTotals {
            additional_bytes: 0x1800,
            linker_counted_bytes: 0,
            external_bytes: 0,
        })
    );
}

#[test]
fn reservations_cannot_exceed_fixed_ram_capacity() {
    static SPACES: [AddressSpace; 2] = [
        AddressSpace {
            id: FLASH,
            kind: AddressSpaceKind::InternalFlash,
            geometry: AddressSpaceGeometry::Fixed(AddressRange::new(0, 0x4000)),
            backing_store: BackingStoreId("flash-chip"),
            backing_offset: 0,
        },
        AddressSpace {
            id: RAM,
            kind: AddressSpaceKind::InternalRam,
            geometry: AddressSpaceGeometry::FixedCapacity { bytes: 0x1000 },
            backing_store: BackingStoreId("sram"),
            backing_offset: 0,
        },
    ];
    static REGIONS: [MemoryRegion; 1] = [region(
        "firmware",
        FLASH,
        AddressRange::new(0x1000, 0x2000),
        RegionOwner::FirmwareImage,
        RegionRetention::ReplaceWithFirmware,
        RegionRole::FirmwareImage,
    )];
    static RESERVATIONS: [RuntimeReservation; 1] = [RuntimeReservation {
        id: ReservationId("oversized-stack"),
        address_space: RAM,
        bytes: 0x1001,
        accounting: ReservationAccounting::Dedicated {
            charge: ReservationCharge::AdditionalToStatic,
        },
    }];
    assert_eq!(
        profile(&SPACES, &REGIONS, &RESERVATIONS).validate(),
        Err(ValidationError::ReservationsExceedCapacity {
            address_space: RAM,
            reserved_bytes: 0x1001,
            capacity_bytes: 0x1000,
        })
    );
}

#[test]
fn legacy_transport_bytes_are_never_firmware_owned() {
    static SPACES: [AddressSpace; 1] = [AddressSpace {
        id: FLASH,
        kind: AddressSpaceKind::InternalFlash,
        geometry: AddressSpaceGeometry::Fixed(AddressRange::new(0, 0x4000)),
        backing_store: BackingStoreId("flash-chip"),
        backing_offset: 0,
    }];
    static REGIONS: [MemoryRegion; 2] = [
        region(
            "firmware",
            FLASH,
            AddressRange::new(0x1000, 0x2000),
            RegionOwner::FirmwareImage,
            RegionRetention::ReplaceWithFirmware,
            RegionRole::FirmwareImage,
        ),
        region(
            "identity",
            FLASH,
            AddressRange::new(0x2000, 0x2100),
            RegionOwner::DeviceIdentity,
            RegionRetention::PreserveAcrossFirmwareUpdate,
            RegionRole::RemoteControlIdentity,
        ),
    ];
    let mut profile = profile(&SPACES, &REGIONS, &[]);
    profile.firmware.transport_envelope = TransportEnvelope {
        range: AddressRange::new(0x1000, 0x2100),
        compatibility: TransportCompatibility::LegacyEnvelope,
    };
    assert_eq!(profile.validate(), Ok(()));
    assert_eq!(
        profile.validate_firmware_image(AddressRange::new(0x1000, 0x2001)),
        Err(ArtifactError::OutsideFirmwareOwnedRegion {
            image: AddressRange::new(0x1000, 0x2001),
            firmware_owned: AddressRange::new(0x1000, 0x2000),
        })
    );

    profile.firmware.transport_envelope.range = AddressRange::new(0xF00, 0x2100);
    assert_eq!(
        profile.validate(),
        Err(ValidationError::TransportStartsBeforeFirmware { region: FIRMWARE })
    );
    assert_eq!(
        profile.validate_firmware_image(AddressRange::new(0x1000, 0x2000)),
        Err(ArtifactError::InvalidProfile)
    );
}

const UPDATE_SLOT_SPACES: [AddressSpace; 1] = [AddressSpace {
    id: FLASH,
    kind: AddressSpaceKind::InternalFlash,
    geometry: AddressSpaceGeometry::Fixed(AddressRange::new(0, 0x4000)),
    backing_store: BackingStoreId("flash-chip"),
    backing_offset: 0,
}];

const SLOT_A: MemoryRegion = region(
    "firmware",
    FLASH,
    AddressRange::new(0x1000, 0x2000),
    RegionOwner::FirmwareImage,
    RegionRetention::ReplaceWithFirmware,
    RegionRole::FirmwareImage,
);

const fn slot_b(
    range: AddressRange,
    owner: RegionOwner,
    retention: RegionRetention,
) -> MemoryRegion {
    region(
        "firmware-update",
        FLASH,
        range,
        owner,
        retention,
        RegionRole::FirmwareUpdateSlot,
    )
}

const fn boot_selection(owner: RegionOwner, retention: RegionRetention) -> MemoryRegion {
    region(
        "otadata",
        FLASH,
        AddressRange::new(0x3000, 0x3100),
        owner,
        retention,
        RegionRole::BootSelection,
    )
}

#[test]
fn update_slots_must_hold_the_firmware_owned_image() {
    static SHORT_SLOT: [MemoryRegion; 3] = [
        SLOT_A,
        slot_b(
            AddressRange::new(0x2000, 0x2F00),
            RegionOwner::FirmwareImage,
            RegionRetention::ReplaceWithFirmware,
        ),
        boot_selection(RegionOwner::Platform, RegionRetention::ReplaceWithFirmware),
    ];
    static EQUAL_SLOT: [MemoryRegion; 3] = [
        SLOT_A,
        slot_b(
            AddressRange::new(0x2000, 0x3000),
            RegionOwner::FirmwareImage,
            RegionRetention::ReplaceWithFirmware,
        ),
        boot_selection(RegionOwner::Platform, RegionRetention::ReplaceWithFirmware),
    ];

    assert_eq!(
        profile(&UPDATE_SLOT_SPACES, &SHORT_SLOT, &[]).validate(),
        Err(ValidationError::UpdateSlotSmallerThanFirmware {
            slot: MemoryRegionId("firmware-update"),
            firmware_owned: FIRMWARE,
        })
    );

    let equal = profile(&UPDATE_SLOT_SPACES, &EQUAL_SLOT, &[]);
    assert_eq!(equal.validate(), Ok(()));
    assert_eq!(
        equal.validate_firmware_image(AddressRange::new(0x1000, 0x2000)),
        Ok(())
    );
    assert_eq!(
        equal.validate_firmware_image(AddressRange::new(0x1000, 0x2001)),
        Err(ArtifactError::OutsideFirmwareOwnedRegion {
            image: AddressRange::new(0x1000, 0x2001),
            firmware_owned: AddressRange::new(0x1000, 0x2000),
        })
    );
}

#[test]
fn update_slots_need_a_boot_selection() {
    static NO_BOOT_SELECTION: [MemoryRegion; 2] = [
        SLOT_A,
        slot_b(
            AddressRange::new(0x2000, 0x3000),
            RegionOwner::FirmwareImage,
            RegionRetention::ReplaceWithFirmware,
        ),
    ];
    assert_eq!(
        profile(&UPDATE_SLOT_SPACES, &NO_BOOT_SELECTION, &[]).validate(),
        Err(ValidationError::MissingBootSelection {
            slot: MemoryRegionId("firmware-update"),
        })
    );
}

#[test]
fn update_slots_must_share_firmware_ownership() {
    static WRONG_OWNER: [MemoryRegion; 3] = [
        SLOT_A,
        slot_b(
            AddressRange::new(0x2000, 0x3000),
            RegionOwner::DeviceIdentity,
            RegionRetention::ReplaceWithFirmware,
        ),
        boot_selection(RegionOwner::Platform, RegionRetention::ReplaceWithFirmware),
    ];
    static WRONG_RETENTION: [MemoryRegion; 3] = [
        SLOT_A,
        slot_b(
            AddressRange::new(0x2000, 0x3000),
            RegionOwner::FirmwareImage,
            RegionRetention::PreserveAcrossFirmwareUpdate,
        ),
        boot_selection(RegionOwner::Platform, RegionRetention::ReplaceWithFirmware),
    ];

    for regions in [&WRONG_OWNER, &WRONG_RETENTION] {
        assert_eq!(
            profile(&UPDATE_SLOT_SPACES, regions, &[]).validate(),
            Err(ValidationError::InvalidUpdateSlot {
                region: MemoryRegionId("firmware-update"),
            })
        );
    }
}

#[test]
fn boot_selection_must_be_platform_owned_and_replaceable() {
    static WRONG_OWNER: [MemoryRegion; 3] = [
        SLOT_A,
        slot_b(
            AddressRange::new(0x2000, 0x3000),
            RegionOwner::FirmwareImage,
            RegionRetention::ReplaceWithFirmware,
        ),
        boot_selection(RegionOwner::Radio, RegionRetention::ReplaceWithFirmware),
    ];
    static WRONG_RETENTION: [MemoryRegion; 3] = [
        SLOT_A,
        slot_b(
            AddressRange::new(0x2000, 0x3000),
            RegionOwner::FirmwareImage,
            RegionRetention::ReplaceWithFirmware,
        ),
        boot_selection(
            RegionOwner::Platform,
            RegionRetention::PreserveAcrossFirmwareUpdate,
        ),
    ];

    for regions in [&WRONG_OWNER, &WRONG_RETENTION] {
        assert_eq!(
            profile(&UPDATE_SLOT_SPACES, regions, &[]).validate(),
            Err(ValidationError::InvalidBootSelection {
                region: MemoryRegionId("otadata"),
            })
        );
    }
}
