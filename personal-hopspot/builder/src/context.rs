mod firmware;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use prns_flash_manifest::ReleaseVersion;

use crate::{BuildConfiguration, BuildError};

static BUILD_CONTEXT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildVersion<'a> {
    Repository,
    Developer(&'a str),
}

#[derive(Debug)]
pub struct BuildContext<'a> {
    repository: &'a Path,
    output_root: &'a Path,
    version: String,
    configuration: BuildConfiguration,
    evidence_run_id: String,
}

impl<'a> BuildContext<'a> {
    pub fn new(
        repository: &'a Path,
        output_root: &'a Path,
        build_version: BuildVersion<'_>,
    ) -> Result<Self, BuildError> {
        let version = resolve_build_version(repository, build_version)?;
        Ok(Self {
            repository,
            output_root,
            version,
            configuration: BuildConfiguration::default(),
            evidence_run_id: evidence_run_id(),
        })
    }

    #[must_use]
    pub const fn with_configuration(mut self, configuration: BuildConfiguration) -> Self {
        self.configuration = configuration;
        self
    }

    pub const fn repository(&self) -> &'a Path {
        self.repository
    }

    pub const fn output_root(&self) -> &'a Path {
        self.output_root
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn source_digest(&self) -> Option<&str> {
        developer_source_digest(&self.version)
    }

    pub const fn configuration(&self) -> BuildConfiguration {
        self.configuration
    }

    pub fn board_output(&self, board_slug: &str) -> PathBuf {
        self.configured_output_root()
            .join("firmware")
            .join("hopspot")
            .join(board_slug)
            .join(&self.version)
    }

    pub fn work_output(&self, board_slug: &str) -> PathBuf {
        self.repository
            .join("target")
            .join("flash-artifacts")
            .join("work")
            .join(board_slug)
    }

    pub fn release_part_path(&self, board_slug: &str, filename: &str) -> String {
        format!("firmware/hopspot/{board_slug}/{}/{filename}", self.version)
    }

    pub fn configured_output_root(&self) -> PathBuf {
        if self.configuration.isolates_artifacts() {
            self.output_root.join(self.configuration.lto().as_str())
        } else {
            self.output_root.to_path_buf()
        }
    }
}

pub fn default_artifact_root(repository: &Path) -> PathBuf {
    repository.join("target").join("flash-artifacts")
}

fn resolve_build_version(
    repository: &Path,
    build_version: BuildVersion<'_>,
) -> Result<String, BuildError> {
    match build_version {
        BuildVersion::Repository => repository_version(repository),
        BuildVersion::Developer(version) => validated_version(version),
    }
}

fn repository_version(repository: &Path) -> Result<String, BuildError> {
    let version = match std::env::var("PRNS_FLASH_VERSION") {
        Ok(version) => version,
        Err(std::env::VarError::NotPresent) => std::fs::read_to_string(repository.join("VERSION"))
            .map_err(|error| BuildError::Repository(format!("could not read VERSION: {error}")))?,
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(BuildError::Repository(
                "PRNS_FLASH_VERSION must be UTF-8".to_string(),
            ));
        }
    };
    validated_version(version.trim())
}

fn validated_version(version: &str) -> Result<String, BuildError> {
    ReleaseVersion::parse(version.to_string())
        .map(|version| version.as_str().to_string())
        .map_err(|error| BuildError::Repository(error.to_string()))
}

fn developer_source_digest(version: &str) -> Option<&str> {
    let digest = version.rsplit('.').next()?;
    (version.contains("-dev.")
        && digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
    .then_some(digest)
}

fn evidence_run_id() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let sequence = BUILD_CONTEXT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{}-{timestamp}-{sequence}", std::process::id())
}
