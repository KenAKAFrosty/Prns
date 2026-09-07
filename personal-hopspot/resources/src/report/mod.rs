mod build;
mod compare;
mod contract;
mod fingerprint;
mod model;

#[cfg(test)]
mod tests;

pub(crate) use build::{write, ReportError};
pub(crate) use compare::{compare_files, ComparisonError};
