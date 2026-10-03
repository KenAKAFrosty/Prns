use std::ffi::OsString;
use std::path::PathBuf;

use thiserror::Error;

use crate::LinkOverflowEvidence;

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
    LinkOverflow(Box<LinkOverflowEvidence>),
    #[error("{0}")]
    Manifest(String),
    #[error("resource builds reject inherited build-semantic environment variable {variable:?}")]
    SemanticEnvironmentOverride { variable: OsString },
    #[error("resource Cargo command has no working directory")]
    MissingCargoWorkingDirectory,
    #[error("could not inspect Cargo configuration {path:?}: {source}")]
    CargoConfigurationIo {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not parse Cargo configuration {path:?}: {reason}")]
    CargoConfigurationParse { path: PathBuf, reason: String },
    #[error("resource builds reject external Cargo configuration key {key:?} from {path:?}")]
    ExternalCargoConfiguration { path: PathBuf, key: String },
}
