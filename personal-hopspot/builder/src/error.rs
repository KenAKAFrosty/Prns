use thiserror::Error;

#[derive(Debug, Error)]
pub enum BuildError {
    #[error("{0}")]
    Repository(String),
    #[error("{0}")]
    Toolchain(String),
    #[error("{0}")]
    Build(String),
    #[error("{0}")]
    Artifact(String),
    #[error("{0}")]
    Manifest(String),
}
