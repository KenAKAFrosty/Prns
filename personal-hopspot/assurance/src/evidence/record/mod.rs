mod failure;
mod source;

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use thiserror::Error;

use crate::capabilities;
use crate::contract::{
    ArchitectureId, ComponentId, EvidenceArtifact, EvidenceFingerprint, EvidencePath,
    IdentifierError, MiriCoverage, PlatformId, PlatformMilestone, ProofArtifactKind,
    ProofContractError, ProofEvidence, ProofFragment, ProofKind, RunnerId, ScenarioId, Subject,
    SupportLevel, ToolIdentity, ToolKind, ValueError, Verdict, PROOF_FRAGMENT_SCHEMA_VERSION,
};

pub(crate) use failure::{record_failure, FailureCapability, FailureRecordRequest};

pub(crate) struct MiriRecordRequest {
    pub component: ComponentId,
    pub scenario: ScenarioId,
    pub runner: RunnerId,
    pub coverage: MiriCoverage,
    pub completed_tests: u32,
    pub rustc_version: String,
    pub miri_version: String,
    pub sources: Vec<PathBuf>,
    pub logs: Vec<PathBuf>,
    pub output: PathBuf,
}

pub(crate) struct TargetIsaRecordRequest {
    pub architecture: ArchitectureId,
    pub scenario: ScenarioId,
    pub runner: RunnerId,
    pub completed_scenarios: u32,
    pub cargo_version: String,
    pub rustc_version: String,
    pub qemu_version: String,
    pub qemu_executable: PathBuf,
    pub sources: Vec<PathBuf>,
    pub transcript: PathBuf,
    pub executable: PathBuf,
    pub logs: Vec<PathBuf>,
    pub output: PathBuf,
}

pub(crate) struct PlatformRecordRequest {
    pub platform: PlatformId,
    pub scenario: ScenarioId,
    pub runner: RunnerId,
    pub milestone: PlatformMilestone,
    pub cargo_version: String,
    pub rustc_version: String,
    pub linker_version: String,
    pub emulator: ToolKind,
    pub emulator_version: String,
    pub emulator_executable: PathBuf,
    pub sources: Vec<PathBuf>,
    pub transcript: PathBuf,
    pub executable: PathBuf,
    pub logs: Vec<PathBuf>,
    pub output: PathBuf,
}

#[derive(Debug, Error)]
pub(crate) enum RecordError {
    #[error("Miri proof completed no tests")]
    NoCompletedTests,
    #[error("target-ISA proof completed no scenarios")]
    NoCompletedScenarios,
    #[error("proof needs at least one source")]
    MissingSources,
    #[error("proof needs at least one log artifact")]
    MissingLogs,
    #[error("tool identity is empty for {0:?}")]
    EmptyToolIdentity(ToolKind),
    #[error(
        "{proof:?} proof is not a required or pilot capability for {subject} scenario {scenario}"
    )]
    Capability {
        subject: Subject,
        scenario: ScenarioId,
        proof: ProofKind,
    },
    #[error("proof output must end in .assurance.json: {0}")]
    OutputSuffix(PathBuf),
    #[error("artifact root is not a directory: {0}")]
    ArtifactRoot(PathBuf),
    #[error("log artifact is not a regular file: {0}")]
    LogFile(PathBuf),
    #[error("transcript artifact is not a regular file: {0}")]
    TranscriptFile(PathBuf),
    #[error("executable artifact is not a regular file: {0}")]
    ExecutableFile(PathBuf),
    #[error("QEMU executable is not a regular file: {0}")]
    QemuExecutable(PathBuf),
    #[error("platform emulator must be QEMU or Renode, found {0:?}")]
    PlatformEmulator(ToolKind),
    #[error("platform emulator executable is not a regular file: {0}")]
    PlatformEmulatorExecutable(PathBuf),
    #[error("evidence artifact {artifact} is outside artifact root {root}")]
    ArtifactOutsideRoot { artifact: PathBuf, root: PathBuf },
    #[error("could not inspect {path}: {source}")]
    Inspect {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not write proof {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Identifier(#[from] IdentifierError),
    #[error(transparent)]
    Value(#[from] ValueError),
    #[error(transparent)]
    Contract(#[from] ProofContractError),
    #[error(transparent)]
    Runner(#[from] capabilities::RunnerContractError),
    #[error(transparent)]
    Source(#[from] source::SourceError),
    #[error("could not serialize proof: {0}")]
    Serialize(#[from] serde_json::Error),
}

pub(crate) fn record_miri(
    repository_root: &Path,
    request: MiriRecordRequest,
) -> Result<(), RecordError> {
    validate_request(&request)?;
    let subject = Subject::Component(request.component.clone());
    let capability = validate_capability(&subject, &request.scenario, ProofKind::Miri)?;
    let source = source::identify(repository_root, &request.sources)?;
    let output_parent = request.output.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(output_parent).map_err(|source| RecordError::Write {
        path: output_parent.to_path_buf(),
        source,
    })?;
    let output_parent = artifact_root(output_parent)?;
    let artifacts = artifacts(&output_parent, &request.logs)?;
    let fragment = ProofFragment {
        schema_version: PROOF_FRAGMENT_SCHEMA_VERSION,
        subject,
        scenario: request.scenario,
        proof: ProofKind::Miri,
        runner: request.runner,
        source,
        tools: vec![
            ToolIdentity {
                kind: ToolKind::Rustc,
                version: request.rustc_version,
            },
            ToolIdentity {
                kind: ToolKind::Miri,
                version: request.miri_version,
            },
        ],
        verdict: Verdict::Passed {
            evidence: ProofEvidence::Miri {
                coverage: request.coverage,
                completed_tests: request.completed_tests,
            },
        },
        artifacts,
    };
    capabilities::validate_runner(&capability, &fragment)?;
    write_fragment(fragment, request.output)
}

pub(crate) fn record_target_isa(
    repository_root: &Path,
    request: TargetIsaRecordRequest,
) -> Result<(), RecordError> {
    validate_target_isa_request(&request)?;
    let subject = Subject::Architecture(request.architecture.clone());
    let capability = validate_capability(&subject, &request.scenario, ProofKind::TargetIsa)?;
    let source = source::identify(repository_root, &request.sources)?;
    let output_parent = request.output.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(output_parent).map_err(|source| RecordError::Write {
        path: output_parent.to_path_buf(),
        source,
    })?;
    let output_parent = artifact_root(output_parent)?;
    let transcript = artifact(
        &output_parent,
        &request.transcript,
        ProofArtifactKind::Transcript,
    )?;
    let transcript_fingerprint = transcript.fingerprint.clone();
    let executable = artifact(
        &output_parent,
        &request.executable,
        ProofArtifactKind::Executable,
    )?;
    let mut evidence_artifacts = vec![transcript, executable];
    evidence_artifacts.extend(artifacts(&output_parent, &request.logs)?);
    let qemu_fingerprint =
        executable_fingerprint(&request.qemu_executable, RecordError::QemuExecutable)?;
    let fragment = ProofFragment {
        schema_version: PROOF_FRAGMENT_SCHEMA_VERSION,
        subject,
        scenario: request.scenario,
        proof: ProofKind::TargetIsa,
        runner: request.runner,
        source,
        tools: vec![
            ToolIdentity {
                kind: ToolKind::Cargo,
                version: request.cargo_version,
            },
            ToolIdentity {
                kind: ToolKind::Rustc,
                version: request.rustc_version,
            },
            ToolIdentity {
                kind: ToolKind::Qemu,
                version: format!(
                    "{}; executable-sha256={}",
                    request.qemu_version,
                    qemu_fingerprint.as_str()
                ),
            },
        ],
        verdict: Verdict::Passed {
            evidence: ProofEvidence::TargetIsa {
                architecture: request.architecture,
                completed_scenarios: request.completed_scenarios,
                transcript_fingerprint,
            },
        },
        artifacts: evidence_artifacts,
    };
    capabilities::validate_runner(&capability, &fragment)?;
    write_fragment(fragment, request.output)
}

pub(crate) fn record_platform_emulation(
    repository_root: &Path,
    request: PlatformRecordRequest,
) -> Result<(), RecordError> {
    validate_platform_request(&request)?;
    let subject = Subject::Platform(request.platform.clone());
    let capability =
        validate_capability(&subject, &request.scenario, ProofKind::PlatformEmulation)?;
    let source = source::identify(repository_root, &request.sources)?;
    let output_parent = request.output.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(output_parent).map_err(|source| RecordError::Write {
        path: output_parent.to_path_buf(),
        source,
    })?;
    let output_parent = artifact_root(output_parent)?;
    let transcript = artifact(
        &output_parent,
        &request.transcript,
        ProofArtifactKind::Transcript,
    )?;
    let transcript_fingerprint = transcript.fingerprint.clone();
    let executable = artifact(
        &output_parent,
        &request.executable,
        ProofArtifactKind::Executable,
    )?;
    let mut evidence_artifacts = vec![transcript, executable];
    evidence_artifacts.extend(artifacts(&output_parent, &request.logs)?);
    let emulator_fingerprint = executable_fingerprint(
        &request.emulator_executable,
        RecordError::PlatformEmulatorExecutable,
    )?;
    let fragment = ProofFragment {
        schema_version: PROOF_FRAGMENT_SCHEMA_VERSION,
        subject,
        scenario: request.scenario,
        proof: ProofKind::PlatformEmulation,
        runner: request.runner,
        source,
        tools: vec![
            ToolIdentity {
                kind: ToolKind::Cargo,
                version: request.cargo_version,
            },
            ToolIdentity {
                kind: ToolKind::Rustc,
                version: request.rustc_version,
            },
            ToolIdentity {
                kind: ToolKind::Linker,
                version: request.linker_version,
            },
            ToolIdentity {
                kind: request.emulator,
                version: format!(
                    "{}; executable-sha256={}",
                    request.emulator_version,
                    emulator_fingerprint.as_str()
                ),
            },
        ],
        verdict: Verdict::Passed {
            evidence: ProofEvidence::PlatformEmulation {
                platform: request.platform,
                milestone: request.milestone,
                transcript_fingerprint,
            },
        },
        artifacts: evidence_artifacts,
    };
    capabilities::validate_runner(&capability, &fragment)?;
    write_fragment(fragment, request.output)
}

fn validate_request(request: &MiriRecordRequest) -> Result<(), RecordError> {
    if request.completed_tests == 0 {
        return Err(RecordError::NoCompletedTests);
    }
    validate_common(&request.sources, &request.logs, &request.output)?;
    for (kind, version) in [
        (ToolKind::Rustc, request.rustc_version.as_str()),
        (ToolKind::Miri, request.miri_version.as_str()),
    ] {
        if version.trim().is_empty() {
            return Err(RecordError::EmptyToolIdentity(kind));
        }
    }
    Ok(())
}

fn validate_target_isa_request(request: &TargetIsaRecordRequest) -> Result<(), RecordError> {
    if request.completed_scenarios == 0 {
        return Err(RecordError::NoCompletedScenarios);
    }
    validate_common(&request.sources, &request.logs, &request.output)?;
    for (kind, version) in [
        (ToolKind::Cargo, request.cargo_version.as_str()),
        (ToolKind::Rustc, request.rustc_version.as_str()),
        (ToolKind::Qemu, request.qemu_version.as_str()),
    ] {
        if version.trim().is_empty() {
            return Err(RecordError::EmptyToolIdentity(kind));
        }
    }
    Ok(())
}

fn validate_platform_request(request: &PlatformRecordRequest) -> Result<(), RecordError> {
    validate_common(&request.sources, &request.logs, &request.output)?;
    if !matches!(request.emulator, ToolKind::Qemu | ToolKind::Renode) {
        return Err(RecordError::PlatformEmulator(request.emulator));
    }
    for (kind, version) in [
        (ToolKind::Cargo, request.cargo_version.as_str()),
        (ToolKind::Rustc, request.rustc_version.as_str()),
        (ToolKind::Linker, request.linker_version.as_str()),
        (request.emulator, request.emulator_version.as_str()),
    ] {
        if version.trim().is_empty() {
            return Err(RecordError::EmptyToolIdentity(kind));
        }
    }
    Ok(())
}

fn validate_common(
    sources: &[PathBuf],
    logs: &[PathBuf],
    output: &Path,
) -> Result<(), RecordError> {
    if sources.is_empty() {
        return Err(RecordError::MissingSources);
    }
    if logs.is_empty() {
        return Err(RecordError::MissingLogs);
    }
    validate_output(output)
}

fn validate_output(output: &Path) -> Result<(), RecordError> {
    if !output.to_string_lossy().ends_with(".assurance.json") {
        return Err(RecordError::OutputSuffix(output.to_path_buf()));
    }
    Ok(())
}

fn validate_capability(
    subject: &Subject,
    scenario: &ScenarioId,
    proof: ProofKind,
) -> Result<crate::contract::Capability, RecordError> {
    let supported = capabilities::canonical()?.into_iter().find(|capability| {
        capability.subject == *subject
            && capability.scenario == *scenario
            && capability.proof == proof
            && matches!(
                capability.support,
                SupportLevel::Required | SupportLevel::Pilot
            )
    });
    supported.ok_or_else(|| RecordError::Capability {
        subject: subject.clone(),
        scenario: scenario.clone(),
        proof,
    })
}

fn artifacts(root: &Path, logs: &[PathBuf]) -> Result<Vec<EvidenceArtifact>, RecordError> {
    logs.iter()
        .map(|log| artifact(root, log, ProofArtifactKind::Log))
        .collect::<Result<Vec<_>, _>>()
}

fn artifact_root(root: &Path) -> Result<PathBuf, RecordError> {
    let root = fs::canonicalize(root).map_err(|_| RecordError::ArtifactRoot(root.to_path_buf()))?;
    if !root.is_dir() {
        return Err(RecordError::ArtifactRoot(root));
    }
    Ok(root)
}

fn artifact(
    root: &Path,
    path: &Path,
    kind: ProofArtifactKind,
) -> Result<EvidenceArtifact, RecordError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| RecordError::Inspect {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() == 0 {
        return Err(match kind {
            ProofArtifactKind::Executable => RecordError::ExecutableFile(path.to_path_buf()),
            ProofArtifactKind::Log => RecordError::LogFile(path.to_path_buf()),
            ProofArtifactKind::Transcript => RecordError::TranscriptFile(path.to_path_buf()),
        });
    }
    let path = fs::canonicalize(path).map_err(|source| RecordError::Inspect {
        path: path.to_path_buf(),
        source,
    })?;
    let relative = path
        .strip_prefix(root)
        .map_err(|_| RecordError::ArtifactOutsideRoot {
            artifact: path.clone(),
            root: root.to_path_buf(),
        })?;
    let bytes = fs::read(&path).map_err(|source| RecordError::Inspect {
        path: path.clone(),
        source,
    })?;
    Ok(EvidenceArtifact {
        kind,
        path: EvidencePath::parse(portable_path(relative))?,
        bytes: bytes.len() as u64,
        fingerprint: EvidenceFingerprint::parse(prns_flash_manifest::sha256_hex(&bytes))?,
    })
}

fn executable_fingerprint(
    path: &Path,
    invalid: impl FnOnce(PathBuf) -> RecordError,
) -> Result<EvidenceFingerprint, RecordError> {
    let path = fs::canonicalize(path).map_err(|source| RecordError::Inspect {
        path: path.to_path_buf(),
        source,
    })?;
    if !path.is_file() {
        return Err(invalid(path));
    }
    let bytes = fs::read(&path).map_err(|source| RecordError::Inspect {
        path: path.clone(),
        source,
    })?;
    EvidenceFingerprint::parse(prns_flash_manifest::sha256_hex(&bytes)).map_err(RecordError::from)
}

fn write_fragment(fragment: ProofFragment, output: PathBuf) -> Result<(), RecordError> {
    fragment.validate()?;
    let mut encoded = serde_json::to_vec_pretty(&fragment)?;
    encoded.push(b'\n');
    fs::write(&output, encoded).map_err(|source| RecordError::Write {
        path: output,
        source,
    })
}

fn portable_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy()),
            Component::CurDir => None,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests;
