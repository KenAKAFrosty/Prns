use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use personal_hopspot_resources::report::{ComparisonError, Document};
use thiserror::Error;

use crate::contract::{ProofContractError, ProofFragment};

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
            Ok(ProofDocument { path, fragment })
        })
        .collect()
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
}
