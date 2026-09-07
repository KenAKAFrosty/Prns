use serde::{Deserialize, Serialize};

use super::fingerprint::Fingerprint;

pub(super) const SCHEMA_VERSION: u32 = 3;

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ResourceReport {
    pub schema_version: u32,
    pub target: TargetIdentity,
    pub architecture: ArchitectureIdentity,
    pub build: BuildIdentity,
    pub toolchain: ToolchainIdentity,
    pub memory_contract: MemoryContractIdentity,
    pub status: BuildStatus,
    pub firmware_flash: Evidence<FirmwareFlashUsage>,
    pub static_ram: Evidence<Vec<RamBackingUsage>>,
    pub artifacts: Evidence<Vec<ArtifactIdentity>>,
    pub analysis: AnalysisEvidence,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TargetIdentity {
    pub id: String,
    pub display_name: String,
    pub memory_profile: String,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ArchitectureIdentity {
    pub rust_target: String,
    pub adapter: String,
    pub linker_flavor: String,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BuildIdentity {
    pub fingerprint: Fingerprint,
    pub firmware_version: String,
    pub cargo_profile: String,
    pub recipe_kind: String,
    pub package: String,
    pub binary: String,
    pub features: Vec<String>,
    pub lto: String,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ToolchainIdentity {
    pub fingerprint: Fingerprint,
    pub rustc_version: String,
    pub cargo_version: String,
    pub linker_program: String,
    pub linker_version: String,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MemoryContractIdentity {
    pub fingerprint: Fingerprint,
    pub address_spaces: Vec<AddressSpaceIdentity>,
    pub regions: Vec<MemoryRegionIdentity>,
    pub firmware_owned_region: String,
    pub transport_envelope: TransportEnvelopeIdentity,
    pub runtime_reservations: Vec<RuntimeReservationIdentity>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AddressSpaceIdentity {
    pub id: String,
    pub kind: String,
    pub geometry: AddressSpaceGeometryIdentity,
    pub backing_store: String,
    pub backing_offset: u64,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(super) enum AddressSpaceGeometryIdentity {
    Fixed { start: u64, end: u64 },
    FixedCapacity { bytes: u64 },
    LinkerDefined,
    RuntimeDetected,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MemoryRegionIdentity {
    pub id: String,
    pub address_space: String,
    pub start: u64,
    pub end: u64,
    pub alignment: u64,
    pub owner: String,
    pub retention: String,
    pub role: String,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TransportEnvelopeIdentity {
    pub address_space: String,
    pub start: u64,
    pub end: u64,
    pub compatibility: String,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RuntimeReservationIdentity {
    pub id: String,
    pub address_space: String,
    pub bytes: u64,
    pub accounting: ReservationAccountingIdentity,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(super) enum ReservationAccountingIdentity {
    Dedicated { charge: String },
    SharedPool { pool: String, charge: String },
    External,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(super) enum BuildStatus {
    Success,
    MemoryOverflow {
        regions: Vec<MemoryOverflowIdentity>,
    },
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MemoryOverflowIdentity {
    pub linker_region: String,
    pub overflow_bytes: u64,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "kebab-case",
    deny_unknown_fields
)]
pub(super) enum Evidence<T> {
    Complete(T),
    Partial(T),
    Unavailable,
}

impl<T> Evidence<T> {
    pub(super) const fn complete(&self) -> Option<&T> {
        match self {
            Self::Complete(value) => Some(value),
            Self::Partial(_) | Self::Unavailable => None,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FirmwareFlashUsage {
    pub region: String,
    pub start: u64,
    pub end: u64,
    pub image_bytes: u64,
    pub headroom_bytes: u64,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RamBackingUsage {
    pub backing_store: String,
    pub address_spaces: Vec<String>,
    pub capacity: RamCapacityIdentity,
    pub static_section_bytes: u64,
    pub linker_padding_bytes: u64,
    pub additional_reservation_bytes: u64,
    pub included_reservation_bytes: u64,
    pub external_reservation_bytes: u64,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(super) enum RamCapacityIdentity {
    Known { bytes: u64, headroom_bytes: u64 },
    RuntimeDetected,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ArtifactIdentity {
    pub path: String,
    pub bytes: u64,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AnalysisEvidence {
    pub linker_map_bytes: u64,
    pub allocated_sections: Evidence<Vec<SectionUsage>>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SectionUsage {
    pub name: String,
    pub kind: SectionKindIdentity,
    pub run_address: u64,
    pub run_end: u64,
    pub run_bytes: u64,
    pub load_bytes: u64,
    pub alignment: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum SectionKindIdentity {
    Code,
    ReadOnlyData,
    InitializedData,
    ZeroFill,
    Other,
}
