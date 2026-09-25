use super::*;
use crate::{
    AddressRange, AddressSpace, AddressSpaceGeometry, AddressSpaceId, Alignment, BackingStoreId,
    FirmwarePlacement, MemoryRegion, ProcessorArchitecture, RegionOwner, RegionRetention,
    TransportCompatibility, TransportEnvelope, ESP_16_MIB_PARTITION_TABLE, HELTEC_V4,
};
use std::string::String;

const HELTEC_V4_ONLY: [MemoryProfileId; 1] = [HELTEC_V4.id];
const DUPLICATE_REGIONS: [EspPartitionBinding; 2] = [
    ESP_16_MIB_PARTITION_TABLE.partitions[0],
    ESP_16_MIB_PARTITION_TABLE.partitions[0],
];
const OUT_OF_ORDER: [EspPartitionBinding; 2] = [
    ESP_16_MIB_PARTITION_TABLE.partitions[1],
    ESP_16_MIB_PARTITION_TABLE.partitions[0],
];
const WRONG_KIND: [EspPartitionBinding; 1] = [EspPartitionBinding {
    region: MemoryRegionId("firmware"),
    name: "factory",
    kind: EspPartitionKind::NvsData,
}];

#[test]
fn duplicate_region_bindings_are_rejected() {
    let table = EspPartitionTable {
        profiles: &HELTEC_V4_ONLY,
        partitions: &DUPLICATE_REGIONS,
    };
    assert!(matches!(
        table.validate(&HELTEC_V4),
        Err(EspPartitionTableError::DuplicateRegion { .. })
    ));
}

#[test]
fn partition_bindings_must_follow_flash_order() {
    let table = EspPartitionTable {
        profiles: &HELTEC_V4_ONLY,
        partitions: &OUT_OF_ORDER,
    };
    assert!(matches!(
        table.validate(&HELTEC_V4),
        Err(EspPartitionTableError::PartitionsOutOfOrder { .. })
    ));
}

#[test]
fn partition_kinds_must_match_region_roles() {
    let table = EspPartitionTable {
        profiles: &HELTEC_V4_ONLY,
        partitions: &WRONG_KIND,
    };
    assert_eq!(
        table.validate(&HELTEC_V4),
        Err(EspPartitionTableError::PartitionKindMismatch {
            region: MemoryRegionId("firmware"),
            kind: EspPartitionKind::NvsData,
        })
    );
}

#[test]
fn invalid_tables_cannot_be_rendered() {
    let table = EspPartitionTable {
        profiles: &HELTEC_V4_ONLY,
        partitions: &DUPLICATE_REGIONS,
    };
    let mut output = String::new();

    assert!(matches!(
        table.write_csv(&HELTEC_V4, &mut output),
        Err(EspPartitionCsvError::InvalidTable(
            EspPartitionTableError::DuplicateRegion { .. }
        ))
    ));
    assert!(output.is_empty());
}

const FIXTURE_ID: MemoryProfileId = MemoryProfileId("esp-ota-fixture");
const FIXTURE_FLASH: AddressSpaceId = AddressSpaceId("internal-flash");
const FIXTURE_PAGE: Alignment = match Alignment::try_new(4096) {
    Ok(value) => value,
    Err(_) => unreachable!(),
};
const FIRMWARE: MemoryRegionId = MemoryRegionId("firmware");
const FIRMWARE_UPDATE: MemoryRegionId = MemoryRegionId("firmware-update");
const OTADATA: MemoryRegionId = MemoryRegionId("otadata");

static FIXTURE_SPACES: [AddressSpace; 1] = [AddressSpace {
    id: FIXTURE_FLASH,
    kind: AddressSpaceKind::InternalFlash,
    geometry: AddressSpaceGeometry::Fixed(AddressRange::new(0, 0x100_0000)),
    backing_store: BackingStoreId("internal-flash"),
    backing_offset: 0,
}];

const fn fixture_region(
    id: MemoryRegionId,
    range: AddressRange,
    owner: RegionOwner,
    retention: RegionRetention,
    role: RegionRole,
) -> MemoryRegion {
    MemoryRegion {
        id,
        address_space: FIXTURE_FLASH,
        range,
        alignment: FIXTURE_PAGE,
        owner,
        retention,
        role,
    }
}

const fn fixture_slot(id: MemoryRegionId, range: AddressRange, role: RegionRole) -> MemoryRegion {
    fixture_region(
        id,
        range,
        RegionOwner::FirmwareImage,
        RegionRetention::ReplaceWithFirmware,
        role,
    )
}

const fn fixture_boot_selection(range: AddressRange) -> MemoryRegion {
    fixture_region(
        OTADATA,
        range,
        RegionOwner::Platform,
        RegionRetention::ReplaceWithFirmware,
        RegionRole::BootSelection,
    )
}

fn fixture_profile(regions: &'static [MemoryRegion]) -> MemoryProfile {
    let firmware = regions
        .iter()
        .find(|region| region.id == FIRMWARE)
        .expect("the fixture carries a firmware-owned region");
    MemoryProfile {
        id: FIXTURE_ID,
        architecture: ProcessorArchitecture::XtensaEsp32S3,
        address_spaces: &FIXTURE_SPACES,
        regions,
        firmware: FirmwarePlacement {
            address_space: FIXTURE_FLASH,
            firmware_owned_region: FIRMWARE,
            transport_envelope: TransportEnvelope {
                range: firmware.range,
                compatibility: TransportCompatibility::ExactFirmwareRegion,
            },
        },
        journals: &[],
        runtime_reservations: &[],
    }
}

const SLOT_A: MemoryRegion = fixture_slot(
    FIRMWARE,
    AddressRange::new(0x1_0000, 0x74_0000),
    RegionRole::FirmwareImage,
);
const SLOT_B: MemoryRegion = fixture_slot(
    FIRMWARE_UPDATE,
    AddressRange::new(0x74_0000, 0xE7_0000),
    RegionRole::FirmwareUpdateSlot,
);

static AB_REGIONS: [MemoryRegion; 3] = [
    SLOT_A,
    SLOT_B,
    fixture_boot_selection(AddressRange::new(0xE7_0000, 0xE7_2000)),
];

const AB_PROFILES: [MemoryProfileId; 1] = [FIXTURE_ID];
const AB_PARTITIONS: [EspPartitionBinding; 3] = [
    EspPartitionBinding {
        region: FIRMWARE,
        name: "ota_0",
        kind: EspPartitionKind::OtaApplication { slot: 0 },
    },
    EspPartitionBinding {
        region: FIRMWARE_UPDATE,
        name: "ota_1",
        kind: EspPartitionKind::OtaApplication { slot: 1 },
    },
    EspPartitionBinding {
        region: OTADATA,
        name: "otadata",
        kind: EspPartitionKind::OtaData,
    },
];
const AB_TABLE: EspPartitionTable = EspPartitionTable {
    profiles: &AB_PROFILES,
    partitions: &AB_PARTITIONS,
};

#[test]
fn ota_partition_kinds_render_idf_names() {
    let mut output = String::new();
    AB_TABLE
        .write_csv(&fixture_profile(&AB_REGIONS), &mut output)
        .expect("the dual-slot fixture renders");

    assert_eq!(
        output,
        concat!(
            "ota_0,app,ota_0,0x10000,0x730000,\n",
            "ota_1,app,ota_1,0x740000,0x730000,\n",
            "otadata,data,ota,0xe70000,0x2000,\n",
        )
    );
}

#[test]
fn ota_kinds_must_match_slot_roles() {
    const OTA_ON_JOURNAL: [EspPartitionBinding; 1] = [EspPartitionBinding {
        region: MemoryRegionId("journal"),
        name: "ota_0",
        kind: EspPartitionKind::OtaApplication { slot: 0 },
    }];
    const OTA_DATA_ON_IDENTITY: [EspPartitionBinding; 1] = [EspPartitionBinding {
        region: MemoryRegionId("ble_id"),
        name: "otadata",
        kind: EspPartitionKind::OtaData,
    }];
    const FACTORY_ON_UPDATE_SLOT: [EspPartitionBinding; 1] = [EspPartitionBinding {
        region: FIRMWARE_UPDATE,
        name: "factory",
        kind: EspPartitionKind::FactoryApplication,
    }];

    assert_eq!(
        EspPartitionTable {
            profiles: &HELTEC_V4_ONLY,
            partitions: &OTA_ON_JOURNAL,
        }
        .validate(&HELTEC_V4),
        Err(EspPartitionTableError::PartitionKindMismatch {
            region: MemoryRegionId("journal"),
            kind: EspPartitionKind::OtaApplication { slot: 0 },
        })
    );
    assert_eq!(
        EspPartitionTable {
            profiles: &HELTEC_V4_ONLY,
            partitions: &OTA_DATA_ON_IDENTITY,
        }
        .validate(&HELTEC_V4),
        Err(EspPartitionTableError::PartitionKindMismatch {
            region: MemoryRegionId("ble_id"),
            kind: EspPartitionKind::OtaData,
        })
    );
    assert_eq!(
        EspPartitionTable {
            profiles: &AB_PROFILES,
            partitions: &FACTORY_ON_UPDATE_SLOT,
        }
        .validate(&fixture_profile(&AB_REGIONS)),
        Err(EspPartitionTableError::PartitionKindMismatch {
            region: FIRMWARE_UPDATE,
            kind: EspPartitionKind::FactoryApplication,
        })
    );
}

#[test]
fn application_partitions_must_start_on_a_flash_mmu_page() {
    static MISALIGNED: [MemoryRegion; 1] = [fixture_slot(
        FIRMWARE,
        AddressRange::new(0x1_1000, 0x74_1000),
        RegionRole::FirmwareImage,
    )];
    const FACTORY: [EspPartitionBinding; 1] = [EspPartitionBinding {
        region: FIRMWARE,
        name: "factory",
        kind: EspPartitionKind::FactoryApplication,
    }];

    let profile = fixture_profile(&MISALIGNED);
    assert_eq!(profile.validate(), Ok(()));
    assert_eq!(
        EspPartitionTable {
            profiles: &AB_PROFILES,
            partitions: &FACTORY,
        }
        .validate(&profile),
        Err(EspPartitionTableError::MisalignedApplicationPartition { region: FIRMWARE })
    );
}

#[test]
fn boot_selection_partitions_are_two_sectors() {
    static ONE_SECTOR: [MemoryRegion; 3] = [
        SLOT_A,
        SLOT_B,
        fixture_boot_selection(AddressRange::new(0xE7_0000, 0xE7_1000)),
    ];

    assert_eq!(
        AB_TABLE.validate(&fixture_profile(&ONE_SECTOR)),
        Err(EspPartitionTableError::InvalidBootSelectionSize { region: OTADATA })
    );
}

#[test]
fn boot_selection_regions_need_a_partition_row() {
    const WITHOUT_OTADATA: [EspPartitionBinding; 2] = [AB_PARTITIONS[0], AB_PARTITIONS[1]];

    assert_eq!(
        EspPartitionTable {
            profiles: &AB_PROFILES,
            partitions: &WITHOUT_OTADATA,
        }
        .validate(&fixture_profile(&AB_REGIONS)),
        Err(EspPartitionTableError::MissingRegion { region: OTADATA })
    );
}
