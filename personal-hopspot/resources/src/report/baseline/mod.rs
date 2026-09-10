#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use personal_hopspot_builder::artifact::publish;
use personal_hopspot_builder::{BuildContext, BuildError, BuildIntent, LtoMode, SourceCustody};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::matrix::Matrix;

use super::build::{architecture_identity, build_identity, target_identity};
use super::compare::{load_report, ComparisonError};
use super::contract;
use super::model::{BuildStatus, ResourceReport, SCHEMA_VERSION};

pub(super) const BASELINE_SCHEMA_VERSION: u32 = 1;
const BASELINE_PATH: &str = "personal-hopspot/resources/baseline/canonical.json";

#[derive(Debug, Error)]
pub(crate) enum BaselineError {
    #[error("could not read resource baseline {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not parse resource baseline {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error(
        "resource baseline {path} uses schema {actual}, but this tool supports schema {supported}"
    )]
    UnsupportedSchema {
        path: PathBuf,
        actual: u32,
        supported: u32,
    },
    #[error(
        "resource baseline {path} contains report schema {actual}, but this tool supports schema {supported}"
    )]
    UnsupportedReportSchema {
        path: PathBuf,
        actual: u32,
        supported: u32,
    },
    #[error("baseline refresh requires configured resource-report builds")]
    NonCanonicalBuild,
    #[error(transparent)]
    Report(#[from] ComparisonError),
    #[error("resource matrix produced duplicate report for {target:?}")]
    DuplicateTarget { target: String },
    #[error("resource matrix did not produce report for {target:?}")]
    MissingTarget { target: String },
    #[error("resource matrix produced unexpected report for {target:?}")]
    UnexpectedTarget { target: String },
    #[error("resource report path {path} does not identify target {target:?}")]
    MismatchedPath { path: PathBuf, target: String },
    #[error(transparent)]
    CanonicalReport(#[from] CanonicalReportError),
    #[error("could not encode canonical resource evidence: {0}")]
    Json(#[from] serde_json::Error),
    #[error("could not publish canonical resource baseline: {0}")]
    Publish(#[from] BuildError),
}

#[derive(Debug, Error)]
pub(crate) enum CanonicalReportError {
    #[error("resource report for {target:?} is not a successful canonical build")]
    UnsuccessfulTarget { target: String },
    #[error("resource report for {target:?} has stale {dimension}")]
    StaleTarget {
        target: String,
        dimension: &'static str,
    },
    #[error("could not derive the current build identity: {0}")]
    BuildIdentity(#[from] serde_json::Error),
    #[error(transparent)]
    MemoryContract(#[from] contract::ContractIdentityError),
}

pub(crate) struct BaselineOutcome {
    path: PathBuf,
    targets: usize,
}

pub(super) enum SourceExpectation<'a> {
    Historical,
    Current(&'a SourceCustody),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CanonicalBaseline {
    pub(super) schema_version: u32,
    pub(super) report_schema_version: u32,
    pub(super) targets: Vec<ResourceReport>,
}

pub(super) fn path(repository: &Path) -> PathBuf {
    repository.join(BASELINE_PATH)
}

pub(super) fn load(path: &Path) -> Result<CanonicalBaseline, BaselineError> {
    let bytes = fs::read(path).map_err(|source| BaselineError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let baseline = serde_json::from_slice::<CanonicalBaseline>(&bytes).map_err(|source| {
        BaselineError::Parse {
            path: path.to_path_buf(),
            source,
        }
    })?;
    if baseline.schema_version != BASELINE_SCHEMA_VERSION {
        return Err(BaselineError::UnsupportedSchema {
            path: path.to_path_buf(),
            actual: baseline.schema_version,
            supported: BASELINE_SCHEMA_VERSION,
        });
    }
    if baseline.report_schema_version != SCHEMA_VERSION {
        return Err(BaselineError::UnsupportedReportSchema {
            path: path.to_path_buf(),
            actual: baseline.report_schema_version,
            supported: SCHEMA_VERSION,
        });
    }
    Ok(baseline)
}

pub(crate) fn refresh_baseline(
    repository: &Path,
    matrix: &Matrix<'_>,
    context: &BuildContext<'_>,
    report_paths: &[PathBuf],
    source: &SourceCustody,
) -> Result<BaselineOutcome, BaselineError> {
    if !matches!(
        context.intent(),
        BuildIntent::ResourceReport {
            lto: LtoMode::Configured
        }
    ) {
        return Err(BaselineError::NonCanonicalBuild);
    }
    let mut reports = BTreeMap::new();
    for path in report_paths {
        let report = load_report(path)?;
        let target = report.target.id.clone();
        if path.file_stem().and_then(|stem| stem.to_str()) != Some(target.as_str()) {
            return Err(BaselineError::MismatchedPath {
                path: path.clone(),
                target,
            });
        }
        if reports.insert(target.clone(), report).is_some() {
            return Err(BaselineError::DuplicateTarget { target });
        }
    }

    let mut targets = Vec::with_capacity(reports.len());
    for target in matrix.iter() {
        let report = reports
            .remove(target.id())
            .ok_or_else(|| BaselineError::MissingTarget {
                target: target.id().to_string(),
            })?;
        validate_target(target, context, &report, SourceExpectation::Current(source))?;
        targets.push(report);
    }
    if let Some((target, _)) = reports.into_iter().next() {
        return Err(BaselineError::UnexpectedTarget { target });
    }

    let baseline = CanonicalBaseline {
        schema_version: BASELINE_SCHEMA_VERSION,
        report_schema_version: SCHEMA_VERSION,
        targets,
    };
    let mut bytes = serde_json::to_vec_pretty(&baseline)?;
    bytes.push(b'\n');
    let path = path(repository);
    publish(&path, &bytes)?;
    Ok(BaselineOutcome {
        path,
        targets: baseline.targets.len(),
    })
}

pub(super) fn validate_target(
    target: &crate::matrix::Target<'_>,
    context: &BuildContext<'_>,
    report: &ResourceReport,
    source: SourceExpectation<'_>,
) -> Result<(), CanonicalReportError> {
    if !matches!(report.status, BuildStatus::Success) {
        return Err(CanonicalReportError::UnsuccessfulTarget {
            target: target.id().to_string(),
        });
    }
    require_current(
        report.target == target_identity(target),
        target.id(),
        "target identity",
    )?;
    require_current(
        report.architecture == architecture_identity(target),
        target.id(),
        "architecture adapter",
    )?;
    require_current(
        report.build == build_identity(context, target.recipe_identity())?,
        target.id(),
        "build recipe",
    )?;
    require_current(
        report.memory_contract == contract::identity(target.profile())?,
        target.id(),
        "memory contract",
    )?;
    if let SourceExpectation::Current(expected) = source {
        require_current(report.source == *expected, target.id(), "source custody")?;
    }
    Ok(())
}

fn require_current(
    current: bool,
    target: &str,
    dimension: &'static str,
) -> Result<(), CanonicalReportError> {
    if current {
        Ok(())
    } else {
        Err(CanonicalReportError::StaleTarget {
            target: target.to_string(),
            dimension,
        })
    }
}

impl BaselineOutcome {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) const fn targets(&self) -> usize {
        self.targets
    }
}
