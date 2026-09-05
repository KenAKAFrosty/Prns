mod espressif;
mod nrf52840;

use crate::{
    AddressRange, AddressSpaceId, Alignment, BackingStoreId, FirmwarePlacement, JournalLayout,
    MemoryProfile, MemoryProfileId, MemoryRegion, MemoryRegionId, RegionOwner, RegionRetention,
    RegionRole, TransportCompatibility, TransportEnvelope,
};

pub use espressif::{
    esp_partition_table, ESP_16_MIB_PARTITION_TABLE, ESP_4_MIB_PARTITION_TABLE,
    ESP_8_MIB_PARTITION_TABLE, HELTEC_E290, HELTEC_V4, HELTEC_V4_R8, T_BEAM_SUPREME, XIAO_ESP32_C6,
};
pub use nrf52840::{
    MESH_TOWER_V2, NRF52840_MEMORY_X_BINDING, T096, T1000_E, T114, T_ECHO_S140_V6, T_ECHO_S140_V7,
};

const KIB: u64 = 1024;
const MIB: u64 = 1024 * KIB;
const FLASH_PAGE_BYTES: u64 = 4096;

const FLASH: AddressSpaceId = AddressSpaceId("internal-flash");
const RAM: AddressSpaceId = AddressSpaceId("internal-ram");

const FLASH_BACKING: BackingStoreId = BackingStoreId("internal-flash");
const INTERNAL_RAM_BACKING: BackingStoreId = BackingStoreId("internal-sram");

const FIRMWARE: MemoryRegionId = MemoryRegionId("firmware");
const JOURNAL: MemoryRegionId = MemoryRegionId("journal");

const PAGE_ALIGNMENT: Alignment = match Alignment::try_new(FLASH_PAGE_BYTES) {
    Ok(value) => value,
    Err(_) => unreachable!(),
};

const fn region(
    id: &'static str,
    address_space: AddressSpaceId,
    start: u64,
    end: u64,
    owner: RegionOwner,
    retention: RegionRetention,
    role: RegionRole,
) -> MemoryRegion {
    MemoryRegion {
        id: MemoryRegionId(id),
        address_space,
        range: AddressRange::new(start, end),
        alignment: PAGE_ALIGNMENT,
        owner,
        retention,
        role,
    }
}

const fn firmware_placement(
    start: u64,
    firmware_end: u64,
    transport_end: u64,
) -> FirmwarePlacement {
    FirmwarePlacement {
        address_space: FLASH,
        firmware_owned_region: FIRMWARE,
        transport_envelope: TransportEnvelope {
            range: AddressRange::new(start, transport_end),
            compatibility: if firmware_end == transport_end {
                TransportCompatibility::ExactFirmwareRegion
            } else {
                TransportCompatibility::LegacyEnvelope
            },
        },
    }
}

const fn journal(
    start: u64,
    timebase_b: u64,
    arena_a_start: u64,
    arena_b_start: u64,
    end: u64,
) -> JournalLayout {
    JournalLayout {
        region: JOURNAL,
        page_bytes: FLASH_PAGE_BYTES,
        timebase_pages: [
            AddressRange::new(start, timebase_b),
            AddressRange::new(timebase_b, arena_a_start),
        ],
        arenas: [
            AddressRange::new(arena_a_start, arena_b_start),
            AddressRange::new(arena_b_start, end),
        ],
    }
}

pub const ALL_MEMORY_PROFILES: [&MemoryProfile; 11] = [
    &HELTEC_V4,
    &HELTEC_V4_R8,
    &HELTEC_E290,
    &T_BEAM_SUPREME,
    &XIAO_ESP32_C6,
    &T_ECHO_S140_V6,
    &T_ECHO_S140_V7,
    &T096,
    &T114,
    &T1000_E,
    &MESH_TOWER_V2,
];

#[must_use]
pub fn memory_profile(id: MemoryProfileId) -> Option<&'static MemoryProfile> {
    ALL_MEMORY_PROFILES
        .iter()
        .copied()
        .find(|profile| profile.id == id)
}

#[cfg(test)]
mod tests;
