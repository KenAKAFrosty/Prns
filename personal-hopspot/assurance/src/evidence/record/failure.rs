use std::fs;
use std::path::{Path, PathBuf};

use crate::capabilities;
use crate::contract::{
    ArchitectureId, ComponentId, Failure, FailureKind, PlatformId, ProofFragment, ProofKind,
    RunnerId, ScenarioId, Subject, ToolIdentity, ToolKind, Verdict, PROOF_FRAGMENT_SCHEMA_VERSION,
};

use super::{
    artifact_root, artifacts, executable_fingerprint, source, validate_capability, validate_common,
    write_fragment, RecordError,
};

pub(crate) enum FailureCapability {
    Miri {
        component: ComponentId,
        rustc_version: String,
        miri_version: String,
    },
    TargetIsa {
        architecture: ArchitectureId,
        cargo_version: String,
        rustc_version: String,
        qemu_version: String,
        qemu_executable: PathBuf,
    },
    Platform {
        platform: PlatformId,
        cargo_version: String,
        rustc_version: String,
        linker_version: String,
        emulator: ToolKind,
        emulator_version: String,
        emulator_executable: PathBuf,
    },
}

pub(crate) struct FailureRecordRequest {
    pub capability: FailureCapability,
    pub scenario: ScenarioId,
    pub runner: RunnerId,
    pub kind: FailureKind,
    pub diagnostic: String,
    pub sources: Vec<PathBuf>,
    pub logs: Vec<PathBuf>,
    pub output: PathBuf,
}

pub(crate) fn record_failure(
    repository_root: &Path,
    request: FailureRecordRequest,
) -> Result<(), RecordError> {
    validate_common(&request.sources, &request.logs, &request.output)?;
    let (subject, proof, tools) = request.capability.into_parts()?;
    let capability = validate_capability(&subject, &request.scenario, proof)?;
    let source = source::identify(repository_root, &request.sources)?;
    let output_parent = request.output.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(output_parent).map_err(|source| RecordError::Write {
        path: output_parent.to_path_buf(),
        source,
    })?;
    let artifacts = artifacts(&artifact_root(output_parent)?, &request.logs)?;
    let fragment = ProofFragment {
        schema_version: PROOF_FRAGMENT_SCHEMA_VERSION,
        subject,
        scenario: request.scenario,
        proof,
        runner: request.runner,
        source,
        tools,
        verdict: Verdict::Failed {
            failure: Failure {
                kind: request.kind,
                diagnostic: request.diagnostic,
            },
        },
        artifacts,
    };
    capabilities::validate_runner(&capability, &fragment)?;
    write_fragment(fragment, request.output)
}

impl FailureCapability {
    fn into_parts(self) -> Result<(Subject, ProofKind, Vec<ToolIdentity>), RecordError> {
        match self {
            Self::Miri {
                component,
                rustc_version,
                miri_version,
            } => Ok((
                Subject::Component(component),
                ProofKind::Miri,
                vec![
                    tool(ToolKind::Rustc, rustc_version)?,
                    tool(ToolKind::Miri, miri_version)?,
                ],
            )),
            Self::TargetIsa {
                architecture,
                cargo_version,
                rustc_version,
                qemu_version,
                qemu_executable,
            } => {
                let qemu_fingerprint =
                    executable_fingerprint(&qemu_executable, RecordError::QemuExecutable)?;
                Ok((
                    Subject::Architecture(architecture),
                    ProofKind::TargetIsa,
                    vec![
                        tool(ToolKind::Cargo, cargo_version)?,
                        tool(ToolKind::Rustc, rustc_version)?,
                        tool(
                            ToolKind::Qemu,
                            format!(
                                "{qemu_version}; executable-sha256={}",
                                qemu_fingerprint.as_str()
                            ),
                        )?,
                    ],
                ))
            }
            Self::Platform {
                platform,
                cargo_version,
                rustc_version,
                linker_version,
                emulator,
                emulator_version,
                emulator_executable,
            } => {
                if !matches!(emulator, ToolKind::Qemu | ToolKind::Renode) {
                    return Err(RecordError::PlatformEmulator(emulator));
                }
                let emulator_fingerprint = executable_fingerprint(
                    &emulator_executable,
                    RecordError::PlatformEmulatorExecutable,
                )?;
                Ok((
                    Subject::Platform(platform),
                    ProofKind::PlatformEmulation,
                    vec![
                        tool(ToolKind::Cargo, cargo_version)?,
                        tool(ToolKind::Rustc, rustc_version)?,
                        tool(ToolKind::Linker, linker_version)?,
                        tool(
                            emulator,
                            format!(
                                "{emulator_version}; executable-sha256={}",
                                emulator_fingerprint.as_str()
                            ),
                        )?,
                    ],
                ))
            }
        }
    }
}

fn tool(kind: ToolKind, version: String) -> Result<ToolIdentity, RecordError> {
    if version.trim().is_empty() {
        Err(RecordError::EmptyToolIdentity(kind))
    } else {
        Ok(ToolIdentity { kind, version })
    }
}
