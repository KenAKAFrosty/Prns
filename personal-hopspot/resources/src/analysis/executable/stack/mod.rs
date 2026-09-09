mod call_graph;
mod frames;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use personal_hopspot_builder::architecture::StackFrameEvidence;
use personal_hopspot_memory::MemoryProfile;
use thiserror::Error;

use super::architecture::{AssuranceAdapter, DecodedInstruction, StackLimit};
use super::{FunctionAnalysis, LoadSegment, StartupStructure};

const LARGEST_FRAME_LIMIT: usize = 20;

#[derive(Debug)]
pub(crate) struct StackAnalysis {
    pub(crate) frame_source: StackFrameEvidence,
    pub(crate) source_bytes: u64,
    pub(crate) frames: Vec<StackFrame>,
    pub(crate) largest_frames: Vec<StackFrame>,
    pub(crate) functions_without_frames: u64,
    pub(crate) roots: Vec<StackRoot>,
    pub(crate) direct_calls: Vec<CallEdge>,
    pub(crate) largest_known_path: KnownCallPath,
    pub(crate) limit: StackLimitAnalysis,
    pub(crate) gaps: Vec<StackAnalysisGap>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StackFrame {
    pub(crate) name: String,
    pub(crate) address: u64,
    pub(crate) bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct KnownCallPath {
    pub(crate) bytes: u64,
    pub(crate) frames: Vec<KnownCallPathFrame>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct KnownCallPathFrame {
    pub(crate) name: String,
    pub(crate) address: u64,
    pub(crate) frame_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CallEdge {
    pub(crate) caller: String,
    pub(crate) caller_address: u64,
    pub(crate) callee: String,
    pub(crate) callee_address: u64,
    pub(crate) call_site: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct StackRoot {
    pub(crate) role: StackRootRole,
    pub(crate) name: String,
    pub(crate) address: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum StackRootRole {
    Startup,
    Trap,
    EmbassyTask,
}

#[derive(Debug)]
pub(crate) enum StackLimitAnalysis {
    Declared {
        reservation: String,
        bytes: u64,
        headroom_bytes: u64,
    },
    Undeclared,
}

#[derive(Debug)]
pub(crate) struct StackAnalysisGap {
    pub(crate) kind: StackAnalysisGapKind,
    pub(crate) occurrences: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum StackAnalysisGapKind {
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
    StackLimitUndeclared,
}

pub(super) struct StackAnalysisInput<'a, 'data> {
    pub(super) path: &'a Path,
    pub(super) profile: &'a MemoryProfile,
    pub(super) adapter: &'a AssuranceAdapter,
    pub(super) object: &'a object::File<'data>,
    pub(super) startup: &'a StartupStructure,
    pub(super) functions: &'a FunctionAnalysis,
    pub(super) instructions: &'a [DecodedInstruction],
    pub(super) load_segments: &'a [LoadSegment],
    pub(super) frame_evidence: StackFrameEvidence,
}

pub(super) fn analyze(
    input: StackAnalysisInput<'_, '_>,
) -> Result<StackAnalysis, StackMetadataError> {
    let frames = frames::analyze(
        input.path,
        input.object,
        input.functions,
        input.load_segments,
        input.adapter.code_address_normalizer(),
        input.frame_evidence,
        input.adapter.dwarf_cfa_registers(),
    )?;
    let graph = call_graph::analyze(
        input.adapter,
        input.startup,
        input.functions,
        &frames.frames,
        input.instructions,
    )?;
    let mut gaps = graph.gaps;
    add_gap(
        &mut gaps,
        StackAnalysisGapKind::FunctionsWithoutFrameEvidence,
        frames.missing_functions,
    );
    add_gap(
        &mut gaps,
        StackAnalysisGapKind::ForeignOrAssemblyFrames,
        frames.foreign_or_assembly_functions,
    );
    add_gap(
        &mut gaps,
        StackAnalysisGapKind::FrameEvidenceWithoutFunction,
        frames.unmatched_records,
    );
    add_gap(
        &mut gaps,
        StackAnalysisGapKind::UnsupportedCfaRule,
        frames.unsupported_records,
    );
    add_gap(
        &mut gaps,
        StackAnalysisGapKind::InterruptRootsUnresolved,
        u64::from(
            input
                .startup
                .anchors
                .iter()
                .any(|anchor| anchor.role == super::StartupAnchorRole::ExceptionVectors),
        ),
    );
    add_gap(
        &mut gaps,
        StackAnalysisGapKind::DynamicAllocationNotProvenAbsent,
        1,
    );
    add_gap(
        &mut gaps,
        StackAnalysisGapKind::InterruptNestingUnmodeled,
        1,
    );

    let limit = match input.adapter.stack_limit() {
        StackLimit::RuntimeReservation(id) => {
            let reservation = input
                .profile
                .runtime_reservations
                .iter()
                .find(|reservation| reservation.id.0 == id)
                .ok_or_else(|| StackMetadataError::MissingReservation {
                    profile: input.profile.id.0.to_string(),
                    reservation: id,
                })?;
            let headroom_bytes = reservation
                .bytes
                .checked_sub(graph.largest_path.bytes)
                .ok_or(StackMetadataError::KnownPathOverflow {
                    reservation: id,
                    available: reservation.bytes,
                    required: graph.largest_path.bytes,
                })?;
            StackLimitAnalysis::Declared {
                reservation: id.to_string(),
                bytes: reservation.bytes,
                headroom_bytes,
            }
        }
        StackLimit::Undeclared => {
            add_gap(&mut gaps, StackAnalysisGapKind::StackLimitUndeclared, 1);
            StackLimitAnalysis::Undeclared
        }
    };
    let mut largest_frames = frames.frames.clone();
    largest_frames.truncate(LARGEST_FRAME_LIMIT);

    Ok(StackAnalysis {
        frame_source: input.frame_evidence,
        source_bytes: frames.section_bytes,
        frames: frames.frames,
        largest_frames,
        functions_without_frames: frames.missing_functions,
        roots: graph.roots,
        direct_calls: graph.edges,
        largest_known_path: graph.largest_path,
        limit,
        gaps: gaps
            .into_iter()
            .map(|(kind, occurrences)| StackAnalysisGap { kind, occurrences })
            .collect(),
    })
}

fn add_gap(
    gaps: &mut BTreeMap<StackAnalysisGapKind, u64>,
    kind: StackAnalysisGapKind,
    occurrences: u64,
) {
    if occurrences != 0 {
        *gaps.entry(kind).or_insert(0) += occurrences;
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum StackMetadataError {
    #[error("firmware ELF {path} has no {section} stack evidence")]
    MissingSection {
        path: PathBuf,
        section: &'static str,
    },
    #[error("firmware ELF {path} marks stack evidence section {section} as allocated memory")]
    AllocatedSection {
        path: PathBuf,
        section: &'static str,
    },
    #[error(
        "firmware ELF {path} places stack evidence section {section} inside a loadable segment"
    )]
    LoadableSection {
        path: PathBuf,
        section: &'static str,
    },
    #[error("stack metadata file range {offset} + {bytes} overflows")]
    SectionRangeOverflow { offset: u64, bytes: u64 },
    #[error("could not read stack evidence section {section} from {path}: {source}")]
    ReadSection {
        path: PathBuf,
        section: &'static str,
        #[source]
        source: object::Error,
    },
    #[error("stack evidence section {section} is empty")]
    EmptySection { section: &'static str },
    #[error(".debug_frame contains no supported stack-frame rules")]
    NoSupportedDwarfFrames,
    #[error(".stack_sizes has a truncated address at byte {offset}")]
    MalformedAddress { offset: usize },
    #[error(".stack_sizes has an invalid ULEB128 frame size at byte {offset}")]
    MalformedSize { offset: usize },
    #[error(".stack_sizes reports conflicting frames at {address:#x}: {first} and {second}")]
    ConflictingFrame {
        address: u64,
        first: u64,
        second: u64,
    },
    #[error("could not parse .debug_frame stack evidence: {0}")]
    Dwarf(#[from] gimli::Error),
    #[error("{evidence} count cannot be represented")]
    CountOverflow { evidence: &'static str },
    #[error("known call-path frame total overflowed")]
    CallPathOverflow,
    #[error("memory profile {profile:?} has no stack reservation {reservation:?}")]
    MissingReservation {
        profile: String,
        reservation: &'static str,
    },
    #[error(
        "known call-path lower bound {required} exceeds stack reservation {reservation:?} ({available} bytes)"
    )]
    KnownPathOverflow {
        reservation: &'static str,
        available: u64,
        required: u64,
    },
}
