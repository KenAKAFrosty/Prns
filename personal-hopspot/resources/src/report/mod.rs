mod baseline;
mod build;
mod compare;
mod contract;
mod fingerprint;
mod model;
mod summary;

#[cfg(test)]
mod tests;

pub(crate) use baseline::{refresh_baseline, BaselineError};
pub(crate) use build::{write, write_overflow, ReportError};
pub(crate) use compare::{compare_files, ComparisonError};
pub(crate) use summary::{summarize, SummaryError};

pub(crate) fn baseline_path(repository: &std::path::Path) -> std::path::PathBuf {
    baseline::path(repository)
}
