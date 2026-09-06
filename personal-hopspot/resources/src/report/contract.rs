use personal_hopspot_memory::{
    AddressSpaceGeometry, AddressSpaceKind, MemoryProfile, RegionOwner, RegionRetention,
    RegionRole, ReservationAccounting, ReservationCharge, TransportCompatibility,
};
use serde::Serialize;
use thiserror::Error;

use super::fingerprint::fingerprint;
use super::model::{
    AddressSpaceGeometryIdentity, AddressSpaceIdentity, MemoryContractIdentity,
    MemoryRegionIdentity, ReservationAccountingIdentity, RuntimeReservationIdentity,
    TransportEnvelopeIdentity,
};

#[derive(Debug, Error)]
pub(crate) enum ContractIdentityError {
    #[error("memory profile {profile:?} has no firmware-owned region {region:?}")]
    MissingFirmwareRegion {
        profile: &'static str,
        region: &'static str,
    },
    #[error("could not fingerprint memory profile: {0}")]
    Fingerprint(#[from] serde_json::Error),
}

pub(super) fn identity(
    profile: &MemoryProfile,
) -> Result<MemoryContractIdentity, ContractIdentityError> {
    let firmware_region = profile
        .region(profile.firmware.firmware_owned_region)
        .ok_or(ContractIdentityError::MissingFirmwareRegion {
            profile: profile.id.0,
            region: profile.firmware.firmware_owned_region.0,
        })?;
    let address_spaces = profile
        .address_spaces
        .iter()
        .map(|space| AddressSpaceIdentity {
            id: space.id.0.to_string(),
            kind: address_space_kind(space.kind).to_string(),
            geometry: geometry(space.geometry),
            backing_store: space.backing_store.0.to_string(),
            backing_offset: space.backing_offset,
        })
        .collect::<Vec<_>>();
    let regions = profile
        .regions
        .iter()
        .map(|region| MemoryRegionIdentity {
            id: region.id.0.to_string(),
            address_space: region.address_space.0.to_string(),
            start: region.range.start(),
            end: region.range.end(),
            alignment: region.alignment.bytes(),
            owner: region_owner(region.owner).to_string(),
            retention: region_retention(region.retention).to_string(),
            role: region_role(region.role).to_string(),
        })
        .collect::<Vec<_>>();
    let runtime_reservations = profile
        .runtime_reservations
        .iter()
        .map(|reservation| RuntimeReservationIdentity {
            id: reservation.id.0.to_string(),
            address_space: reservation.address_space.0.to_string(),
            bytes: reservation.bytes,
            accounting: reservation_accounting(reservation.accounting),
        })
        .collect::<Vec<_>>();
    let transport = profile.firmware.transport_envelope;
    let transport_envelope = TransportEnvelopeIdentity {
        address_space: profile.firmware.address_space.0.to_string(),
        start: transport.range.start(),
        end: transport.range.end(),
        compatibility: transport_compatibility(transport.compatibility).to_string(),
    };
    let fingerprint = fingerprint(&MemoryContractFingerprint {
        profile: profile.id.0,
        address_spaces: &address_spaces,
        regions: &regions,
        firmware_owned_region: firmware_region.id.0,
        transport_envelope: &transport_envelope,
        runtime_reservations: &runtime_reservations,
    })?;
    Ok(MemoryContractIdentity {
        fingerprint,
        address_spaces,
        regions,
        firmware_owned_region: firmware_region.id.0.to_string(),
        transport_envelope,
        runtime_reservations,
    })
}

#[derive(Serialize)]
struct MemoryContractFingerprint<'a> {
    profile: &'a str,
    address_spaces: &'a [AddressSpaceIdentity],
    regions: &'a [MemoryRegionIdentity],
    firmware_owned_region: &'a str,
    transport_envelope: &'a TransportEnvelopeIdentity,
    runtime_reservations: &'a [RuntimeReservationIdentity],
}

fn geometry(value: AddressSpaceGeometry) -> AddressSpaceGeometryIdentity {
    match value {
        AddressSpaceGeometry::Fixed(range) => AddressSpaceGeometryIdentity::Fixed {
            start: range.start(),
            end: range.end(),
        },
        AddressSpaceGeometry::FixedCapacity { bytes } => {
            AddressSpaceGeometryIdentity::FixedCapacity { bytes }
        }
        AddressSpaceGeometry::LinkerDefined => AddressSpaceGeometryIdentity::LinkerDefined,
        AddressSpaceGeometry::RuntimeDetected => AddressSpaceGeometryIdentity::RuntimeDetected,
    }
}

fn reservation_accounting(value: ReservationAccounting) -> ReservationAccountingIdentity {
    match value {
        ReservationAccounting::Dedicated { charge } => ReservationAccountingIdentity::Dedicated {
            charge: reservation_charge(charge).to_string(),
        },
        ReservationAccounting::SharedPool { pool, charge } => {
            ReservationAccountingIdentity::SharedPool {
                pool: pool.0.to_string(),
                charge: reservation_charge(charge).to_string(),
            }
        }
        ReservationAccounting::External => ReservationAccountingIdentity::External,
    }
}

const fn address_space_kind(value: AddressSpaceKind) -> &'static str {
    match value {
        AddressSpaceKind::InternalFlash => "internal-flash",
        AddressSpaceKind::InternalRam => "internal-ram",
        AddressSpaceKind::InstructionRam => "instruction-ram",
        AddressSpaceKind::DataRam => "data-ram",
        AddressSpaceKind::ReclaimedRam => "reclaimed-ram",
        AddressSpaceKind::DataCacheRam => "data-cache-ram",
        AddressSpaceKind::ExternalPsram => "external-psram",
        AddressSpaceKind::ExternalStorage => "external-storage",
    }
}

const fn region_owner(value: RegionOwner) -> &'static str {
    match value {
        RegionOwner::Platform => "platform",
        RegionOwner::FirmwareImage => "firmware-image",
        RegionOwner::DeviceIdentity => "device-identity",
        RegionOwner::Provisioning => "provisioning",
        RegionOwner::Radio => "radio",
        RegionOwner::LearnedState => "learned-state",
        RegionOwner::Factory => "factory",
    }
}

const fn region_retention(value: RegionRetention) -> &'static str {
    match value {
        RegionRetention::ReplaceWithFirmware => "replace-with-firmware",
        RegionRetention::PreserveAcrossFirmwareUpdate => "preserve-across-firmware-update",
        RegionRetention::Immutable => "immutable",
    }
}

const fn region_role(value: RegionRole) -> &'static str {
    match value {
        RegionRole::Bootloader => "bootloader",
        RegionRole::PartitionTable => "partition-table",
        RegionRole::SoftDevice => "soft-device",
        RegionRole::PlatformData => "platform-data",
        RegionRole::FirmwareImage => "firmware-image",
        RegionRole::BleIdentity => "ble-identity",
        RegionRole::Provisioning => "provisioning",
        RegionRole::NodeIdentity => "node-identity",
        RegionRole::PhyInitialization => "phy-initialization",
        RegionRole::RemoteControlIdentity => "remote-control-identity",
        RegionRole::RadioProfile => "radio-profile",
        RegionRole::Journal => "journal",
        RegionRole::RecoveryBootloader => "recovery-bootloader",
        RegionRole::FactoryReserved => "factory-reserved",
        RegionRole::Reserved => "reserved",
    }
}

const fn reservation_charge(value: ReservationCharge) -> &'static str {
    match value {
        ReservationCharge::AdditionalToStatic => "additional-to-static",
        ReservationCharge::IncludedInStaticImage => "included-in-static-image",
    }
}

const fn transport_compatibility(value: TransportCompatibility) -> &'static str {
    match value {
        TransportCompatibility::ExactFirmwareRegion => "exact-firmware-region",
        TransportCompatibility::LegacyEnvelope => "legacy-envelope",
    }
}
