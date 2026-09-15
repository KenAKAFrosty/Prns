use std::collections::btree_map::Entry;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use thiserror::Error;

use crate::contract::{EvidenceFingerprint, SourceIdentity, ValueError};

const SCENARIO_FINGERPRINT_SCHEMA: u32 = 1;

#[derive(Debug, Error)]
pub(crate) enum SourceError {
    #[error("scenario source does not exist: {0}")]
    Missing(PathBuf),
    #[error("scenario source is a symbolic link: {0}")]
    SymbolicLink(PathBuf),
    #[error("scenario source is outside repository root: {0}")]
    OutsideRepository(PathBuf),
    #[error("scenario source is not a regular file or directory: {0}")]
    UnsupportedKind(PathBuf),
    #[error("scenario source is repeated: {0}")]
    Duplicate(PathBuf),
    #[error("source changed while the proof identity was being recorded")]
    Changed,
    #[error("could not inspect scenario source {path}: {source}")]
    Inspect {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Custody(#[from] personal_hopspot_builder::SourceCaptureError),
    #[error("could not serialize source identity: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error(transparent)]
    Value(#[from] ValueError),
}

#[derive(Serialize)]
struct FingerprintDocument<'a> {
    schema_version: u32,
    files: Vec<FileIdentity<'a>>,
}

#[derive(Serialize)]
struct FileIdentity<'a> {
    path: &'a str,
    fingerprint: &'a str,
}

pub(super) fn identify(
    repository_root: &Path,
    requested_sources: &[PathBuf],
) -> Result<SourceIdentity, SourceError> {
    let repository_root =
        fs::canonicalize(repository_root).map_err(|source| SourceError::Inspect {
            path: repository_root.to_path_buf(),
            source,
        })?;
    let custody = personal_hopspot_builder::capture_source_custody(&repository_root)?;
    let files = collect_sources(&repository_root, requested_sources)?;
    let scenario_fingerprint = fingerprint_files(&files)?;
    if personal_hopspot_builder::capture_source_custody(&repository_root)? != custody {
        return Err(SourceError::Changed);
    }
    Ok(SourceIdentity {
        custody,
        scenario_fingerprint,
    })
}

fn collect_sources(
    repository_root: &Path,
    requested_sources: &[PathBuf],
) -> Result<BTreeMap<String, String>, SourceError> {
    let mut files = BTreeMap::new();
    for requested in requested_sources {
        let absolute = if requested.is_absolute() {
            requested.clone()
        } else {
            repository_root.join(requested)
        };
        collect_path(repository_root, &absolute, &mut files)?;
    }
    Ok(files)
}

fn collect_path(
    repository_root: &Path,
    path: &Path,
    files: &mut BTreeMap<String, String>,
) -> Result<(), SourceError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(SourceError::Missing(path.to_path_buf()));
        }
        Err(source) => {
            return Err(SourceError::Inspect {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(SourceError::SymbolicLink(path.to_path_buf()));
    }
    let canonical = fs::canonicalize(path).map_err(|source| SourceError::Inspect {
        path: path.to_path_buf(),
        source,
    })?;
    if !canonical.starts_with(repository_root) {
        return Err(SourceError::OutsideRepository(canonical));
    }
    if metadata.is_dir() {
        let mut entries = fs::read_dir(&canonical)
            .map_err(|source| SourceError::Inspect {
                path: canonical.clone(),
                source,
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| SourceError::Inspect {
                path: canonical.clone(),
                source,
            })?;
        entries.sort_by_key(std::fs::DirEntry::path);
        for entry in entries {
            collect_path(repository_root, &entry.path(), files)?;
        }
        return Ok(());
    }
    if !metadata.is_file() {
        return Err(SourceError::UnsupportedKind(canonical));
    }
    let relative = canonical
        .strip_prefix(repository_root)
        .map_err(|_| SourceError::OutsideRepository(canonical.clone()))?;
    let key = portable_path(relative);
    let bytes = fs::read(&canonical).map_err(|source| SourceError::Inspect {
        path: canonical.clone(),
        source,
    })?;
    match files.entry(key) {
        Entry::Vacant(entry) => {
            entry.insert(prns_flash_manifest::sha256_hex(&bytes));
            Ok(())
        }
        Entry::Occupied(entry) => Err(SourceError::Duplicate(PathBuf::from(entry.key()))),
    }
}

fn fingerprint_files(files: &BTreeMap<String, String>) -> Result<EvidenceFingerprint, SourceError> {
    let document = FingerprintDocument {
        schema_version: SCENARIO_FINGERPRINT_SCHEMA,
        files: files
            .iter()
            .map(|(path, fingerprint)| FileIdentity { path, fingerprint })
            .collect(),
    };
    let encoded = serde_json::to_vec(&document)?;
    EvidenceFingerprint::parse(prns_flash_manifest::sha256_hex(&encoded)).map_err(SourceError::from)
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
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{collect_sources, fingerprint_files, SourceError};

    #[test]
    fn scenario_fingerprint_is_order_independent_and_content_sensitive(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let repository = tempdir()?;
        fs::create_dir(repository.path().join("scenario"))?;
        fs::write(repository.path().join("scenario/a.rs"), b"a")?;
        fs::write(repository.path().join("scenario/b.rs"), b"b")?;
        let repository_root = repository.path().canonicalize()?;
        let first = collect_sources(
            &repository_root,
            &["scenario/a.rs".into(), "scenario/b.rs".into()],
        )?;
        let second = collect_sources(&repository_root, &["scenario".into()])?;
        assert_eq!(fingerprint_files(&first)?, fingerprint_files(&second)?);
        fs::write(repository.path().join("scenario/b.rs"), b"changed")?;
        let changed = collect_sources(&repository_root, &["scenario".into()])?;
        assert_ne!(fingerprint_files(&first)?, fingerprint_files(&changed)?);
        Ok(())
    }

    #[test]
    fn overlapping_source_roots_are_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let repository = tempdir()?;
        fs::create_dir(repository.path().join("scenario"))?;
        fs::write(repository.path().join("scenario/a.rs"), b"a")?;
        let repository_root = repository.path().canonicalize()?;
        assert!(matches!(
            collect_sources(
                &repository_root,
                &["scenario".into(), "scenario/a.rs".into()]
            ),
            Err(SourceError::Duplicate(_))
        ));
        Ok(())
    }
}
