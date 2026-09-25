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
    WifiConfiguration,
    Journal,
    RecoveryBootloader,
    FactoryReserved,
    Reserved,
    BootSelection,
    FirmwareUpdateSlot,
}

impl RegionRole {
    pub(super) const fn same_as(self, other: Self) -> bool {
        match self {
            Self::Bootloader => matches!(other, Self::Bootloader),
            Self::PartitionTable => matches!(other, Self::PartitionTable),
            Self::SoftDevice => matches!(other, Self::SoftDevice),
            Self::PlatformData => matches!(other, Self::PlatformData),
            Self::FirmwareImage => matches!(other, Self::FirmwareImage),
            Self::BleIdentity => matches!(other, Self::BleIdentity),
            Self::Provisioning => matches!(other, Self::Provisioning),
            Self::NodeIdentity => matches!(other, Self::NodeIdentity),
            Self::PhyInitialization => matches!(other, Self::PhyInitialization),
            Self::RemoteControlIdentity => matches!(other, Self::RemoteControlIdentity),
            Self::RadioProfile => matches!(other, Self::RadioProfile),
            Self::WifiConfiguration => matches!(other, Self::WifiConfiguration),
            Self::Journal => matches!(other, Self::Journal),
            Self::RecoveryBootloader => matches!(other, Self::RecoveryBootloader),
            Self::FactoryReserved => matches!(other, Self::FactoryReserved),
            Self::Reserved => matches!(other, Self::Reserved),
            Self::BootSelection => matches!(other, Self::BootSelection),
            Self::FirmwareUpdateSlot => matches!(other, Self::FirmwareUpdateSlot),
        }
    }
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
