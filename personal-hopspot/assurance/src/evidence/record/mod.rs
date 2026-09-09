mod source;

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use thiserror::Error;

use crate::capabilities;
use crate::contract::{
    ComponentId, EvidenceArtifact, EvidenceFingerprint, EvidencePath, IdentifierError,
    MiriCoverage, ProofArtifactKind, ProofContractError, ProofEvidence, ProofFragment, ProofKind,
    RunnerId, ScenarioId, Subject, SupportLevel, ToolIdentity, ToolKind, ValueError, Verdict,
    PROOF_FRAGMENT_SCHEMA_VERSION,
};

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

#[derive(Debug, Error)]
pub(crate) enum RecordError {
    #[error("Miri proof completed no tests")]
    NoCompletedTests,
    #[error("Miri proof needs at least one source")]
    MissingSources,
    #[error("Miri proof needs at least one log artifact")]
    MissingLogs,
    #[error("tool identity is empty for {0:?}")]
    EmptyToolIdentity(ToolKind),
    #[error("Miri proof is not a required or pilot capability for component {component} scenario {scenario}")]
    Capability {
        component: ComponentId,
        scenario: ScenarioId,
    },
    #[error("proof output must end in .assurance.json: {0}")]
    OutputSuffix(PathBuf),
    #[error("artifact root is not a directory: {0}")]
    ArtifactRoot(PathBuf),
    #[error("log artifact is not a regular file: {0}")]
    LogFile(PathBuf),
    #[error("log artifact {log} is outside artifact root {root}")]
    LogOutsideRoot { log: PathBuf, root: PathBuf },
    #[error("could not inspect {path}: {source}")]
    Inspect {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not write Miri proof {path}: {source}")]
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
    Source(#[from] source::SourceError),
    #[error("could not serialize Miri proof: {0}")]
    Serialize(#[from] serde_json::Error),
}

pub(crate) fn record_miri(
    repository_root: &Path,
    request: MiriRecordRequest,
) -> Result<(), RecordError> {
    validate_request(&request)?;
    let subject = Subject::Component(request.component.clone());
    validate_capability(&request.component, &request.scenario)?;
    let source = source::identify(repository_root, &request.sources)?;
    let output_parent = request.output.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(output_parent).map_err(|source| RecordError::Write {
        path: output_parent.to_path_buf(),
        source,
    })?;
    let artifacts = artifacts(output_parent, &request.logs)?;
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
    fragment.validate()?;
    let mut encoded = serde_json::to_vec_pretty(&fragment)?;
    encoded.push(b'\n');
    fs::write(&request.output, encoded).map_err(|source| RecordError::Write {
        path: request.output,
        source,
    })
}

fn validate_request(request: &MiriRecordRequest) -> Result<(), RecordError> {
    if request.completed_tests == 0 {
        return Err(RecordError::NoCompletedTests);
    }
    if request.sources.is_empty() {
        return Err(RecordError::MissingSources);
    }
    if request.logs.is_empty() {
        return Err(RecordError::MissingLogs);
    }
    for (kind, version) in [
        (ToolKind::Rustc, request.rustc_version.as_str()),
        (ToolKind::Miri, request.miri_version.as_str()),
    ] {
        if version.trim().is_empty() {
            return Err(RecordError::EmptyToolIdentity(kind));
        }
    }
    let output = request.output.to_string_lossy();
    if !output.ends_with(".assurance.json") {
        return Err(RecordError::OutputSuffix(request.output.clone()));
    }
    Ok(())
}

fn validate_capability(component: &ComponentId, scenario: &ScenarioId) -> Result<(), RecordError> {
    let supported = capabilities::canonical()?.into_iter().any(|capability| {
        capability.subject == Subject::Component(component.clone())
            && capability.scenario == *scenario
            && capability.proof == ProofKind::Miri
            && matches!(
                capability.support,
                SupportLevel::Required | SupportLevel::Pilot
            )
    });
    if supported {
        Ok(())
    } else {
        Err(RecordError::Capability {
            component: component.clone(),
            scenario: scenario.clone(),
        })
    }
}

fn artifacts(root: &Path, logs: &[PathBuf]) -> Result<Vec<EvidenceArtifact>, RecordError> {
    let root = fs::canonicalize(root).map_err(|_| RecordError::ArtifactRoot(root.to_path_buf()))?;
    if !root.is_dir() {
        return Err(RecordError::ArtifactRoot(root));
    }
    logs.iter()
        .map(|log| artifact(&root, log))
        .collect::<Result<Vec<_>, _>>()
}

fn artifact(root: &Path, log: &Path) -> Result<EvidenceArtifact, RecordError> {
    let metadata = fs::symlink_metadata(log).map_err(|source| RecordError::Inspect {
        path: log.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() == 0 {
        return Err(RecordError::LogFile(log.to_path_buf()));
    }
    let log = fs::canonicalize(log).map_err(|source| RecordError::Inspect {
        path: log.to_path_buf(),
        source,
    })?;
    let relative = log
        .strip_prefix(root)
        .map_err(|_| RecordError::LogOutsideRoot {
            log: log.clone(),
            root: root.to_path_buf(),
        })?;
    let bytes = fs::read(&log).map_err(|source| RecordError::Inspect {
        path: log.clone(),
        source,
    })?;
    Ok(EvidenceArtifact {
        kind: ProofArtifactKind::Log,
        path: EvidencePath::parse(portable_path(relative))?,
        bytes: bytes.len() as u64,
        fingerprint: EvidenceFingerprint::parse(prns_flash_manifest::sha256_hex(&bytes))?,
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
