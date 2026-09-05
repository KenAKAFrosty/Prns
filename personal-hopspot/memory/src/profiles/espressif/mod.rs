use super::{
    firmware_placement, journal, region, FLASH, FLASH_BACKING, INTERNAL_RAM_BACKING, KIB, MIB,
};
use crate::{
    AddressRange, AddressSpace, AddressSpaceGeometry, AddressSpaceId, AddressSpaceKind,
    BackingStoreId, EspPartitionBinding, EspPartitionKind, EspPartitionTable, JournalLayout,
    MemoryProfile, MemoryProfileId, MemoryRegion, MemoryRegionId, ProcessorArchitecture,
    RegionOwner, RegionRetention, RegionRole, ReservationAccounting, ReservationCharge,
    ReservationId, ReservationPoolId, RuntimeReservation,
};

const IRAM: AddressSpaceId = AddressSpaceId("instruction-ram");
const DRAM: AddressSpaceId = AddressSpaceId("data-ram");
const RECLAIMED_RAM: AddressSpaceId = AddressSpaceId("reclaimed-ram");
const DCACHE_RAM: AddressSpaceId = AddressSpaceId("dcache-ram");
const PSRAM: AddressSpaceId = AddressSpaceId("external-psram");

const RECLAIMED_RAM_BACKING: BackingStoreId = BackingStoreId("reclaimed-sram");
const DCACHE_RAM_BACKING: BackingStoreId = BackingStoreId("dcache-sram");
const PSRAM_BACKING: BackingStoreId = BackingStoreId("external-psram");

const fn esp_binding(
    region: &'static str,
    name: &'static str,
    kind: EspPartitionKind,
) -> EspPartitionBinding {
    EspPartitionBinding {
        region: MemoryRegionId(region),
        name,
        kind,
    }
}

const ESP32S3_16_MIB_RUNTIME_PSRAM_SPACES: [AddressSpace; 6] =
    esp32s3_spaces(16 * MIB, AddressSpaceGeometry::RuntimeDetected);
const ESP32S3_16_MIB_FIXED_PSRAM_SPACES: [AddressSpace; 6] = esp32s3_spaces(
    16 * MIB,
    AddressSpaceGeometry::FixedCapacity { bytes: 8 * MIB },
);
const ESP32S3_8_MIB_RUNTIME_PSRAM_SPACES: [AddressSpace; 6] =
    esp32s3_spaces(8 * MIB, AddressSpaceGeometry::RuntimeDetected);

const fn esp32s3_spaces(
    flash_bytes: u64,
    psram_geometry: AddressSpaceGeometry,
) -> [AddressSpace; 6] {
    [
        AddressSpace {
            id: FLASH,
            kind: AddressSpaceKind::InternalFlash,
            geometry: AddressSpaceGeometry::Fixed(AddressRange::new(0, flash_bytes)),
            backing_store: FLASH_BACKING,
            backing_offset: 0,
        },
        AddressSpace {
            id: IRAM,
            kind: AddressSpaceKind::InstructionRam,
            geometry: AddressSpaceGeometry::LinkerDefined,
            backing_store: INTERNAL_RAM_BACKING,
            backing_offset: 0,
        },
        AddressSpace {
            id: DRAM,
            kind: AddressSpaceKind::DataRam,
            geometry: AddressSpaceGeometry::LinkerDefined,
            backing_store: INTERNAL_RAM_BACKING,
            backing_offset: 0,
        },
        AddressSpace {
            id: RECLAIMED_RAM,
            kind: AddressSpaceKind::ReclaimedRam,
            geometry: AddressSpaceGeometry::FixedCapacity { bytes: 72 * KIB },
            backing_store: RECLAIMED_RAM_BACKING,
            backing_offset: 0,
        },
        AddressSpace {
            id: DCACHE_RAM,
            kind: AddressSpaceKind::DataCacheRam,
            geometry: AddressSpaceGeometry::Fixed(AddressRange::new(0x3FCF_0000, 0x3FCF_8000)),
            backing_store: DCACHE_RAM_BACKING,
            backing_offset: 0,
        },
        AddressSpace {
            id: PSRAM,
            kind: AddressSpaceKind::ExternalPsram,
            geometry: psram_geometry,
            backing_store: PSRAM_BACKING,
            backing_offset: 0,
        },
    ]
}

const ESP32C6_SPACES: [AddressSpace; 3] = [
    AddressSpace {
        id: FLASH,
        kind: AddressSpaceKind::InternalFlash,
        geometry: AddressSpaceGeometry::Fixed(AddressRange::new(0, 4 * MIB)),
        backing_store: FLASH_BACKING,
        backing_offset: 0,
    },
    AddressSpace {
        id: IRAM,
        kind: AddressSpaceKind::InstructionRam,
        geometry: AddressSpaceGeometry::LinkerDefined,
        backing_store: INTERNAL_RAM_BACKING,
        backing_offset: 0,
    },
    AddressSpace {
        id: DRAM,
        kind: AddressSpaceKind::DataRam,
        geometry: AddressSpaceGeometry::LinkerDefined,
        backing_store: INTERNAL_RAM_BACKING,
        backing_offset: 0,
    },
];

const ESP_COMMON_PREFIX: [MemoryRegion; 7] = [
    region(
        "bootloader",
        FLASH,
        0,
        0x8000,
        RegionOwner::Platform,
        RegionRetention::Immutable,
        RegionRole::Bootloader,
    ),
    region(
        "partition-table",
        FLASH,
        0x8000,
        0x9000,
        RegionOwner::Platform,
        RegionRetention::Immutable,
        RegionRole::PartitionTable,
    ),
    region(
        "nvs",
        FLASH,
        0x9000,
        0xC000,
        RegionOwner::Platform,
        RegionRetention::PreserveAcrossFirmwareUpdate,
        RegionRole::PlatformData,
    ),
    region(
        "ble_id",
        FLASH,
        0xC000,
        0xD000,
        RegionOwner::DeviceIdentity,
        RegionRetention::PreserveAcrossFirmwareUpdate,
        RegionRole::BleIdentity,
    ),
    region(
        "hopcfg",
        FLASH,
        0xD000,
        0xE000,
        RegionOwner::Provisioning,
        RegionRetention::PreserveAcrossFirmwareUpdate,
        RegionRole::Provisioning,
    ),
    region(
        "node_id",
        FLASH,
        0xE000,
        0xF000,
        RegionOwner::DeviceIdentity,
        RegionRetention::PreserveAcrossFirmwareUpdate,
        RegionRole::NodeIdentity,
    ),
    region(
        "phy_init",
        FLASH,
        0xF000,
        0x10000,
        RegionOwner::Radio,
        RegionRetention::PreserveAcrossFirmwareUpdate,
        RegionRole::PhyInitialization,
    ),
];

const ESP_16_MIB_REGIONS: [MemoryRegion; 11] = [
    ESP_COMMON_PREFIX[0],
    ESP_COMMON_PREFIX[1],
    ESP_COMMON_PREFIX[2],
    ESP_COMMON_PREFIX[3],
    ESP_COMMON_PREFIX[4],
    ESP_COMMON_PREFIX[5],
    ESP_COMMON_PREFIX[6],
    region(
        "firmware",
        FLASH,
        0x10000,
        0xE7D000,
        RegionOwner::FirmwareImage,
        RegionRetention::ReplaceWithFirmware,
        RegionRole::FirmwareImage,
    ),
    region(
        "remote_ctl_id",
        FLASH,
        0xE7D000,
        0xE7E000,
        RegionOwner::DeviceIdentity,
        RegionRetention::PreserveAcrossFirmwareUpdate,
        RegionRole::RemoteControlIdentity,
    ),
    region(
        "radio_cfg",
        FLASH,
        0xE7E000,
        0xE80000,
        RegionOwner::Radio,
        RegionRetention::PreserveAcrossFirmwareUpdate,
        RegionRole::RadioProfile,
    ),
    region(
        "journal",
        FLASH,
        0xE80000,
        0x1000000,
        RegionOwner::LearnedState,
        RegionRetention::PreserveAcrossFirmwareUpdate,
        RegionRole::Journal,
    ),
];

const ESP_8_MIB_REGIONS: [MemoryRegion; 11] = [
    ESP_COMMON_PREFIX[0],
    ESP_COMMON_PREFIX[1],
    ESP_COMMON_PREFIX[2],
    ESP_COMMON_PREFIX[3],
    ESP_COMMON_PREFIX[4],
    ESP_COMMON_PREFIX[5],
    ESP_COMMON_PREFIX[6],
    region(
        "firmware",
        FLASH,
        0x10000,
        0x67D000,
        RegionOwner::FirmwareImage,
        RegionRetention::ReplaceWithFirmware,
        RegionRole::FirmwareImage,
    ),
    region(
        "remote_ctl_id",
        FLASH,
        0x67D000,
        0x67E000,
        RegionOwner::DeviceIdentity,
        RegionRetention::PreserveAcrossFirmwareUpdate,
        RegionRole::RemoteControlIdentity,
    ),
    region(
        "radio_cfg",
        FLASH,
        0x67E000,
        0x680000,
        RegionOwner::Radio,
        RegionRetention::PreserveAcrossFirmwareUpdate,
        RegionRole::RadioProfile,
    ),
    region(
        "journal",
        FLASH,
        0x680000,
        0x800000,
        RegionOwner::LearnedState,
        RegionRetention::PreserveAcrossFirmwareUpdate,
        RegionRole::Journal,
    ),
];

const ESP_4_MIB_REGIONS: [MemoryRegion; 10] = [
    ESP_COMMON_PREFIX[0],
    ESP_COMMON_PREFIX[1],
    ESP_COMMON_PREFIX[2],
    ESP_COMMON_PREFIX[3],
    ESP_COMMON_PREFIX[4],
    ESP_COMMON_PREFIX[5],
    ESP_COMMON_PREFIX[6],
    region(
        "firmware",
        FLASH,
        0x10000,
        0x3DF000,
        RegionOwner::FirmwareImage,
        RegionRetention::ReplaceWithFirmware,
        RegionRole::FirmwareImage,
    ),
    region(
        "remote_ctl_id",
        FLASH,
        0x3DF000,
        0x3E0000,
        RegionOwner::DeviceIdentity,
        RegionRetention::PreserveAcrossFirmwareUpdate,
        RegionRole::RemoteControlIdentity,
    ),
    region(
        "journal",
        FLASH,
        0x3E0000,
        0x400000,
        RegionOwner::LearnedState,
        RegionRetention::PreserveAcrossFirmwareUpdate,
        RegionRole::Journal,
    ),
];

const ESP_16_MIB_JOURNALS: [JournalLayout; 1] =
    [journal(0xE80000, 0xE81000, 0xE82000, 0xF41000, 0x1000000)];
const ESP_8_MIB_JOURNALS: [JournalLayout; 1] =
    [journal(0x680000, 0x681000, 0x682000, 0x741000, 0x800000)];
const ESP_4_MIB_JOURNALS: [JournalLayout; 1] =
    [journal(0x3E0000, 0x3E1000, 0x3E2000, 0x3F1000, 0x400000)];

const S3_RUNTIME_RESERVATIONS: [RuntimeReservation; 4] = [
    RuntimeReservation {
        id: ReservationId("core1-stack"),
        address_space: DRAM,
        bytes: 72 * KIB,
        accounting: ReservationAccounting::Dedicated {
            charge: ReservationCharge::IncludedInStaticImage,
        },
    },
    RuntimeReservation {
        id: ReservationId("reclaimed-heap"),
        address_space: RECLAIMED_RAM,
        bytes: 72 * KIB,
        accounting: ReservationAccounting::SharedPool {
            pool: ReservationPoolId("reclaimed-heap"),
            charge: ReservationCharge::IncludedInStaticImage,
        },
    },
    RuntimeReservation {
        id: ReservationId("radio-internal-heap"),
        address_space: DRAM,
        bytes: 52 * KIB,
        accounting: ReservationAccounting::SharedPool {
            pool: ReservationPoolId("radio-internal-heap"),
            charge: ReservationCharge::IncludedInStaticImage,
        },
    },
    RuntimeReservation {
        id: ReservationId("dcache-heap"),
        address_space: DCACHE_RAM,
        bytes: 32 * KIB,
        accounting: ReservationAccounting::SharedPool {
            pool: ReservationPoolId("dcache-heap"),
            charge: ReservationCharge::AdditionalToStatic,
        },
    },
];

const C6_RUNTIME_RESERVATIONS: [RuntimeReservation; 1] = [RuntimeReservation {
    id: ReservationId("radio-runtime-heap"),
    address_space: DRAM,
    bytes: 88 * KIB,
    accounting: ReservationAccounting::SharedPool {
        pool: ReservationPoolId("radio-runtime-heap"),
        charge: ReservationCharge::AdditionalToStatic,
    },
}];

pub const HELTEC_V4: MemoryProfile = MemoryProfile {
    id: MemoryProfileId("heltec-v4"),
    architecture: ProcessorArchitecture::XtensaEsp32S3,
    address_spaces: &ESP32S3_16_MIB_RUNTIME_PSRAM_SPACES,
    regions: &ESP_16_MIB_REGIONS,
    firmware: firmware_placement(0x10000, 0xE7D000, 0xE7D000),
    journals: &ESP_16_MIB_JOURNALS,
    runtime_reservations: &S3_RUNTIME_RESERVATIONS,
};

pub const HELTEC_V4_R8: MemoryProfile = MemoryProfile {
    id: MemoryProfileId("heltec-v4-r8"),
    architecture: ProcessorArchitecture::XtensaEsp32S3,
    address_spaces: &ESP32S3_16_MIB_FIXED_PSRAM_SPACES,
    regions: &ESP_16_MIB_REGIONS,
    firmware: firmware_placement(0x10000, 0xE7D000, 0xE7D000),
    journals: &ESP_16_MIB_JOURNALS,
    runtime_reservations: &S3_RUNTIME_RESERVATIONS,
};

pub const HELTEC_E290: MemoryProfile = MemoryProfile {
    id: MemoryProfileId("heltec-e290"),
    architecture: ProcessorArchitecture::XtensaEsp32S3,
    address_spaces: &ESP32S3_16_MIB_FIXED_PSRAM_SPACES,
    regions: &ESP_16_MIB_REGIONS,
    firmware: firmware_placement(0x10000, 0xE7D000, 0xE7D000),
    journals: &ESP_16_MIB_JOURNALS,
    runtime_reservations: &S3_RUNTIME_RESERVATIONS,
};

pub const T_BEAM_SUPREME: MemoryProfile = MemoryProfile {
    id: MemoryProfileId("t-beam-supreme"),
    architecture: ProcessorArchitecture::XtensaEsp32S3,
    address_spaces: &ESP32S3_8_MIB_RUNTIME_PSRAM_SPACES,
    regions: &ESP_8_MIB_REGIONS,
    firmware: firmware_placement(0x10000, 0x67D000, 0x67D000),
    journals: &ESP_8_MIB_JOURNALS,
    runtime_reservations: &S3_RUNTIME_RESERVATIONS,
};

pub const XIAO_ESP32_C6: MemoryProfile = MemoryProfile {
    id: MemoryProfileId("xiao-esp32-c6"),
    architecture: ProcessorArchitecture::RiscV32Imac,
    address_spaces: &ESP32C6_SPACES,
    regions: &ESP_4_MIB_REGIONS,
    firmware: firmware_placement(0x10000, 0x3DF000, 0x3DF000),
    journals: &ESP_4_MIB_JOURNALS,
    runtime_reservations: &C6_RUNTIME_RESERVATIONS,
};

const ESP_COMMON_PARTITIONS: [EspPartitionBinding; 5] = [
    esp_binding("nvs", "nvs", EspPartitionKind::NvsData),
    esp_binding(
        "ble_id",
        "ble_id",
        EspPartitionKind::Custom {
            partition_type: 0x40,
            subtype: 0x00,
        },
    ),
    esp_binding(
        "hopcfg",
        "hopcfg",
        EspPartitionKind::Custom {
            partition_type: 0x41,
            subtype: 0x00,
        },
    ),
    esp_binding(
        "node_id",
        "node_id",
        EspPartitionKind::Custom {
            partition_type: 0x42,
            subtype: 0x00,
        },
    ),
    esp_binding("phy_init", "phy_init", EspPartitionKind::PhyData),
];

const ESP_16_MIB_PARTITIONS: [EspPartitionBinding; 9] = [
    ESP_COMMON_PARTITIONS[0],
    ESP_COMMON_PARTITIONS[1],
    ESP_COMMON_PARTITIONS[2],
    ESP_COMMON_PARTITIONS[3],
    ESP_COMMON_PARTITIONS[4],
    esp_binding("firmware", "factory", EspPartitionKind::FactoryApplication),
    esp_binding(
        "remote_ctl_id",
        "remote_ctl_id",
        EspPartitionKind::Custom {
            partition_type: 0x45,
            subtype: 0x00,
        },
    ),
    esp_binding(
        "radio_cfg",
        "radio_cfg",
        EspPartitionKind::Custom {
            partition_type: 0x44,
            subtype: 0x00,
        },
    ),
    esp_binding(
        "journal",
        "prns_state",
        EspPartitionKind::Custom {
            partition_type: 0x43,
            subtype: 0x00,
        },
    ),
];

const ESP_8_MIB_PARTITIONS: [EspPartitionBinding; 9] = ESP_16_MIB_PARTITIONS;

const ESP_4_MIB_PARTITIONS: [EspPartitionBinding; 8] = [
    ESP_COMMON_PARTITIONS[0],
    ESP_COMMON_PARTITIONS[1],
    ESP_COMMON_PARTITIONS[2],
    ESP_COMMON_PARTITIONS[3],
    ESP_COMMON_PARTITIONS[4],
    ESP_16_MIB_PARTITIONS[5],
    ESP_16_MIB_PARTITIONS[6],
    ESP_16_MIB_PARTITIONS[8],
];

const ESP_16_MIB_PROFILES: [MemoryProfileId; 3] = [HELTEC_V4.id, HELTEC_V4_R8.id, HELTEC_E290.id];
const ESP_8_MIB_PROFILES: [MemoryProfileId; 1] = [T_BEAM_SUPREME.id];
const ESP_4_MIB_PROFILES: [MemoryProfileId; 1] = [XIAO_ESP32_C6.id];

pub const ESP_16_MIB_PARTITION_TABLE: EspPartitionTable = EspPartitionTable {
    profiles: &ESP_16_MIB_PROFILES,
    partitions: &ESP_16_MIB_PARTITIONS,
};

pub const ESP_8_MIB_PARTITION_TABLE: EspPartitionTable = EspPartitionTable {
    profiles: &ESP_8_MIB_PROFILES,
    partitions: &ESP_8_MIB_PARTITIONS,
};

pub const ESP_4_MIB_PARTITION_TABLE: EspPartitionTable = EspPartitionTable {
    profiles: &ESP_4_MIB_PROFILES,
    partitions: &ESP_4_MIB_PARTITIONS,
};

#[must_use]
pub fn esp_partition_table(id: MemoryProfileId) -> Option<&'static EspPartitionTable> {
    [
        &ESP_16_MIB_PARTITION_TABLE,
        &ESP_8_MIB_PARTITION_TABLE,
        &ESP_4_MIB_PARTITION_TABLE,
    ]
    .into_iter()
    .find(|table| table.supports(id))
}

#[cfg(test)]
mod tests;
