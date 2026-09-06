use std::path::{Path, PathBuf};

use prns_flash_manifest::ReleaseVersion;

use crate::BuildError;

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
        })
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

    pub fn board_output(&self, board_slug: &str) -> PathBuf {
        self.output_root
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn developer_context_owns_validated_identity_and_paths() -> Result<(), BuildError> {
        let repository = Path::new("/repository");
        let output = Path::new("/artifacts");
        let digest = "e3ffc728180a8194c2efb55f90b0285f093db6e53e6dc800d4b229426e966399";
        let version = format!("0.3.7-dev.dirty.{digest}");
        let context = BuildContext::new(repository, output, BuildVersion::Developer(&version))?;

        assert_eq!(context.repository(), repository);
        assert_eq!(context.output_root(), output);
        assert_eq!(context.version(), version);
        assert_eq!(context.source_digest(), Some(digest));
        assert_eq!(
            context.board_output("t-echo"),
            Path::new("/artifacts/firmware/hopspot/t-echo").join(&version)
        );
        assert_eq!(
            context.work_output("t-echo"),
            Path::new("/repository/target/flash-artifacts/work/t-echo")
        );
        assert_eq!(
            context.release_part_path("t-echo", "application.uf2"),
            format!("firmware/hopspot/t-echo/{version}/application.uf2")
        );
        Ok(())
    }

    #[test]
    fn release_artifact_root_is_repository_scoped() {
        assert_eq!(
            default_artifact_root(Path::new("/repository")),
            Path::new("/repository/target/flash-artifacts")
        );
    }

    #[test]
    fn invalid_developer_versions_never_form_contexts() {
        assert!(matches!(
            BuildContext::new(
                Path::new("/repository"),
                Path::new("/artifacts"),
                BuildVersion::Developer("../invalid")
            ),
            Err(BuildError::Repository(_))
        ));
    }
}
