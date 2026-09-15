use personal_hopspot_builder::SourceCustody;
use serde::{Deserialize, Serialize};

use super::fingerprint::Fingerprint;

pub(super) const SCHEMA_VERSION: u32 = 9;

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ResourceReport {
    pub schema_version: u32,
    pub source: SourceCustody,
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
    pub rustflags: Vec<String>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BuildIdentity {
    pub fingerprint: Fingerprint,
    pub firmware_version: String,
    pub cargo_profile: String,
    pub recipe_kind: String,
    pub manifest: String,
    pub package: String,
    pub binary: String,
    pub features: Vec<String>,
    pub requested: RequestedBuildSettingsIdentity,
    pub effective_release: ReleaseSettingsIdentity,
    pub package_overrides: Vec<PackageReleaseOverrideIdentity>,
    pub build_override: PackageReleaseSettingsIdentity,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RequestedBuildSettingsIdentity {
    pub lto: RequestedLtoIdentity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum RequestedLtoIdentity {
    Configured,
    Fat,
    Thin,
}

impl RequestedLtoIdentity {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Configured => "configured",
            Self::Fat => "fat",
            Self::Thin => "thin",
        }
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReleaseSettingsIdentity {
    pub opt_level: OptimizationLevelIdentity,
    pub debug: DebugInfoIdentity,
    pub split_debuginfo: SplitDebuginfoIdentity,
    pub strip: StripIdentity,
    pub debug_assertions: bool,
    pub overflow_checks: bool,
    pub lto: CargoLtoIdentity,
    pub panic: PanicStrategyIdentity,
    pub incremental: bool,
    pub codegen_units: u32,
    pub rpath: bool,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PackageReleaseOverrideIdentity {
    pub package: String,
    pub settings: PackageReleaseSettingsIdentity,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PackageReleaseSettingsIdentity {
    pub opt_level: OptimizationLevelIdentity,
    pub debug: DebugInfoIdentity,
    pub split_debuginfo: SplitDebuginfoIdentity,
    pub strip: StripIdentity,
    pub debug_assertions: bool,
    pub overflow_checks: bool,
    pub incremental: bool,
    pub codegen_units: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum OptimizationLevelIdentity {
    Zero,
    One,
    Two,
    Three,
    Size,
    SizeMin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum DebugInfoIdentity {
    None,
    Limited,
    Full,
    LineTablesOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum SplitDebuginfoIdentity {
    ToolchainDefault,
    Off,
    Packed,
    Unpacked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum StripIdentity {
    None,
    Debuginfo,
    Symbols,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum CargoLtoIdentity {
    False,
    Fat,
    Thin,
    Off,
}

impl CargoLtoIdentity {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::False => "false",
            Self::Fat => "fat",
            Self::Thin => "thin",
            Self::Off => "off",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum PanicStrategyIdentity {
    Unwind,
    Abort,
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
    pub linker_ranges: Vec<AddressRangeIdentity>,
    pub backing_store: String,
    pub backing_offset: u64,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AddressRangeIdentity {
    pub start: u64,
    pub end: u64,
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
    pub fingerprint: Fingerprint,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AnalysisEvidence {
    pub linker_map_bytes: u64,
    pub allocated_sections: Evidence<Vec<SectionUsage>>,
    pub flash_attribution: Evidence<FlashAttributionIdentity>,
    pub executable: Evidence<ExecutableIdentity>,
    pub async_memory: Evidence<AsyncMemoryIdentity>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExecutableIdentity {
    pub rust_target: String,
    pub architecture: ExecutableArchitectureIdentity,
    pub byte_order: ByteOrderIdentity,
    pub entry_point: u64,
    pub load_segments: Vec<LoadSegmentIdentity>,
    pub executable_sections: Vec<ExecutableSectionIdentity>,
    pub startup: StartupStructureIdentity,
    pub functions: FunctionAnalysisIdentity,
    pub disassembly: DisassemblyIdentity,
    pub stack: Evidence<StackAnalysisIdentity>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StackAnalysisIdentity {
    pub frame_source: StackFrameSourceIdentity,
    pub source_bytes: u64,
    pub frame_count: u64,
    pub functions_without_frames: u64,
    pub largest_frames: Vec<StackFrameIdentity>,
    pub roots: Vec<StackRootIdentity>,
    pub direct_call_count: u64,
    pub largest_modeled_direct_call_chain: ModeledDirectCallChainIdentity,
    pub reservation: StackReservationIdentity,
    pub gaps: Vec<StackAnalysisGapIdentity>,
    pub artifact: EvidenceArtifactIdentity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum StackFrameSourceIdentity {
    LlvmStackSizes,
    DwarfDebugFrame,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StackFrameIdentity {
    pub name: String,
    pub address: u64,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StackRootIdentity {
    pub role: StackRootRoleIdentity,
    pub name: String,
    pub address: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum StackRootRoleIdentity {
    Startup,
    Interrupt,
    Trap,
    EmbassyTask,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ModeledDirectCallChainIdentity {
    pub bytes: u64,
    pub frames: Vec<ModeledCallChainFrameIdentity>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ModeledCallChainFrameIdentity {
    pub name: String,
    pub address: u64,
    pub frame_bytes: u64,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(super) enum StackReservationIdentity {
    Declared {
        reservation: String,
        bytes: u64,
        assessment: ModeledChainAssessmentIdentity,
    },
    Undeclared,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(super) enum ModeledChainAssessmentIdentity {
    WithinReservation { remaining_bytes: u64 },
    OverReservation { excess_bytes: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StackAnalysisGapIdentity {
    pub kind: StackAnalysisGapKindIdentity,
    pub occurrences: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum StackAnalysisGapKindIdentity {
    FunctionsWithoutFrameEvidence,
    ForeignOrAssemblyFrames,
    FrameEvidenceWithoutFunction,
    UnsupportedCfaRule,
    CallSiteOutsideFunction,
    DirectCallOutsideFunctions,
    UnresolvedDirectCall,
    IndirectCall,
    RecursiveCallCycle,
    RootOutsideFunctions,
    InterruptRootsUnresolved,
    DynamicAllocationNotProvenAbsent,
    InterruptNestingUnmodeled,
    StackReservationUndeclared,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AsyncMemoryIdentity {
    pub task_pool_accounting: TaskPoolAccountingIdentity,
    pub task_pool_bytes: u64,
    pub task_pools: Vec<TaskPoolAllocationIdentity>,
    pub scenario_futures: ScenarioFutureSizesIdentity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum TaskPoolAccountingIdentity {
    IncludedInStaticRam,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TaskPoolAllocationIdentity {
    pub task: String,
    pub address: u64,
    pub bytes: u64,
    pub section: String,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(super) enum ScenarioFutureSizesIdentity {
    Measured {
        futures: Vec<NamedFutureSizeIdentity>,
    },
    Unavailable {
        reason: FutureSizeUnavailableReasonIdentity,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NamedFutureSizeIdentity {
    pub scenario: String,
    pub bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum FutureSizeUnavailableReasonIdentity {
    SemanticHarnessNotProduced,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum ExecutableArchitectureIdentity {
    Thumbv7em,
    Riscv32imac,
    XtensaEsp32s3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum ByteOrderIdentity {
    Little,
    Big,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LoadSegmentIdentity {
    pub file_offset: u64,
    pub run_address: u64,
    pub run_end: u64,
    pub load_address: u64,
    pub load_end: u64,
    pub file_bytes: u64,
    pub memory_bytes: u64,
    pub alignment: u64,
    pub permissions: Vec<LoadPermissionIdentity>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum LoadPermissionIdentity {
    Read,
    Write,
    Execute,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExecutableSectionIdentity {
    pub name: String,
    pub address: u64,
    pub end: u64,
    pub bytes: u64,
    pub alignment: u64,
    pub fingerprint: Fingerprint,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StartupStructureIdentity {
    pub entry_section: String,
    pub entry_symbol: String,
    pub anchors: Vec<StartupAnchorIdentity>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StartupAnchorIdentity {
    pub role: StartupAnchorRoleIdentity,
    pub address: u64,
    pub section: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum StartupAnchorRoleIdentity {
    EntryPoint,
    InitialStackPointer,
    ResetVector,
    TrapVector,
    ExceptionVectors,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FunctionAnalysisIdentity {
    pub normalization: FunctionNormalizationIdentity,
    pub boundary_count: u64,
    pub boundaries_fingerprint: Fingerprint,
    pub boundaries_artifact: EvidenceArtifactIdentity,
    pub classified_bytes: u64,
    pub unclassified_bytes: u64,
    pub largest: Vec<FunctionBoundaryIdentity>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum FunctionNormalizationIdentity {
    LinkedFunctionBodySha256V1,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EvidenceArtifactIdentity {
    pub path: String,
    pub bytes: u64,
    pub fingerprint: Fingerprint,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FunctionBoundaryIdentity {
    pub name: String,
    pub address: u64,
    pub end: u64,
    pub bytes: u64,
    pub fingerprint: Fingerprint,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DisassemblyIdentity {
    pub adapter: ExecutableArchitectureIdentity,
    pub flavor: DisassemblerFlavorIdentity,
    pub program: String,
    pub version: String,
    pub executable_bytes: u64,
    pub decoded_bytes: u64,
    pub undecoded_bytes: u64,
    pub instruction_count: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum DisassemblerFlavorIdentity {
    LlvmObjdump,
    GnuObjdump,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FlashAttributionIdentity {
    pub crates: AttributionCategoryIdentity,
    pub symbols: AttributionCategoryIdentity,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AttributionCategoryIdentity {
    pub coverage: AttributionCoverageIdentity,
    pub largest: Vec<AttributionEntryIdentity>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AttributionCoverageIdentity {
    pub analyzed_bytes: u64,
    pub attributed_bytes: u64,
    pub unclassified_bytes: u64,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AttributionEntryIdentity {
    pub name: String,
    pub bytes: u64,
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
