#![no_std]
#![forbid(unsafe_code)]

mod contract;
mod formats;
mod profiles;

pub use contract::{
    AddressRange, AddressRangeError, AddressSpace, AddressSpaceGeometry, AddressSpaceId,
    AddressSpaceKind, Alignment, AlignmentError, ArtifactError, BackingStoreId, FirmwarePlacement,
    JournalLayout, MemoryProfile, MemoryProfileId, MemoryRegion, MemoryRegionId,
    ProcessorArchitecture, RegionOwner, RegionRetention, RegionRole, RegionRoleLookupError,
    ReservationAccounting, ReservationCharge, ReservationId, ReservationPoolId, ReservationTotals,
    RuntimeReservation, TransportCompatibility, TransportEnvelope, ValidationError,
};
pub use formats::{
    EspPartitionBinding, EspPartitionKind, EspPartitionTable, EspPartitionTableError,
    NrfMemoryXBinding, NrfMemoryXError, NrfMemoryXLayout,
};
pub use profiles::{
    esp_partition_table, memory_profile, ALL_MEMORY_PROFILES, ESP_16_MIB_PARTITION_TABLE,
    ESP_4_MIB_PARTITION_TABLE, ESP_8_MIB_PARTITION_TABLE, HELTEC_E290, HELTEC_V4, HELTEC_V4_R8,
    MESH_TOWER_V2, NRF52840_MEMORY_X_BINDING, T096, T1000_E, T114, T_BEAM_SUPREME, T_ECHO_S140_V6,
    T_ECHO_S140_V7, XIAO_ESP32_C6,
};
