use super::{AddressRange, AddressSpaceId, MemoryRegionId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportCompatibility {
    ExactFirmwareRegion,
    LegacyEnvelope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportEnvelope {
    pub range: AddressRange,
    pub compatibility: TransportCompatibility,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FirmwarePlacement {
    pub address_space: AddressSpaceId,
    pub firmware_owned_region: MemoryRegionId,
    pub transport_envelope: TransportEnvelope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactError {
    InvalidProfile,
    OutsideFirmwareOwnedRegion {
        image: AddressRange,
        firmware_owned: AddressRange,
    },
}
