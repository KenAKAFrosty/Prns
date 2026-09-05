use super::{AddressRange, AddressSpaceId, Alignment};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryRegionId(pub &'static str);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionOwner {
    Platform,
    FirmwareImage,
    DeviceIdentity,
    Provisioning,
    Radio,
    LearnedState,
    Factory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionRetention {
    ReplaceWithFirmware,
    PreserveAcrossFirmwareUpdate,
    Immutable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionRole {
    Bootloader,
    PartitionTable,
    SoftDevice,
    PlatformData,
    FirmwareImage,
    BleIdentity,
    Provisioning,
    NodeIdentity,
    PhyInitialization,
    RemoteControlIdentity,
    RadioProfile,
    Journal,
    RecoveryBootloader,
    FactoryReserved,
    Reserved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryRegion {
    pub id: MemoryRegionId,
    pub address_space: AddressSpaceId,
    pub range: AddressRange,
    pub alignment: Alignment,
    pub owner: RegionOwner,
    pub retention: RegionRetention,
    pub role: RegionRole,
}
