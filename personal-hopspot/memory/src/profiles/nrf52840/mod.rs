use super::{
    firmware_placement, journal, region, FLASH, FLASH_BACKING, INTERNAL_RAM_BACKING, KIB, MIB, RAM,
};
use crate::{
    AddressRange, AddressSpace, AddressSpaceGeometry, AddressSpaceKind, JournalLayout,
    MemoryProfile, MemoryProfileId, MemoryRegion, ProcessorArchitecture, RegionOwner,
    RegionRetention, RegionRole, ReservationAccounting, ReservationCharge, ReservationId,
    RuntimeReservation,
};

const NRF52840_S140_RAM_SPACES: [AddressSpace; 2] = nrf52840_spaces(0x2000_C000);
const NRF52840_T1000E_RAM_SPACES: [AddressSpace; 2] = nrf52840_spaces(0x2001_0000);

const fn nrf52840_spaces(application_ram_start: u64) -> [AddressSpace; 2] {
    [
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
            geometry: AddressSpaceGeometry::Fixed(AddressRange::new(
                application_ram_start,
                0x2004_0000,
            )),
            backing_store: INTERNAL_RAM_BACKING,
            backing_offset: application_ram_start - 0x2000_0000,
        },
    ]
}

const NRF_RUNTIME_RESERVATIONS: [RuntimeReservation; 1] = [RuntimeReservation {
    id: ReservationId("minimum-runtime-stack"),
    address_space: RAM,
    bytes: 68 * KIB,
    accounting: ReservationAccounting::Dedicated {
        charge: ReservationCharge::AdditionalToStatic,
    },
}];

const T_ECHO_S140_V6_REGIONS: [MemoryRegion; 8] = [
    region(
        "platform-firmware",
        FLASH,
        0,
        0x26000,
        RegionOwner::Platform,
        RegionRetention::Immutable,
        RegionRole::SoftDevice,
    ),
    region(
        "firmware",
        FLASH,
        0x26000,
        0xBF000,
        RegionOwner::FirmwareImage,
        RegionRetention::ReplaceWithFirmware,
        RegionRole::FirmwareImage,
    ),
    region(
        "remote-control-identity",
        FLASH,
        0xBF000,
        0xC0000,
        RegionOwner::DeviceIdentity,
        RegionRetention::PreserveAcrossFirmwareUpdate,
        RegionRole::RemoteControlIdentity,
    ),
    region(
        "journal",
        FLASH,
        0xC0000,
        0xE9000,
        RegionOwner::LearnedState,
        RegionRetention::PreserveAcrossFirmwareUpdate,
        RegionRole::Journal,
    ),
    region(
        "radio-profile",
        FLASH,
        0xE9000,
        0xEB000,
        RegionOwner::Radio,
        RegionRetention::PreserveAcrossFirmwareUpdate,
        RegionRole::RadioProfile,
    ),
    region(
        "node-identity",
        FLASH,
        0xEB000,
        0xEC000,
        RegionOwner::DeviceIdentity,
        RegionRetention::PreserveAcrossFirmwareUpdate,
        RegionRole::NodeIdentity,
    ),
    region(
        "ble-identity",
        FLASH,
        0xEC000,
        0xED000,
        RegionOwner::DeviceIdentity,
        RegionRetention::PreserveAcrossFirmwareUpdate,
        RegionRole::BleIdentity,
    ),
    region(
        "reserved",
        FLASH,
        0xED000,
        0x100000,
        RegionOwner::Factory,
        RegionRetention::Immutable,
        RegionRole::Reserved,
    ),
];

const T_ECHO_S140_V7_REGIONS: [MemoryRegion; 8] = [
    region(
        "platform-firmware",
        FLASH,
        0,
        0x27000,
        RegionOwner::Platform,
        RegionRetention::Immutable,
        RegionRole::SoftDevice,
    ),
    region(
        "firmware",
        FLASH,
        0x27000,
        0xBF000,
        RegionOwner::FirmwareImage,
        RegionRetention::ReplaceWithFirmware,
        RegionRole::FirmwareImage,
    ),
    T_ECHO_S140_V6_REGIONS[2],
    T_ECHO_S140_V6_REGIONS[3],
    T_ECHO_S140_V6_REGIONS[4],
    T_ECHO_S140_V6_REGIONS[5],
    T_ECHO_S140_V6_REGIONS[6],
    T_ECHO_S140_V6_REGIONS[7],
];

const T_ECHO_JOURNALS: [JournalLayout; 1] = [journal(0xC0000, 0xC1000, 0xC2000, 0xD6000, 0xE9000)];

const T114_REGIONS: [MemoryRegion; 8] = [
    region(
        "platform-firmware",
        FLASH,
        0,
        0x26000,
        RegionOwner::Platform,
        RegionRetention::Immutable,
        RegionRole::SoftDevice,
    ),
    region(
        "firmware",
        FLASH,
        0x26000,
        0xE1000,
        RegionOwner::FirmwareImage,
        RegionRetention::ReplaceWithFirmware,
        RegionRole::FirmwareImage,
    ),
    region(
        "remote-control-identity",
        FLASH,
        0xE1000,
        0xE2000,
        RegionOwner::DeviceIdentity,
        RegionRetention::PreserveAcrossFirmwareUpdate,
        RegionRole::RemoteControlIdentity,
    ),
    region(
        "journal",
        FLASH,
        0xE2000,
        0xE8000,
        RegionOwner::LearnedState,
        RegionRetention::PreserveAcrossFirmwareUpdate,
        RegionRole::Journal,
    ),
    region(
        "ble-identity",
        FLASH,
        0xE8000,
        0xE9000,
        RegionOwner::DeviceIdentity,
        RegionRetention::PreserveAcrossFirmwareUpdate,
        RegionRole::BleIdentity,
    ),
    region(
        "radio-profile",
        FLASH,
        0xE9000,
        0xEB000,
        RegionOwner::Radio,
        RegionRetention::PreserveAcrossFirmwareUpdate,
        RegionRole::RadioProfile,
    ),
    region(
        "node-identity",
        FLASH,
        0xEB000,
        0xEC000,
        RegionOwner::DeviceIdentity,
        RegionRetention::PreserveAcrossFirmwareUpdate,
        RegionRole::NodeIdentity,
    ),
    region(
        "recovery-bootloader",
        FLASH,
        0xEC000,
        0x100000,
        RegionOwner::Platform,
        RegionRetention::Immutable,
        RegionRole::RecoveryBootloader,
    ),
];

const T096_REGIONS: [MemoryRegion; 10] = [
    T114_REGIONS[0],
    T114_REGIONS[1],
    T114_REGIONS[2],
    T114_REGIONS[3],
    T114_REGIONS[4],
    T114_REGIONS[5],
    T114_REGIONS[6],
    region(
        "application-data-reserved",
        FLASH,
        0xEC000,
        0xED000,
        RegionOwner::Factory,
        RegionRetention::PreserveAcrossFirmwareUpdate,
        RegionRole::Reserved,
    ),
    region(
        "factory-reserved",
        FLASH,
        0xED000,
        0xF4000,
        RegionOwner::Factory,
        RegionRetention::Immutable,
        RegionRole::FactoryReserved,
    ),
    region(
        "recovery-bootloader",
        FLASH,
        0xF4000,
        0x100000,
        RegionOwner::Platform,
        RegionRetention::Immutable,
        RegionRole::RecoveryBootloader,
    ),
];

const HELTEC_DISPLAY_JOURNALS: [JournalLayout; 1] =
    [journal(0xE2000, 0xE3000, 0xE4000, 0xE6000, 0xE8000)];

const MESH_TOWER_V2_REGIONS: [MemoryRegion; 8] = [
    region(
        "platform-firmware",
        FLASH,
        0,
        0x26000,
        RegionOwner::Platform,
        RegionRetention::Immutable,
        RegionRole::SoftDevice,
    ),
    region(
        "firmware",
        FLASH,
        0x26000,
        0xE2000,
        RegionOwner::FirmwareImage,
        RegionRetention::ReplaceWithFirmware,
        RegionRole::FirmwareImage,
    ),
    region(
        "remote-control-identity",
        FLASH,
        0xE2000,
        0xE3000,
        RegionOwner::DeviceIdentity,
        RegionRetention::PreserveAcrossFirmwareUpdate,
        RegionRole::RemoteControlIdentity,
    ),
    region(
        "journal",
        FLASH,
        0xE3000,
        0xE9000,
        RegionOwner::LearnedState,
        RegionRetention::PreserveAcrossFirmwareUpdate,
        RegionRole::Journal,
    ),
    region(
        "radio-profile",
        FLASH,
        0xE9000,
        0xEA000,
        RegionOwner::Radio,
        RegionRetention::PreserveAcrossFirmwareUpdate,
        RegionRole::RadioProfile,
    ),
    region(
        "ble-identity",
        FLASH,
        0xEA000,
        0xEB000,
        RegionOwner::DeviceIdentity,
        RegionRetention::PreserveAcrossFirmwareUpdate,
        RegionRole::BleIdentity,
    ),
    region(
        "node-identity",
        FLASH,
        0xEB000,
        0xEC000,
        RegionOwner::DeviceIdentity,
        RegionRetention::PreserveAcrossFirmwareUpdate,
        RegionRole::NodeIdentity,
    ),
    region(
        "recovery-bootloader",
        FLASH,
        0xEC000,
        0x100000,
        RegionOwner::Platform,
        RegionRetention::Immutable,
        RegionRole::RecoveryBootloader,
    ),
];

const MESH_TOWER_JOURNALS: [JournalLayout; 1] =
    [journal(0xE3000, 0xE4000, 0xE5000, 0xE7000, 0xE9000)];

const T1000E_REGIONS: [MemoryRegion; 7] = [
    region(
        "platform-firmware",
        FLASH,
        0,
        0x27000,
        RegionOwner::Platform,
        RegionRetention::Immutable,
        RegionRole::SoftDevice,
    ),
    region(
        "firmware",
        FLASH,
        0x27000,
        0xE9000,
        RegionOwner::FirmwareImage,
        RegionRetention::ReplaceWithFirmware,
        RegionRole::FirmwareImage,
    ),
    region(
        "remote-control-identity",
        FLASH,
        0xE9000,
        0xEA000,
        RegionOwner::DeviceIdentity,
        RegionRetention::PreserveAcrossFirmwareUpdate,
        RegionRole::RemoteControlIdentity,
    ),
    region(
        "journal",
        FLASH,
        0xEA000,
        0xF0000,
        RegionOwner::LearnedState,
        RegionRetention::PreserveAcrossFirmwareUpdate,
        RegionRole::Journal,
    ),
    region(
        "node-identity",
        FLASH,
        0xF0000,
        0xF1000,
        RegionOwner::DeviceIdentity,
        RegionRetention::PreserveAcrossFirmwareUpdate,
        RegionRole::NodeIdentity,
    ),
    region(
        "reserved",
        FLASH,
        0xF1000,
        0xF4000,
        RegionOwner::Factory,
        RegionRetention::Immutable,
        RegionRole::Reserved,
    ),
    region(
        "recovery-bootloader",
        FLASH,
        0xF4000,
        0x100000,
        RegionOwner::Platform,
        RegionRetention::Immutable,
        RegionRole::RecoveryBootloader,
    ),
];

const T1000E_JOURNALS: [JournalLayout; 1] = [journal(0xEA000, 0xEB000, 0xEC000, 0xEE000, 0xF0000)];

pub const T_ECHO_S140_V6: MemoryProfile = MemoryProfile {
    id: MemoryProfileId("t-echo-s140-v6"),
    architecture: ProcessorArchitecture::ThumbV7em,
    address_spaces: &NRF52840_S140_RAM_SPACES,
    regions: &T_ECHO_S140_V6_REGIONS,
    firmware: firmware_placement(0x26000, 0xBF000, 0xC0000),
    journals: &T_ECHO_JOURNALS,
    runtime_reservations: &NRF_RUNTIME_RESERVATIONS,
};

pub const T_ECHO_S140_V7: MemoryProfile = MemoryProfile {
    id: MemoryProfileId("t-echo-s140-v7"),
    architecture: ProcessorArchitecture::ThumbV7em,
    address_spaces: &NRF52840_S140_RAM_SPACES,
    regions: &T_ECHO_S140_V7_REGIONS,
    firmware: firmware_placement(0x27000, 0xBF000, 0xC0000),
    journals: &T_ECHO_JOURNALS,
    runtime_reservations: &NRF_RUNTIME_RESERVATIONS,
};

pub const T096: MemoryProfile = MemoryProfile {
    id: MemoryProfileId("t096"),
    architecture: ProcessorArchitecture::ThumbV7em,
    address_spaces: &NRF52840_S140_RAM_SPACES,
    regions: &T096_REGIONS,
    firmware: firmware_placement(0x26000, 0xE1000, 0xE8000),
    journals: &HELTEC_DISPLAY_JOURNALS,
    runtime_reservations: &NRF_RUNTIME_RESERVATIONS,
};

pub const T114: MemoryProfile = MemoryProfile {
    id: MemoryProfileId("t114"),
    architecture: ProcessorArchitecture::ThumbV7em,
    address_spaces: &NRF52840_S140_RAM_SPACES,
    regions: &T114_REGIONS,
    firmware: firmware_placement(0x26000, 0xE1000, 0xE9000),
    journals: &HELTEC_DISPLAY_JOURNALS,
    runtime_reservations: &NRF_RUNTIME_RESERVATIONS,
};

pub const T1000_E: MemoryProfile = MemoryProfile {
    id: MemoryProfileId("t1000-e"),
    architecture: ProcessorArchitecture::ThumbV7em,
    address_spaces: &NRF52840_T1000E_RAM_SPACES,
    regions: &T1000E_REGIONS,
    firmware: firmware_placement(0x27000, 0xE9000, 0xEA000),
    journals: &T1000E_JOURNALS,
    runtime_reservations: &NRF_RUNTIME_RESERVATIONS,
};

pub const MESH_TOWER_V2: MemoryProfile = MemoryProfile {
    id: MemoryProfileId("mesh-tower-v2"),
    architecture: ProcessorArchitecture::ThumbV7em,
    address_spaces: &NRF52840_S140_RAM_SPACES,
    regions: &MESH_TOWER_V2_REGIONS,
    firmware: firmware_placement(0x26000, 0xE2000, 0xE2000),
    journals: &MESH_TOWER_JOURNALS,
    runtime_reservations: &NRF_RUNTIME_RESERVATIONS,
};

#[cfg(test)]
mod tests;
