use std::path::Path;

use personal_hopspot_builder::architecture::DisassemblerFlavor;
use personal_hopspot_builder::architecture::StackFrameEvidence;
use personal_hopspot_builder::artifact::publish;
use personal_hopspot_builder::{BuildContext, BuildError};
use personal_hopspot_memory::ProcessorArchitecture;
use serde::Serialize;
use thiserror::Error;

use crate::analysis;

use super::fingerprint::{fingerprint, Fingerprint};
use super::model::{
    ByteOrderIdentity, DisassemblerFlavorIdentity, DisassemblyIdentity, Evidence,
    EvidenceArtifactIdentity, ExecutableArchitectureIdentity, ExecutableIdentity,
    ExecutableSectionIdentity, FunctionAnalysisIdentity, FunctionBoundaryIdentity,
    FunctionNormalizationIdentity, KnownCallPathFrameIdentity, KnownCallPathIdentity,
    LoadPermissionIdentity, LoadSegmentIdentity, StackAnalysisGapIdentity,
    StackAnalysisGapKindIdentity, StackAnalysisIdentity, StackFrameIdentity,
    StackFrameSourceIdentity, StackLimitIdentity, StackRootIdentity, StackRootRoleIdentity,
    StartupAnchorIdentity, StartupAnchorRoleIdentity, StartupStructureIdentity,
};

const FUNCTION_BOUNDARY_SCHEMA_VERSION: u32 = 1;
const STACK_EVIDENCE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Error)]
pub(crate) enum ExecutableReportError {
    #[error("could not validate generated executable fingerprint: {0}")]
    Fingerprint(#[from] prns_flash_manifest::DomainValueError),
    #[error("{evidence} cannot be represented in the report schema")]
    CountOverflow { evidence: &'static str },
    #[error("could not serialize executable evidence: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("could not publish executable evidence: {0}")]
    Publish(#[from] BuildError),
}

pub(super) fn identity(
    context: &BuildContext<'_>,
    target_id: &str,
    rust_target: &str,
    analysis: analysis::ExecutableAnalysis,
) -> Result<ExecutableIdentity, ExecutableReportError> {
    let stack = stack_analysis_identity(context, target_id, analysis.stack)?;
    Ok(ExecutableIdentity {
        rust_target: rust_target.to_string(),
        architecture: architecture_identity(analysis.architecture),
        byte_order: match analysis.byte_order {
            analysis::executable::ByteOrder::Little => ByteOrderIdentity::Little,
        },
        entry_point: analysis.entry_point,
        load_segments: analysis
            .load_segments
            .into_iter()
            .map(|segment| LoadSegmentIdentity {
                file_offset: segment.file_offset,
                run_address: segment.run_range.start(),
                run_end: segment.run_range.end(),
                load_address: segment.load_range.start(),
                load_end: segment.load_range.end(),
                file_bytes: segment.file_bytes,
                memory_bytes: segment.memory_bytes,
                alignment: segment.alignment,
                permissions: segment
                    .permissions
                    .into_iter()
                    .map(|permission| match permission {
                        analysis::executable::LoadPermission::Read => LoadPermissionIdentity::Read,
                        analysis::executable::LoadPermission::Write => {
                            LoadPermissionIdentity::Write
                        }
                        analysis::executable::LoadPermission::Execute => {
                            LoadPermissionIdentity::Execute
                        }
                    })
                    .collect(),
            })
            .collect(),
        executable_sections: analysis
            .executable_sections
            .into_iter()
            .map(|section| {
                Ok(ExecutableSectionIdentity {
                    name: section.name,
                    address: section.range.start(),
                    end: section.range.end(),
                    bytes: section.range.byte_len(),
                    alignment: section.alignment,
                    fingerprint: Fingerprint::parse(section.fingerprint)?,
                })
            })
            .collect::<Result<Vec<_>, ExecutableReportError>>()?,
        startup: StartupStructureIdentity {
            entry_section: analysis.startup.entry_section,
            entry_symbol: analysis.startup.entry_symbol,
            anchors: analysis
                .startup
                .anchors
                .into_iter()
                .map(|anchor| StartupAnchorIdentity {
                    role: match anchor.role {
                        analysis::executable::StartupAnchorRole::EntryPoint => {
                            StartupAnchorRoleIdentity::EntryPoint
                        }
                        analysis::executable::StartupAnchorRole::InitialStackPointer => {
                            StartupAnchorRoleIdentity::InitialStackPointer
                        }
                        analysis::executable::StartupAnchorRole::ResetVector => {
                            StartupAnchorRoleIdentity::ResetVector
                        }
                        analysis::executable::StartupAnchorRole::TrapVector => {
                            StartupAnchorRoleIdentity::TrapVector
                        }
                        analysis::executable::StartupAnchorRole::ExceptionVectors => {
                            StartupAnchorRoleIdentity::ExceptionVectors
                        }
                    },
                    address: anchor.address,
                    section: anchor.section,
                })
                .collect(),
        },
        functions: function_analysis_identity(context, target_id, analysis.functions)?,
        disassembly: DisassemblyIdentity {
            adapter: architecture_identity(analysis.disassembly.adapter),
            flavor: disassembler_flavor_identity(analysis.disassembly.flavor),
            program: analysis.disassembly.program.to_string(),
            version: analysis.disassembly.version,
            executable_bytes: analysis.disassembly.executable_bytes,
            decoded_bytes: analysis.disassembly.decoded_bytes,
            undecoded_bytes: analysis.disassembly.undecoded_bytes,
            instruction_count: analysis.disassembly.instruction_count,
        },
        stack,
    })
}

fn stack_analysis_identity(
    context: &BuildContext<'_>,
    target_id: &str,
    analysis: analysis::executable::StackAnalysis,
) -> Result<Evidence<StackAnalysisIdentity>, ExecutableReportError> {
    let frames = analysis
        .frames
        .into_iter()
        .map(stack_frame_identity)
        .collect::<Vec<_>>();
    let largest_frames = analysis
        .largest_frames
        .into_iter()
        .map(stack_frame_identity)
        .collect::<Vec<_>>();
    let roots = analysis
        .roots
        .into_iter()
        .map(stack_root_identity)
        .collect::<Vec<_>>();
    let direct_calls = analysis
        .direct_calls
        .into_iter()
        .map(|edge| CallEdgeArtifactIdentity {
            caller: edge.caller,
            caller_address: edge.caller_address,
            callee: edge.callee,
            callee_address: edge.callee_address,
            call_site: edge.call_site,
        })
        .collect::<Vec<_>>();
    let largest_known_path = known_call_path_identity(analysis.largest_known_path);
    let limit = match analysis.limit {
        analysis::executable::StackLimitAnalysis::Declared {
            reservation,
            bytes,
            headroom_bytes,
        } => StackLimitIdentity::Declared {
            reservation,
            bytes,
            headroom_bytes,
        },
        analysis::executable::StackLimitAnalysis::Undeclared => StackLimitIdentity::Undeclared,
    };
    let gaps = analysis
        .gaps
        .into_iter()
        .map(|gap| StackAnalysisGapIdentity {
            kind: stack_gap_identity(gap.kind),
            occurrences: gap.occurrences,
        })
        .collect::<Vec<_>>();
    let relative_path = Path::new("work")
        .join(target_id)
        .join("stack-evidence.json");
    let artifact = StackEvidenceArtifact {
        schema_version: STACK_EVIDENCE_SCHEMA_VERSION,
        target: target_id,
        frame_source: stack_frame_source_identity(analysis.frame_source),
        frames: &frames,
        roots: &roots,
        direct_calls: &direct_calls,
        largest_known_path: &largest_known_path,
        limit: &limit,
        gaps: &gaps,
    };
    let mut bytes = serde_json::to_vec_pretty(&artifact)?;
    bytes.push(b'\n');
    publish(
        &context.configured_output_root().join(&relative_path),
        &bytes,
    )?;
    let identity =
        StackAnalysisIdentity {
            frame_source: stack_frame_source_identity(analysis.frame_source),
            source_bytes: analysis.source_bytes,
            frame_count: frames.len().try_into().map_err(|_| {
                ExecutableReportError::CountOverflow {
                    evidence: "stack frame count",
                }
            })?,
            functions_without_frames: analysis.functions_without_frames,
            largest_frames,
            roots,
            direct_call_count: direct_calls.len().try_into().map_err(|_| {
                ExecutableReportError::CountOverflow {
                    evidence: "direct call count",
                }
            })?,
            largest_known_path,
            limit,
            gaps,
            artifact: EvidenceArtifactIdentity {
                path: relative_path.to_string_lossy().into_owned(),
                bytes: bytes.len().try_into().map_err(|_| {
                    ExecutableReportError::CountOverflow {
                        evidence: "stack evidence artifact bytes",
                    }
                })?,
                fingerprint: Fingerprint::parse(prns_flash_manifest::sha256_hex(&bytes))?,
            },
        };
    if identity.gaps.is_empty() {
        Ok(Evidence::Complete(identity))
    } else {
        Ok(Evidence::Partial(identity))
    }
}

fn stack_frame_identity(frame: analysis::executable::StackFrame) -> StackFrameIdentity {
    StackFrameIdentity {
        name: frame.name,
        address: frame.address,
        bytes: frame.bytes,
    }
}

fn stack_root_identity(root: analysis::executable::StackRoot) -> StackRootIdentity {
    StackRootIdentity {
        role: match root.role {
            analysis::executable::StackRootRole::Startup => StackRootRoleIdentity::Startup,
            analysis::executable::StackRootRole::Trap => StackRootRoleIdentity::Trap,
            analysis::executable::StackRootRole::EmbassyTask => StackRootRoleIdentity::EmbassyTask,
        },
        name: root.name,
        address: root.address,
    }
}

fn known_call_path_identity(path: analysis::executable::KnownCallPath) -> KnownCallPathIdentity {
    KnownCallPathIdentity {
        bytes: path.bytes,
        frames: path
            .frames
            .into_iter()
            .map(|frame| KnownCallPathFrameIdentity {
                name: frame.name,
                address: frame.address,
                frame_bytes: frame.frame_bytes,
            })
            .collect(),
    }
}

const fn stack_gap_identity(
    gap: analysis::executable::StackAnalysisGapKind,
) -> StackAnalysisGapKindIdentity {
    match gap {
        analysis::executable::StackAnalysisGapKind::FunctionsWithoutFrameEvidence => {
            StackAnalysisGapKindIdentity::FunctionsWithoutFrameEvidence
        }
        analysis::executable::StackAnalysisGapKind::ForeignOrAssemblyFrames => {
            StackAnalysisGapKindIdentity::ForeignOrAssemblyFrames
        }
        analysis::executable::StackAnalysisGapKind::FrameEvidenceWithoutFunction => {
            StackAnalysisGapKindIdentity::FrameEvidenceWithoutFunction
        }
        analysis::executable::StackAnalysisGapKind::UnsupportedCfaRule => {
            StackAnalysisGapKindIdentity::UnsupportedCfaRule
        }
        analysis::executable::StackAnalysisGapKind::CallSiteOutsideFunction => {
            StackAnalysisGapKindIdentity::CallSiteOutsideFunction
        }
        analysis::executable::StackAnalysisGapKind::DirectCallOutsideFunctions => {
            StackAnalysisGapKindIdentity::DirectCallOutsideFunctions
        }
        analysis::executable::StackAnalysisGapKind::UnresolvedDirectCall => {
            StackAnalysisGapKindIdentity::UnresolvedDirectCall
        }
        analysis::executable::StackAnalysisGapKind::IndirectCall => {
            StackAnalysisGapKindIdentity::IndirectCall
        }
        analysis::executable::StackAnalysisGapKind::RecursiveCallCycle => {
            StackAnalysisGapKindIdentity::RecursiveCallCycle
        }
        analysis::executable::StackAnalysisGapKind::RootOutsideFunctions => {
            StackAnalysisGapKindIdentity::RootOutsideFunctions
        }
        analysis::executable::StackAnalysisGapKind::InterruptRootsUnresolved => {
            StackAnalysisGapKindIdentity::InterruptRootsUnresolved
        }
        analysis::executable::StackAnalysisGapKind::DynamicAllocationNotProvenAbsent => {
            StackAnalysisGapKindIdentity::DynamicAllocationNotProvenAbsent
        }
        analysis::executable::StackAnalysisGapKind::InterruptNestingUnmodeled => {
            StackAnalysisGapKindIdentity::InterruptNestingUnmodeled
        }
        analysis::executable::StackAnalysisGapKind::StackLimitUndeclared => {
            StackAnalysisGapKindIdentity::StackLimitUndeclared
        }
    }
}

const fn stack_frame_source_identity(source: StackFrameEvidence) -> StackFrameSourceIdentity {
    match source {
        StackFrameEvidence::LlvmStackSizes => StackFrameSourceIdentity::LlvmStackSizes,
        StackFrameEvidence::DwarfDebugFrame => StackFrameSourceIdentity::DwarfDebugFrame,
    }
}

#[derive(Serialize)]
struct StackEvidenceArtifact<'a> {
    schema_version: u32,
    target: &'a str,
    frame_source: StackFrameSourceIdentity,
    frames: &'a [StackFrameIdentity],
    roots: &'a [StackRootIdentity],
    direct_calls: &'a [CallEdgeArtifactIdentity],
    largest_known_path: &'a KnownCallPathIdentity,
    limit: &'a StackLimitIdentity,
    gaps: &'a [StackAnalysisGapIdentity],
}

#[derive(Serialize)]
struct CallEdgeArtifactIdentity {
    caller: String,
    caller_address: u64,
    callee: String,
    callee_address: u64,
    call_site: u64,
}

pub(super) const fn architecture_identity(
    architecture: ProcessorArchitecture,
) -> ExecutableArchitectureIdentity {
    match architecture {
        ProcessorArchitecture::ThumbV7em => ExecutableArchitectureIdentity::Thumbv7em,
        ProcessorArchitecture::RiscV32Imac => ExecutableArchitectureIdentity::Riscv32imac,
        ProcessorArchitecture::XtensaEsp32S3 => ExecutableArchitectureIdentity::XtensaEsp32s3,
    }
}

const fn disassembler_flavor_identity(flavor: DisassemblerFlavor) -> DisassemblerFlavorIdentity {
    match flavor {
        DisassemblerFlavor::LlvmObjdump => DisassemblerFlavorIdentity::LlvmObjdump,
        DisassemblerFlavor::GnuObjdump => DisassemblerFlavorIdentity::GnuObjdump,
    }
}

fn function_analysis_identity(
    context: &BuildContext<'_>,
    target_id: &str,
    analysis: analysis::executable::FunctionAnalysis,
) -> Result<FunctionAnalysisIdentity, ExecutableReportError> {
    let boundaries = function_identities(analysis.boundaries)?;
    let boundary_count =
        u64::try_from(boundaries.len()).map_err(|_| ExecutableReportError::CountOverflow {
            evidence: "function boundary count",
        })?;
    let boundaries_fingerprint = fingerprint(&boundaries)?;
    let relative_path = Path::new("work")
        .join(target_id)
        .join("function-boundaries.json");
    let artifact = FunctionBoundaryArtifact {
        schema_version: FUNCTION_BOUNDARY_SCHEMA_VERSION,
        target: target_id,
        normalization: FunctionNormalizationIdentity::LinkedFunctionBodySha256V1,
        boundaries: &boundaries,
    };
    let mut bytes = serde_json::to_vec_pretty(&artifact)?;
    bytes.push(b'\n');
    publish(
        &context.configured_output_root().join(&relative_path),
        &bytes,
    )?;
    Ok(FunctionAnalysisIdentity {
        normalization: FunctionNormalizationIdentity::LinkedFunctionBodySha256V1,
        boundary_count,
        boundaries_fingerprint,
        boundaries_artifact: EvidenceArtifactIdentity {
            path: relative_path.to_string_lossy().into_owned(),
            bytes: u64::try_from(bytes.len()).map_err(|_| {
                ExecutableReportError::CountOverflow {
                    evidence: "function boundary artifact size",
                }
            })?,
            fingerprint: Fingerprint::parse(prns_flash_manifest::sha256_hex(&bytes))?,
        },
        classified_bytes: analysis.classified_bytes,
        unclassified_bytes: analysis.unclassified_bytes,
        largest: function_identities(analysis.largest)?,
    })
}

#[derive(Serialize)]
struct FunctionBoundaryArtifact<'a> {
    schema_version: u32,
    target: &'a str,
    normalization: FunctionNormalizationIdentity,
    boundaries: &'a [FunctionBoundaryIdentity],
}

fn function_identities(
    boundaries: Vec<analysis::executable::FunctionBoundary>,
) -> Result<Vec<FunctionBoundaryIdentity>, ExecutableReportError> {
    boundaries
        .into_iter()
        .map(|boundary| {
            Ok(FunctionBoundaryIdentity {
                name: boundary.name,
                address: boundary.range.start(),
                end: boundary.range.end(),
                bytes: boundary.range.byte_len(),
                fingerprint: Fingerprint::parse(boundary.fingerprint)?,
            })
        })
        .collect()
}
