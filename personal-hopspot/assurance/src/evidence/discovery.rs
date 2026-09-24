use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use personal_hopspot_resources::report::{ComparisonError, Document};
use thiserror::Error;

use crate::contract::{EvidenceArtifact, ProofContractError, ProofFragment};

const PROOF_SUFFIX: &str = ".assurance.json";

#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("evidence root is not a directory: {path}")]
    InvalidRoot { path: PathBuf },
    #[error("could not inspect evidence path {path}: {source}")]
    Inspect {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("evidence contains a symbolic link: {path}")]
    SymbolicLink { path: PathBuf },
    #[error("resource report directory contains a nested directory: {path}")]
    NestedResourceDirectory { path: PathBuf },
    #[error(transparent)]
    Resource(#[from] ComparisonError),
    #[error("could not parse assurance proof {path}: {source}")]
    ParseProof {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("assurance proof {path} violates its contract: {source}")]
    InvalidProof {
        path: PathBuf,
        #[source]
        source: ProofContractError,
    },
    #[error("could not inspect assurance artifact {artifact} referenced by {proof}: {source}")]
    InspectArtifact {
        proof: PathBuf,
        artifact: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("assurance artifact is not a regular file: {artifact} referenced by {proof}")]
    InvalidArtifact { proof: PathBuf, artifact: PathBuf },
    #[error("assurance artifact size differs for {artifact} referenced by {proof}: expected {expected}, found {actual}")]
    ArtifactSize {
        proof: PathBuf,
        artifact: PathBuf,
        expected: u64,
        actual: u64,
    },
    #[error("assurance artifact fingerprint differs for {artifact} referenced by {proof}")]
    ArtifactFingerprint { proof: PathBuf, artifact: PathBuf },
}

pub struct ResourceDocument {
    pub path: PathBuf,
    pub document: Document,
}

pub struct ProofDocument {
    pub path: PathBuf,
    pub fragment: ProofFragment,
}

pub fn discover_resources(root: &Path) -> Result<Vec<ResourceDocument>, DiscoveryError> {
    require_directory(root)?;
    let mut paths = Vec::new();
    if root.file_name().and_then(|name| name.to_str()) == Some("reports") {
        collect_report_directory(root, &mut paths)?;
    } else {
        collect_report_roots(root, &mut paths)?;
    }
    paths.sort();
    paths
        .into_iter()
        .map(|path| Document::load(&path).map(|document| ResourceDocument { path, document }))
        .collect::<Result<Vec<_>, _>>()
        .map_err(DiscoveryError::from)
}

pub fn discover_proofs(root: &Path) -> Result<Vec<ProofDocument>, DiscoveryError> {
    require_directory(root)?;
    let mut paths = Vec::new();
    collect_proof_paths(root, &mut paths)?;
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let bytes = fs::read(&path).map_err(|source| DiscoveryError::Inspect {
                path: path.clone(),
                source,
            })?;
            let fragment = serde_json::from_slice::<ProofFragment>(&bytes).map_err(|source| {
                DiscoveryError::ParseProof {
                    path: path.clone(),
                    source,
                }
            })?;
            fragment
                .validate()
                .map_err(|source| DiscoveryError::InvalidProof {
                    path: path.clone(),
                    source,
                })?;
            validate_artifacts(&path, &fragment.artifacts)?;
            Ok(ProofDocument { path, fragment })
        })
        .collect()
}

fn validate_artifacts(proof: &Path, artifacts: &[EvidenceArtifact]) -> Result<(), DiscoveryError> {
    let parent = proof.parent().unwrap_or(Path::new("."));
    for declared in artifacts {
        let artifact = parent.join(declared.path.as_str());
        let metadata =
            fs::symlink_metadata(&artifact).map_err(|source| DiscoveryError::InspectArtifact {
                proof: proof.to_path_buf(),
                artifact: artifact.clone(),
                source,
            })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(DiscoveryError::InvalidArtifact {
                proof: proof.to_path_buf(),
                artifact,
            });
        }
        if metadata.len() != declared.bytes {
            return Err(DiscoveryError::ArtifactSize {
                proof: proof.to_path_buf(),
                artifact,
                expected: declared.bytes,
                actual: metadata.len(),
            });
        }
        let bytes = fs::read(&artifact).map_err(|source| DiscoveryError::InspectArtifact {
            proof: proof.to_path_buf(),
            artifact: artifact.clone(),
            source,
        })?;
        if prns_flash_manifest::sha256_hex(&bytes) != declared.fingerprint.as_str() {
            return Err(DiscoveryError::ArtifactFingerprint {
                proof: proof.to_path_buf(),
                artifact,
            });
        }
    }
    Ok(())
}

fn require_directory(path: &Path) -> Result<(), DiscoveryError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| DiscoveryError::Inspect {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() {
        return Err(DiscoveryError::SymbolicLink {
            path: path.to_path_buf(),
        });
    }
    if metadata.is_dir() {
        Ok(())
    } else {
        Err(DiscoveryError::InvalidRoot {
            path: path.to_path_buf(),
        })
    }
}

fn collect_report_roots(
    directory: &Path,
    reports: &mut Vec<PathBuf>,
) -> Result<(), DiscoveryError> {
    for (path, kind) in entries(directory)? {
        if kind.is_symlink() {
            return Err(DiscoveryError::SymbolicLink { path });
        }
        if !kind.is_dir() {
            continue;
        }
        if path.file_name().and_then(|name| name.to_str()) == Some("reports") {
            collect_report_directory(&path, reports)?;
        } else {
            collect_report_roots(&path, reports)?;
        }
    }
    Ok(())
}

fn collect_report_directory(
    directory: &Path,
    reports: &mut Vec<PathBuf>,
) -> Result<(), DiscoveryError> {
    for (path, kind) in entries(directory)? {
        if kind.is_symlink() {
            return Err(DiscoveryError::SymbolicLink { path });
        }
        if kind.is_dir() {
            return Err(DiscoveryError::NestedResourceDirectory { path });
        }
        if kind.is_file() && path.extension().and_then(|value| value.to_str()) == Some("json") {
            reports.push(path);
        }
    }
    Ok(())
}

fn collect_proof_paths(directory: &Path, proofs: &mut Vec<PathBuf>) -> Result<(), DiscoveryError> {
    for (path, kind) in entries(directory)? {
        if kind.is_symlink() {
            return Err(DiscoveryError::SymbolicLink { path });
        }
        if kind.is_dir() {
            collect_proof_paths(&path, proofs)?;
        } else if kind.is_file()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(PROOF_SUFFIX))
        {
            proofs.push(path);
        }
    }
    Ok(())
}

fn entries(directory: &Path) -> Result<Vec<(PathBuf, fs::FileType)>, DiscoveryError> {
    let entries = fs::read_dir(directory).map_err(|source| DiscoveryError::Inspect {
        path: directory.to_path_buf(),
        source,
    })?;
    let mut values = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| DiscoveryError::Inspect {
            path: directory.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let kind = entry
            .file_type()
            .map_err(|source| DiscoveryError::Inspect {
                path: path.clone(),
                source,
            })?;
        values.push((path, kind));
    }
    values.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(values)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{discover_proofs, DiscoveryError};

    fn proof(log_fingerprint: &str) -> serde_json::Value {
        serde_json::json!({
            "schema_version": crate::contract::PROOF_FRAGMENT_SCHEMA_VERSION,
            "subject": { "kind": "component", "id": "sx126x" },
            "scenario": "sx126x-state-machine",
            "proof": "miri",
            "runner": "miri-stacked",
            "source": {
                "custody": { "kind": "clean-commit", "commit": "a".repeat(40) },
                "scenario_fingerprint": "b".repeat(64)
            },
            "tools": [
                { "kind": "rustc", "version": "rustc 1" },
                { "kind": "miri", "version": "miri 1" }
            ],
            "verdict": {
                "kind": "passed",
                "evidence": {
                    "kind": "miri",
                    "coverage": "stacked",
                    "completed_tests": 1
                }
            },
            "artifacts": [{
                "kind": "log",
                "path": "miri.log",
                "bytes": 4,
                "fingerprint": log_fingerprint
            }]
        })
    }

    #[test]
    fn malformed_proof_evidence_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        fs::write(directory.path().join("broken.assurance.json"), b"{")?;
        assert!(matches!(
            discover_proofs(directory.path()),
            Err(DiscoveryError::ParseProof { .. })
        ));
        Ok(())
    }

    #[test]
    fn unrelated_validation_json_is_not_a_proof() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        fs::write(directory.path().join("result.json"), b"{}")?;
        assert!(discover_proofs(directory.path())?.is_empty());
        Ok(())
    }

    #[test]
    fn proof_discovery_verifies_artifact_custody() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let log = b"Miri";
        fs::write(directory.path().join("miri.log"), log)?;
        fs::write(
            directory.path().join("valid.assurance.json"),
            serde_json::to_vec(&proof(&prns_flash_manifest::sha256_hex(log)))?,
        )?;
        assert_eq!(discover_proofs(directory.path())?.len(), 1);
        fs::write(directory.path().join("miri.log"), b"mori")?;
        assert!(matches!(
            discover_proofs(directory.path()),
            Err(DiscoveryError::ArtifactFingerprint { .. })
        ));
        Ok(())
    }
}
