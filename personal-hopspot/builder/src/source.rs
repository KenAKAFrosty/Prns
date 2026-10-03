use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct RepositoryCommit(String);

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct WorkingTreeFingerprint(String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SourceCustody {
    CleanCommit {
        commit: RepositoryCommit,
    },
    WorkingTree {
        head: RepositoryCommit,
        diff_fingerprint: WorkingTreeFingerprint,
    },
}

#[derive(Debug, Error)]
pub enum SourceCaptureError {
    #[error("source repository does not exist: {0}")]
    MissingRepository(PathBuf),
    #[error("could not inspect source repository {path}: {source}")]
    InspectRepository {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("source repository path is not a directory: {0}")]
    InvalidRepository(PathBuf),
    #[error("git {operation} failed: {diagnostic}")]
    Git {
        operation: &'static str,
        diagnostic: String,
    },
    #[error("git returned a non-UTF-8 source path")]
    NonUtf8Path,
    #[error("could not read untracked source {path}: {source}")]
    ReadUntracked {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid repository commit {0:?}")]
    InvalidCommit(String),
    #[error("git returned invalid source commit timestamp {0:?}")]
    InvalidCommitTimestamp(String),
}

impl RepositoryCommit {
    pub fn parse(value: impl Into<String>) -> Result<Self, SourceCaptureError> {
        let value = value.into();
        if value.len() == 40
            && value
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            Ok(Self(value))
        } else {
            Err(SourceCaptureError::InvalidCommit(value))
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl WorkingTreeFingerprint {
    fn from_bytes(bytes: &[u8]) -> Self {
        Self(prns_flash_manifest::sha256_hex(bytes))
    }

    pub fn parse(value: impl Into<String>) -> Result<Self, prns_flash_manifest::DomainValueError> {
        prns_flash_manifest::Sha256Digest::parse(value.into())
            .map(|digest| Self(digest.as_str().to_string()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RepositoryCommit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::fmt::Display for WorkingTreeFingerprint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

macro_rules! serialized_value {
    ($name:ty, $parse:path) => {
        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                $parse(value).map_err(D::Error::custom)
            }
        }
    };
}

serialized_value!(RepositoryCommit, RepositoryCommit::parse);
serialized_value!(WorkingTreeFingerprint, WorkingTreeFingerprint::parse);

pub fn capture_source_custody(repository: &Path) -> Result<SourceCustody, SourceCaptureError> {
    let metadata = match fs::metadata(repository) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            return Err(SourceCaptureError::MissingRepository(
                repository.to_path_buf(),
            ));
        }
        Err(source) => {
            return Err(SourceCaptureError::InspectRepository {
                path: repository.to_path_buf(),
                source,
            });
        }
    };
    if !metadata.is_dir() {
        return Err(SourceCaptureError::InvalidRepository(
            repository.to_path_buf(),
        ));
    }
    let head = git(repository, "resolve HEAD", &["rev-parse", "HEAD"])?;
    let head = RepositoryCommit::parse(String::from_utf8_lossy(&head.stdout).trim())?;
    let diff = git(
        repository,
        "read tracked changes",
        &["diff", "--binary", "HEAD"],
    )?;
    let untracked = git(
        repository,
        "list untracked files",
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?;
    if diff.stdout.is_empty() && untracked.stdout.is_empty() {
        return Ok(SourceCustody::CleanCommit { commit: head });
    }
    let diff_fingerprint = working_tree_fingerprint(repository, &diff.stdout, &untracked.stdout)?;
    Ok(SourceCustody::WorkingTree {
        head,
        diff_fingerprint,
    })
}

pub(crate) fn source_date_epoch(repository: &Path) -> Result<String, SourceCaptureError> {
    let output = git(
        repository,
        "resolve source commit timestamp",
        &["show", "-s", "--format=%ct", "HEAD"],
    )?;
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    parse_source_date_epoch(value)
}

fn parse_source_date_epoch(value: String) -> Result<String, SourceCaptureError> {
    value
        .parse::<u64>()
        .is_ok_and(|timestamp| timestamp > 0)
        .then_some(value.clone())
        .ok_or(SourceCaptureError::InvalidCommitTimestamp(value))
}

fn working_tree_fingerprint(
    repository: &Path,
    diff: &[u8],
    untracked: &[u8],
) -> Result<WorkingTreeFingerprint, SourceCaptureError> {
    let mut identity = Vec::new();
    field(&mut identity, b"personal-hopspot-source-custody-v1");
    field(&mut identity, diff);
    let mut untracked = untracked
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .collect::<Vec<_>>();
    untracked.sort_unstable();
    for encoded in untracked {
        let relative = std::str::from_utf8(encoded).map_err(|_| SourceCaptureError::NonUtf8Path)?;
        let path = repository.join(relative);
        let bytes =
            fs::read(&path).map_err(|source| SourceCaptureError::ReadUntracked { path, source })?;
        field(&mut identity, encoded);
        field(
            &mut identity,
            prns_flash_manifest::sha256_hex(&bytes).as_bytes(),
        );
    }
    Ok(WorkingTreeFingerprint::from_bytes(&identity))
}

fn field(output: &mut Vec<u8>, value: &[u8]) {
    output.extend_from_slice(&(value.len() as u64).to_le_bytes());
    output.extend_from_slice(value);
}

fn git(
    repository: &Path,
    operation: &'static str,
    arguments: &[&str],
) -> Result<Output, SourceCaptureError> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(repository)
        .output()
        .map_err(|error| SourceCaptureError::Git {
            operation,
            diagnostic: error.to_string(),
        })?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(SourceCaptureError::Git {
            operation,
            diagnostic: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_values_validate_during_deserialization() -> Result<(), Box<dyn std::error::Error>> {
        let clean = serde_json::json!({
            "kind": "clean-commit",
            "commit": "a".repeat(40),
        });
        let custody: SourceCustody = serde_json::from_value(clean)?;
        assert!(matches!(custody, SourceCustody::CleanCommit { .. }));
        assert!(serde_json::from_value::<SourceCustody>(serde_json::json!({
            "kind": "clean-commit",
            "commit": "not-a-commit",
        }))
        .is_err());
        Ok(())
    }

    #[test]
    fn untracked_content_changes_the_working_tree_fingerprint(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let repository = tempfile::tempdir()?;
        let source = repository.path().join("source.rs");
        fs::write(&source, b"first")?;
        let first = working_tree_fingerprint(repository.path(), b"diff", b"source.rs\0")?;
        fs::write(&source, b"second")?;
        let second = working_tree_fingerprint(repository.path(), b"diff", b"source.rs\0")?;
        assert_ne!(first, second);
        Ok(())
    }

    #[test]
    fn commit_timestamp_requires_a_positive_integer() {
        assert!(matches!(
            parse_source_date_epoch("1".to_string()),
            Ok(value) if value == "1"
        ));
        assert!(matches!(
            parse_source_date_epoch("0".to_string()),
            Err(SourceCaptureError::InvalidCommitTimestamp(value)) if value == "0"
        ));
        assert!(matches!(
            parse_source_date_epoch("invalid".to_string()),
            Err(SourceCaptureError::InvalidCommitTimestamp(value)) if value == "invalid"
        ));
    }
}
