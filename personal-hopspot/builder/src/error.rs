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
    #[error("{target} application is {actual} bytes; firmware owns at most {maximum}")]
    FirmwareOverflow {
        target: String,
        actual: u64,
        maximum: u64,
    },
    #[error("{0}")]
    Manifest(String),
}
