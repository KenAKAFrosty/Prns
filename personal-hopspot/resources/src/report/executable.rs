use std::path::Path;

use personal_hopspot_builder::architecture::DisassemblerFlavor;
use personal_hopspot_builder::artifact::publish;
use personal_hopspot_builder::{BuildContext, BuildError};
use personal_hopspot_memory::ProcessorArchitecture;
use serde::Serialize;
use thiserror::Error;

use crate::analysis;

use super::fingerprint::{fingerprint, Fingerprint};
use super::model::{
    ByteOrderIdentity, DisassemblerFlavorIdentity, DisassemblyIdentity, EvidenceArtifactIdentity,
    ExecutableArchitectureIdentity, ExecutableIdentity, ExecutableSectionIdentity,
    FunctionAnalysisIdentity, FunctionBoundaryIdentity, FunctionNormalizationIdentity,
    LoadPermissionIdentity, LoadSegmentIdentity, StartupAnchorIdentity, StartupAnchorRoleIdentity,
    StartupStructureIdentity,
};

const FUNCTION_BOUNDARY_SCHEMA_VERSION: u32 = 1;

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
    })
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
