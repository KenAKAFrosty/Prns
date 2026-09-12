mod model;
mod render;
mod validation;

use std::collections::BTreeSet;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

use super::model::{
    ArtifactIdentity, AsyncMemoryIdentity, AttributionCategoryIdentity, AttributionEntryIdentity,
    BuildIdentity, BuildStatus, Evidence, ExecutableIdentity, FirmwareFlashUsage,
    FlashAttributionIdentity, FutureSizeUnavailableReasonIdentity, MemoryOverflowIdentity,
    ModeledChainAssessmentIdentity, RamBackingUsage, RamCapacityIdentity, ResourceReport,
    ScenarioFutureSizesIdentity, SectionKindIdentity, SectionUsage, StackAnalysisGapKindIdentity,
    StackAnalysisIdentity, StackReservationIdentity,
};
use model::{
    ArtifactComparison, AsyncMemoryComparison, AttributionCandidateBaseline,
    AttributionCandidateComparison, AttributionCategoriesComparison, AttributionCategoryComparison,
    AttributionComparison, AttributionCoverageComparison, ByteComparison, ChangeState,
    CountComparison, EvidenceAvailability, EvidenceComparison, ExecutableComparison,
    FlashComparison, ModeledChainAssessmentComparison, NamedCountComparison, NamedSizeComparison,
    OverflowComparison, OverflowState, RamComparison, RamHeadroomComparison, ResourceComparison,
    ScenarioFutureComparison, SectionComparison, SettingDifference, StackComparison,
    StackEvidenceComparison, StackReservationComparison, StatusComparison, StatusKind,
};

const SECTION_KINDS: [(SectionKindIdentity, &str); 5] = [
    (SectionKindIdentity::Code, "code"),
    (SectionKindIdentity::ReadOnlyData, "read-only-data"),
    (SectionKindIdentity::InitializedData, "initialized-data"),
    (SectionKindIdentity::ZeroFill, "zero-fill"),
    (SectionKindIdentity::Other, "other"),
];
const ATTRIBUTION_CANDIDATE_LIMIT: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompatibilityDimension {
    Target,
    Architecture,
    BuildRecipe,
    Toolchain,
    MemoryContract,
    FirmwareRegion,
    ArtifactSet,
    RamTopology,
}

#[derive(Debug, Error)]
pub enum ComparisonError {
    #[error("could not read resource report {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not parse resource report {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error(
        "resource report {path} uses schema {actual}, but this tool supports schema {supported}"
    )]
    UnsupportedSchema {
        path: PathBuf,
        actual: u32,
        supported: u32,
    },
    #[error("resource report {path} has invalid flash accounting")]
    InvalidFlashAccounting { path: PathBuf },
    #[error("resource report {path} has no flash evidence")]
    MissingFlashEvidence { path: PathBuf },
    #[error("resource report {path} has no artifact evidence")]
    MissingArtifactEvidence { path: PathBuf },
    #[error("resource report {path} has invalid artifact evidence for {artifact:?}")]
    InvalidArtifact { path: PathBuf, artifact: String },
    #[error("resource report {path} repeats artifact {artifact:?}")]
    DuplicateArtifact { path: PathBuf, artifact: String },
    #[error("resource report {path} has no RAM evidence")]
    MissingRamEvidence { path: PathBuf },
    #[error("resource report {path} repeats RAM backing store {backing_store:?}")]
    DuplicateRamBacking {
        path: PathBuf,
        backing_store: String,
    },
    #[error("resource report {path} has invalid RAM accounting for {backing_store:?}")]
    InvalidRamAccounting {
        path: PathBuf,
        backing_store: String,
    },
    #[error("resource report {path} has no linker-map evidence")]
    MissingLinkerMapEvidence { path: PathBuf },
    #[error("resource report {path} has an invalid toolchain identity")]
    InvalidToolchainIdentity { path: PathBuf },
    #[error("resource report {path} has an invalid build identity")]
    InvalidBuildIdentity { path: PathBuf },
    #[error("resource report {path} has an invalid memory-contract identity")]
    InvalidMemoryContractIdentity { path: PathBuf },
    #[error("resource report {path} has an invalid report fingerprint")]
    InvalidReportFingerprint { path: PathBuf },
    #[error("resource report {path} has no allocated-section evidence")]
    MissingSectionEvidence { path: PathBuf },
    #[error("resource report {path} has invalid section accounting for {section:?}")]
    InvalidSectionAccounting { path: PathBuf, section: String },
    #[error("resource report {path} has no flash-attribution evidence")]
    MissingAttributionEvidence { path: PathBuf },
    #[error("resource report {path} has no executable evidence")]
    MissingExecutableEvidence { path: PathBuf },
    #[error("resource report {path} has no async-memory evidence")]
    MissingAsyncMemoryEvidence { path: PathBuf },
    #[error("resource report {path} has invalid executable evidence: {reason}")]
    InvalidExecutableEvidence { path: PathBuf, reason: &'static str },
    #[error("resource report {path} has invalid async-memory evidence")]
    InvalidAsyncMemoryEvidence { path: PathBuf },
    #[error("resource report {path} has invalid {category} attribution")]
    InvalidAttribution {
        path: PathBuf,
        category: &'static str,
    },
    #[error("resource report {path} has no memory-overflow diagnostics")]
    MissingOverflowEvidence { path: PathBuf },
    #[error("resource report {path} has invalid memory-overflow evidence for {linker_region:?}")]
    InvalidOverflowEvidence {
        path: PathBuf,
        linker_region: String,
    },
    #[error("resource comparison overflowed the {kind} section total")]
    SectionTotalOverflow { kind: &'static str },
    #[error("resource reports are incompatible in {dimension}")]
    Incompatible { dimension: CompatibilityDimension },
}

pub(crate) fn compare_files(before: &Path, after: &Path) -> Result<String, ComparisonError> {
    let before_report = validation::load(before)?;
    let after_report = validation::load(after)?;
    let comparison = compare_reports(&before_report, &after_report)?;
    Ok(render::render(&comparison, before, after))
}

pub(super) fn load_report(path: &Path) -> Result<ResourceReport, ComparisonError> {
    validation::load(path)
}

pub(in crate::report) fn validate_report(
    path: &Path,
    report: &ResourceReport,
) -> Result<(), ComparisonError> {
    validation::validate_report(path, report)
}

#[cfg(test)]
pub(super) fn render_comparison(
    comparison: &ResourceComparison,
    before: &Path,
    after: &Path,
) -> String {
    render::render(comparison, before, after)
}

pub(super) fn compare_reports(
    before: &ResourceReport,
    after: &ResourceReport,
) -> Result<ResourceComparison, ComparisonError> {
    compare_reports_with(before, after, ToolchainCompatibility::Exact)
}

pub(in crate::report) fn require_matrix_compatible(
    before: &ResourceReport,
    after: &ResourceReport,
) -> Result<(), ComparisonError> {
    compare_reports_with(before, after, ToolchainCompatibility::MayDiffer).map(drop)
}

#[derive(Clone, Copy)]
enum ToolchainCompatibility {
    Exact,
    MayDiffer,
}

fn compare_reports_with(
    before: &ResourceReport,
    after: &ResourceReport,
    toolchain_compatibility: ToolchainCompatibility,
) -> Result<ResourceComparison, ComparisonError> {
    require(
        before.target == after.target,
        CompatibilityDimension::Target,
    )?;
    require(
        before.architecture == after.architecture,
        CompatibilityDimension::Architecture,
    )?;
    require(
        compatible_build(&before.build, &after.build),
        CompatibilityDimension::BuildRecipe,
    )?;
    if matches!(toolchain_compatibility, ToolchainCompatibility::Exact) {
        require(
            before.toolchain == after.toolchain,
            CompatibilityDimension::Toolchain,
        )?;
    }
    require(
        before.memory_contract == after.memory_contract,
        CompatibilityDimension::MemoryContract,
    )?;
    let mut settings = Vec::new();
    if before.build.requested.lto != after.build.requested.lto {
        settings.push(SettingDifference::RequestedLto {
            before: before.build.requested.lto.as_str().to_string(),
            after: after.build.requested.lto.as_str().to_string(),
        });
    }
    if before.build.effective_release.lto != after.build.effective_release.lto {
        settings.push(SettingDifference::EffectiveLto {
            before: before.build.effective_release.lto.as_str().to_string(),
            after: after.build.effective_release.lto.as_str().to_string(),
        });
    }
    Ok(ResourceComparison {
        target: before.target.id.clone(),
        settings,
        status: compare_status(&before.status, &after.status),
        flash: compare_evidence(&before.firmware_flash, &after.firmware_flash, compare_flash)?,
        artifacts: compare_evidence(&before.artifacts, &after.artifacts, |before, after| {
            compare_artifacts(before, after)
        })?,
        ram: compare_evidence(&before.static_ram, &after.static_ram, |before, after| {
            compare_ram(before, after)
        })?,
        sections: compare_evidence(
            &before.analysis.allocated_sections,
            &after.analysis.allocated_sections,
            |before, after| compare_sections(before, after),
        )?,
        attribution: compare_attribution(
            &before.analysis.flash_attribution,
            &after.analysis.flash_attribution,
        ),
        executable: compare_evidence(
            &before.analysis.executable,
            &after.analysis.executable,
            |before, after| Ok(compare_executable(before, after)),
        )?,
        stack: compare_stack_evidence(&before.analysis.executable, &after.analysis.executable),
        async_memory: compare_evidence(
            &before.analysis.async_memory,
            &after.analysis.async_memory,
            |before, after| Ok(compare_async_memory(before, after)),
        )?,
    })
}

fn compare_stack_evidence(
    before: &Evidence<ExecutableIdentity>,
    after: &Evidence<ExecutableIdentity>,
) -> StackEvidenceComparison {
    let (before_availability, before_stack) = stack_evidence(before);
    let (after_availability, after_stack) = stack_evidence(after);
    match (before_stack, after_stack) {
        (Some(before), Some(after)) => StackEvidenceComparison::Comparable {
            before: before_availability,
            after: after_availability,
            value: Box::new(compare_stack(before, after)),
        },
        _ => StackEvidenceComparison::NotComparable {
            before: before_availability,
            after: after_availability,
        },
    }
}

fn stack_evidence(
    executable: &Evidence<ExecutableIdentity>,
) -> (EvidenceAvailability, Option<&StackAnalysisIdentity>) {
    let executable = match executable {
        Evidence::Complete(executable) | Evidence::Partial(executable) => executable,
        Evidence::Unavailable => return (EvidenceAvailability::Unavailable, None),
    };
    match &executable.stack {
        Evidence::Complete(stack) => (EvidenceAvailability::Complete, Some(stack)),
        Evidence::Partial(stack) => (EvidenceAvailability::Partial, Some(stack)),
        Evidence::Unavailable => (EvidenceAvailability::Unavailable, None),
    }
}

fn compare_stack(before: &StackAnalysisIdentity, after: &StackAnalysisIdentity) -> StackComparison {
    let frame_names = before
        .largest_frames
        .iter()
        .map(|frame| frame.name.as_str())
        .chain(after.largest_frames.iter().map(|frame| frame.name.as_str()))
        .collect::<BTreeSet<_>>();
    let gap_names = before
        .gaps
        .iter()
        .map(|gap| stack_gap_name(gap.kind))
        .chain(after.gaps.iter().map(|gap| stack_gap_name(gap.kind)))
        .collect::<BTreeSet<_>>();
    StackComparison {
        frame_source: change_state(before.frame_source == after.frame_source),
        source_bytes: ByteComparison::new(before.source_bytes, after.source_bytes),
        frames: CountComparison::new(before.frame_count, after.frame_count),
        modeled_chain: ByteComparison::new(
            before.largest_modeled_direct_call_chain.bytes,
            after.largest_modeled_direct_call_chain.bytes,
        ),
        changed_largest_frames: frame_names
            .into_iter()
            .filter(|name| {
                before
                    .largest_frames
                    .iter()
                    .find(|frame| frame.name == **name)
                    != after
                        .largest_frames
                        .iter()
                        .find(|frame| frame.name == **name)
            })
            .map(str::to_string)
            .collect(),
        reservation: match (&before.reservation, &after.reservation) {
            (
                StackReservationIdentity::Declared {
                    reservation,
                    bytes,
                    assessment: before,
                },
                StackReservationIdentity::Declared {
                    reservation: after_reservation,
                    bytes: after_bytes,
                    assessment: after,
                },
            ) if reservation == after_reservation && bytes == after_bytes => {
                StackReservationComparison::Declared {
                    reservation: reservation.clone(),
                    bytes: *bytes,
                    assessment: compare_modeled_chain_assessment(before, after),
                }
            }
            (StackReservationIdentity::Undeclared, StackReservationIdentity::Undeclared) => {
                StackReservationComparison::Undeclared
            }
            _ => StackReservationComparison::Changed,
        },
        gaps: gap_names
            .into_iter()
            .map(|name| NamedCountComparison {
                name: name.to_string(),
                count: CountComparison::new(
                    stack_gap_count(before, name),
                    stack_gap_count(after, name),
                ),
            })
            .collect(),
    }
}

fn compare_modeled_chain_assessment(
    before: &ModeledChainAssessmentIdentity,
    after: &ModeledChainAssessmentIdentity,
) -> ModeledChainAssessmentComparison {
    match (before, after) {
        (
            ModeledChainAssessmentIdentity::WithinReservation {
                remaining_bytes: before,
            },
            ModeledChainAssessmentIdentity::WithinReservation {
                remaining_bytes: after,
            },
        ) => ModeledChainAssessmentComparison::WithinReservation {
            remaining: ByteComparison::new(*before, *after),
        },
        (
            ModeledChainAssessmentIdentity::OverReservation {
                excess_bytes: before,
            },
            ModeledChainAssessmentIdentity::OverReservation {
                excess_bytes: after,
            },
        ) => ModeledChainAssessmentComparison::OverReservation {
            excess: ByteComparison::new(*before, *after),
        },
        _ => ModeledChainAssessmentComparison::Changed,
    }
}

fn stack_gap_count(stack: &StackAnalysisIdentity, name: &str) -> u64 {
    stack
        .gaps
        .iter()
        .find(|gap| stack_gap_name(gap.kind) == name)
        .map_or(0, |gap| gap.occurrences)
}

const fn stack_gap_name(kind: StackAnalysisGapKindIdentity) -> &'static str {
    match kind {
        StackAnalysisGapKindIdentity::FunctionsWithoutFrameEvidence => {
            "functions-without-frame-evidence"
        }
        StackAnalysisGapKindIdentity::ForeignOrAssemblyFrames => "foreign-or-assembly-frames",
        StackAnalysisGapKindIdentity::FrameEvidenceWithoutFunction => {
            "frame-evidence-without-function"
        }
        StackAnalysisGapKindIdentity::UnsupportedCfaRule => "unsupported-cfa-rule",
        StackAnalysisGapKindIdentity::CallSiteOutsideFunction => "call-site-outside-function",
        StackAnalysisGapKindIdentity::DirectCallOutsideFunctions => "direct-call-outside-functions",
        StackAnalysisGapKindIdentity::UnresolvedDirectCall => "unresolved-direct-call",
        StackAnalysisGapKindIdentity::IndirectCall => "indirect-call",
        StackAnalysisGapKindIdentity::RecursiveCallCycle => "recursive-call-cycle",
        StackAnalysisGapKindIdentity::RootOutsideFunctions => "root-outside-functions",
        StackAnalysisGapKindIdentity::InterruptRootsUnresolved => "interrupt-roots-unresolved",
        StackAnalysisGapKindIdentity::DynamicAllocationNotProvenAbsent => {
            "dynamic-allocation-not-proven-absent"
        }
        StackAnalysisGapKindIdentity::InterruptNestingUnmodeled => "interrupt-nesting-unmodeled",
        StackAnalysisGapKindIdentity::StackReservationUndeclared => "stack-reservation-undeclared",
    }
}

fn compare_async_memory(
    before: &AsyncMemoryIdentity,
    after: &AsyncMemoryIdentity,
) -> AsyncMemoryComparison {
    AsyncMemoryComparison {
        task_pool_total: ByteComparison::new(before.task_pool_bytes, after.task_pool_bytes),
        task_pools: named_sizes(
            before
                .task_pools
                .iter()
                .map(|pool| (pool.task.as_str(), pool.bytes)),
            after
                .task_pools
                .iter()
                .map(|pool| (pool.task.as_str(), pool.bytes)),
        ),
        scenario_futures: compare_scenario_futures(
            &before.scenario_futures,
            &after.scenario_futures,
        ),
    }
}

fn compare_scenario_futures(
    before: &ScenarioFutureSizesIdentity,
    after: &ScenarioFutureSizesIdentity,
) -> ScenarioFutureComparison {
    match (before, after) {
        (
            ScenarioFutureSizesIdentity::Measured { futures: before },
            ScenarioFutureSizesIdentity::Measured { futures: after },
        ) => ScenarioFutureComparison::Measured(named_sizes(
            before
                .iter()
                .map(|future| (future.scenario.as_str(), future.bytes)),
            after
                .iter()
                .map(|future| (future.scenario.as_str(), future.bytes)),
        )),
        (
            ScenarioFutureSizesIdentity::Unavailable { reason: before },
            ScenarioFutureSizesIdentity::Unavailable { reason: after },
        ) => ScenarioFutureComparison::Unavailable {
            before: future_reason(*before).to_string(),
            after: future_reason(*after).to_string(),
        },
        _ => ScenarioFutureComparison::AvailabilityChanged {
            before: scenario_future_state(before).to_string(),
            after: scenario_future_state(after).to_string(),
        },
    }
}

fn named_sizes<'a>(
    before: impl Iterator<Item = (&'a str, u64)>,
    after: impl Iterator<Item = (&'a str, u64)>,
) -> Vec<NamedSizeComparison> {
    let before = before.collect::<std::collections::BTreeMap<_, _>>();
    let after = after.collect::<std::collections::BTreeMap<_, _>>();
    before
        .keys()
        .chain(after.keys())
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|name| NamedSizeComparison {
            name: name.to_string(),
            before: before.get(name).copied(),
            after: after.get(name).copied(),
        })
        .collect()
}

const fn future_reason(reason: FutureSizeUnavailableReasonIdentity) -> &'static str {
    match reason {
        FutureSizeUnavailableReasonIdentity::SemanticHarnessNotProduced => {
            "semantic-harness-not-produced"
        }
    }
}

const fn scenario_future_state(futures: &ScenarioFutureSizesIdentity) -> &'static str {
    match futures {
        ScenarioFutureSizesIdentity::Measured { .. } => "measured",
        ScenarioFutureSizesIdentity::Unavailable { reason } => future_reason(*reason),
    }
}

fn compare_executable(
    before: &ExecutableIdentity,
    after: &ExecutableIdentity,
) -> ExecutableComparison {
    let section_names = before
        .executable_sections
        .iter()
        .map(|section| section.name.as_str())
        .chain(
            after
                .executable_sections
                .iter()
                .map(|section| section.name.as_str()),
        )
        .collect::<BTreeSet<_>>();
    let changed_sections = section_names
        .into_iter()
        .filter(|name| {
            let before = before
                .executable_sections
                .iter()
                .find(|section| section.name == **name);
            let after = after
                .executable_sections
                .iter()
                .find(|section| section.name == **name);
            before != after
        })
        .map(str::to_string)
        .collect();
    let ranked_names = before
        .functions
        .largest
        .iter()
        .map(|function| function.name.as_str())
        .chain(
            after
                .functions
                .largest
                .iter()
                .map(|function| function.name.as_str()),
        )
        .collect::<BTreeSet<_>>();
    let changed_ranked_functions = ranked_names
        .into_iter()
        .filter(|name| {
            let before = before
                .functions
                .largest
                .iter()
                .find(|function| function.name == **name);
            let after = after
                .functions
                .largest
                .iter()
                .find(|function| function.name == **name);
            before != after
        })
        .map(str::to_string)
        .collect();
    ExecutableComparison {
        entry_point: change_state(before.entry_point == after.entry_point),
        section_bytes: ByteComparison::new(
            before.disassembly.executable_bytes,
            after.disassembly.executable_bytes,
        ),
        changed_sections,
        function_boundaries: change_state(
            before.functions.boundaries_fingerprint == after.functions.boundaries_fingerprint,
        ),
        functions_before: before.functions.boundary_count,
        functions_after: after.functions.boundary_count,
        changed_ranked_functions,
        decoded_bytes: ByteComparison::new(
            before.disassembly.decoded_bytes,
            after.disassembly.decoded_bytes,
        ),
        undecoded_bytes: ByteComparison::new(
            before.disassembly.undecoded_bytes,
            after.disassembly.undecoded_bytes,
        ),
    }
}

const fn change_state(unchanged: bool) -> ChangeState {
    if unchanged {
        ChangeState::Unchanged
    } else {
        ChangeState::Changed
    }
}

fn compare_attribution(
    before: &Evidence<FlashAttributionIdentity>,
    after: &Evidence<FlashAttributionIdentity>,
) -> AttributionComparison {
    let before_availability = evidence_availability(before);
    let after_availability = evidence_availability(after);
    match (before, after) {
        (Evidence::Unavailable, _) | (_, Evidence::Unavailable) => {
            AttributionComparison::NotComparable {
                before: before_availability,
                after: after_availability,
            }
        }
        (
            Evidence::Complete(before) | Evidence::Partial(before),
            Evidence::Complete(after) | Evidence::Partial(after),
        ) => AttributionComparison::Comparable {
            before: before_availability,
            after: after_availability,
            categories: Box::new(AttributionCategoriesComparison {
                crates: compare_attribution_category(&before.crates, &after.crates),
                symbols: compare_attribution_category(&before.symbols, &after.symbols),
            }),
        },
    }
}

fn compare_attribution_category(
    before: &AttributionCategoryIdentity,
    after: &AttributionCategoryIdentity,
) -> AttributionCategoryComparison {
    AttributionCategoryComparison {
        coverage: AttributionCoverageComparison {
            analyzed: ByteComparison::new(
                before.coverage.analyzed_bytes,
                after.coverage.analyzed_bytes,
            ),
            attributed: ByteComparison::new(
                before.coverage.attributed_bytes,
                after.coverage.attributed_bytes,
            ),
            unclassified: ByteComparison::new(
                before.coverage.unclassified_bytes,
                after.coverage.unclassified_bytes,
            ),
        },
        candidates: after
            .largest
            .iter()
            .take(ATTRIBUTION_CANDIDATE_LIMIT)
            .enumerate()
            .map(|(index, entry)| AttributionCandidateComparison {
                rank: index + 1,
                name: entry.name.clone(),
                before: ranked_bytes(&before.largest, &entry.name),
                after_bytes: entry.bytes,
            })
            .collect(),
    }
}

fn ranked_bytes(entries: &[AttributionEntryIdentity], name: &str) -> AttributionCandidateBaseline {
    entries
        .iter()
        .find(|entry| entry.name == name)
        .map_or(AttributionCandidateBaseline::NotRanked, |entry| {
            AttributionCandidateBaseline::Ranked(entry.bytes)
        })
}

fn compare_status(before: &BuildStatus, after: &BuildStatus) -> StatusComparison {
    let mut regions = overflow_regions(before)
        .iter()
        .map(|overflow| overflow.linker_region.as_str())
        .collect::<Vec<_>>();
    for overflow in overflow_regions(after) {
        if !regions.contains(&overflow.linker_region.as_str()) {
            regions.push(&overflow.linker_region);
        }
    }
    StatusComparison {
        before: status_kind(before),
        after: status_kind(after),
        overflows: regions
            .into_iter()
            .map(|linker_region| OverflowComparison {
                linker_region: linker_region.to_string(),
                before: overflow_state(before, linker_region),
                after: overflow_state(after, linker_region),
            })
            .collect(),
    }
}

const fn status_kind(status: &BuildStatus) -> StatusKind {
    match status {
        BuildStatus::Success => StatusKind::Success,
        BuildStatus::MemoryOverflow { .. } => StatusKind::MemoryOverflow,
    }
}

fn overflow_regions(status: &BuildStatus) -> &[MemoryOverflowIdentity] {
    match status {
        BuildStatus::Success => &[],
        BuildStatus::MemoryOverflow { regions } => regions,
    }
}

fn overflow_state(status: &BuildStatus, linker_region: &str) -> OverflowState {
    match status {
        BuildStatus::Success => OverflowState::NoOverflow,
        BuildStatus::MemoryOverflow { regions } => regions
            .iter()
            .find(|overflow| overflow.linker_region == linker_region)
            .map_or(OverflowState::NotReported, |overflow| {
                OverflowState::Overflow(overflow.overflow_bytes)
            }),
    }
}

fn compare_evidence<T, U>(
    before: &Evidence<T>,
    after: &Evidence<T>,
    compare: impl FnOnce(&T, &T) -> Result<U, ComparisonError>,
) -> Result<EvidenceComparison<U>, ComparisonError> {
    match (before, after) {
        (Evidence::Complete(before), Evidence::Complete(after)) => {
            compare(before, after).map(EvidenceComparison::Comparable)
        }
        _ => Ok(EvidenceComparison::NotComparable {
            before: evidence_availability(before),
            after: evidence_availability(after),
        }),
    }
}

const fn evidence_availability<T>(evidence: &Evidence<T>) -> EvidenceAvailability {
    match evidence {
        Evidence::Complete(_) => EvidenceAvailability::Complete,
        Evidence::Partial(_) => EvidenceAvailability::Partial,
        Evidence::Unavailable => EvidenceAvailability::Unavailable,
    }
}

fn compare_flash(
    before: &FirmwareFlashUsage,
    after: &FirmwareFlashUsage,
) -> Result<FlashComparison, ComparisonError> {
    require(
        before.region == after.region && before.start == after.start && before.end == after.end,
        CompatibilityDimension::FirmwareRegion,
    )?;
    Ok(FlashComparison {
        image: ByteComparison::new(before.image_bytes, after.image_bytes),
        headroom: ByteComparison::new(before.headroom_bytes, after.headroom_bytes),
    })
}

fn compatible_build(before: &BuildIdentity, after: &BuildIdentity) -> bool {
    let lto_matches = before.requested.lto == after.requested.lto
        && before.effective_release.lto == after.effective_release.lto;
    let fingerprints_match_difference = if lto_matches {
        before.fingerprint == after.fingerprint
    } else {
        before.fingerprint != after.fingerprint
    };
    fingerprints_match_difference
        && before.firmware_version == after.firmware_version
        && before.cargo_profile == after.cargo_profile
        && before.recipe_kind == after.recipe_kind
        && before.manifest == after.manifest
        && before.package == after.package
        && before.binary == after.binary
        && before.features == after.features
        && compatible_release_except_lto(&before.effective_release, &after.effective_release)
        && before.package_overrides == after.package_overrides
        && before.build_override == after.build_override
}

fn compatible_release_except_lto(
    before: &super::model::ReleaseSettingsIdentity,
    after: &super::model::ReleaseSettingsIdentity,
) -> bool {
    before.opt_level == after.opt_level
        && before.debug == after.debug
        && before.split_debuginfo == after.split_debuginfo
        && before.strip == after.strip
        && before.debug_assertions == after.debug_assertions
        && before.overflow_checks == after.overflow_checks
        && before.panic == after.panic
        && before.incremental == after.incremental
        && before.codegen_units == after.codegen_units
        && before.rpath == after.rpath
}

fn compare_artifacts(
    before: &[ArtifactIdentity],
    after: &[ArtifactIdentity],
) -> Result<Vec<ArtifactComparison>, ComparisonError> {
    require(
        before.len() == after.len(),
        CompatibilityDimension::ArtifactSet,
    )?;
    before
        .iter()
        .map(|artifact| {
            let Some(other) = after.iter().find(|other| other.path == artifact.path) else {
                return Err(ComparisonError::Incompatible {
                    dimension: CompatibilityDimension::ArtifactSet,
                });
            };
            Ok(ArtifactComparison {
                path: artifact.path.clone(),
                bytes: ByteComparison::new(artifact.bytes, other.bytes),
                fingerprint: change_state(artifact.fingerprint == other.fingerprint),
            })
        })
        .collect()
}

fn compare_ram(
    before: &[RamBackingUsage],
    after: &[RamBackingUsage],
) -> Result<Vec<RamComparison>, ComparisonError> {
    require(
        before.len() == after.len(),
        CompatibilityDimension::RamTopology,
    )?;
    before
        .iter()
        .map(|usage| {
            let Some(other) = after
                .iter()
                .find(|other| other.backing_store == usage.backing_store)
            else {
                return Err(ComparisonError::Incompatible {
                    dimension: CompatibilityDimension::RamTopology,
                });
            };
            require(
                compatible_ram(usage, other),
                CompatibilityDimension::RamTopology,
            )?;
            let headroom = match (&usage.capacity, &other.capacity) {
                (
                    RamCapacityIdentity::Known {
                        headroom_bytes: before,
                        ..
                    },
                    RamCapacityIdentity::Known {
                        headroom_bytes: after,
                        ..
                    },
                ) => RamHeadroomComparison::Known(ByteComparison::new(*before, *after)),
                (RamCapacityIdentity::RuntimeDetected, RamCapacityIdentity::RuntimeDetected) => {
                    RamHeadroomComparison::RuntimeDetected
                }
                _ => {
                    return Err(ComparisonError::Incompatible {
                        dimension: CompatibilityDimension::RamTopology,
                    });
                }
            };
            Ok(RamComparison {
                backing_store: usage.backing_store.clone(),
                static_sections: ByteComparison::new(
                    usage.static_section_bytes,
                    other.static_section_bytes,
                ),
                linker_padding: ByteComparison::new(
                    usage.linker_padding_bytes,
                    other.linker_padding_bytes,
                ),
                headroom,
            })
        })
        .collect()
}

fn compatible_ram(before: &RamBackingUsage, after: &RamBackingUsage) -> bool {
    before.address_spaces == after.address_spaces
        && before.additional_reservation_bytes == after.additional_reservation_bytes
        && before.included_reservation_bytes == after.included_reservation_bytes
        && before.external_reservation_bytes == after.external_reservation_bytes
        && match (&before.capacity, &after.capacity) {
            (
                RamCapacityIdentity::Known { bytes: before, .. },
                RamCapacityIdentity::Known { bytes: after, .. },
            ) => before == after,
            (RamCapacityIdentity::RuntimeDetected, RamCapacityIdentity::RuntimeDetected) => true,
            _ => false,
        }
}

fn compare_sections(
    before: &[SectionUsage],
    after: &[SectionUsage],
) -> Result<Vec<SectionComparison>, ComparisonError> {
    SECTION_KINDS
        .iter()
        .map(|(kind, name)| {
            let (before_run, before_load) = section_totals(before, *kind)
                .ok_or(ComparisonError::SectionTotalOverflow { kind: name })?;
            let (after_run, after_load) = section_totals(after, *kind)
                .ok_or(ComparisonError::SectionTotalOverflow { kind: name })?;
            Ok(SectionComparison {
                kind: name,
                run_bytes: ByteComparison::new(before_run, after_run),
                load_bytes: ByteComparison::new(before_load, after_load),
            })
        })
        .collect()
}

fn section_totals(sections: &[SectionUsage], kind: SectionKindIdentity) -> Option<(u64, u64)> {
    sections
        .iter()
        .filter(|section| section.kind == kind)
        .try_fold((0_u64, 0_u64), |(run, load), section| {
            Some((
                run.checked_add(section.run_bytes)?,
                load.checked_add(section.load_bytes)?,
            ))
        })
}

fn require(condition: bool, dimension: CompatibilityDimension) -> Result<(), ComparisonError> {
    if condition {
        Ok(())
    } else {
        Err(ComparisonError::Incompatible { dimension })
    }
}

impl fmt::Display for CompatibilityDimension {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Target => "target identity",
            Self::Architecture => "architecture identity",
            Self::BuildRecipe => "build recipe",
            Self::Toolchain => "toolchain identity",
            Self::MemoryContract => "memory contract",
            Self::FirmwareRegion => "firmware region",
            Self::ArtifactSet => "artifact set",
            Self::RamTopology => "RAM topology",
        })
    }
}
