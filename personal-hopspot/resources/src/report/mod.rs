mod baseline;
mod build;
mod compare;
mod contract;
mod document;
mod executable;
mod fingerprint;
mod model;
mod summary;

#[cfg(test)]
mod tests;

pub(crate) use baseline::{refresh_baseline, BaselineError};
pub(crate) use build::{write, write_overflow, ReportError};
pub(crate) use compare::compare_files;
pub use compare::ComparisonError;
pub use document::{BuildOutcome, Document};
pub use fingerprint::Fingerprint;
pub(crate) use summary::{summarize, SummaryError};

pub const SCHEMA_VERSION: u32 = model::SCHEMA_VERSION;

pub(crate) fn baseline_path(repository: &std::path::Path) -> std::path::PathBuf {
    baseline::path(repository)
}
