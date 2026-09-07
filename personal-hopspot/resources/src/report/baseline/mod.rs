#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use personal_hopspot_builder::artifact::publish;
use personal_hopspot_builder::{BuildContext, BuildError, BuildIntent, LtoMode};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::matrix::Matrix;

use super::build::{architecture_identity, build_identity, target_identity};
use super::compare::{load_report, ComparisonError};
use super::contract;
use super::model::{BuildStatus, ResourceReport, SCHEMA_VERSION};

const BASELINE_SCHEMA_VERSION: u32 = 1;
const BASELINE_PATH: &str = "personal-hopspot/resources/baseline/canonical.json";

#[derive(Debug, Error)]
pub(crate) enum BaselineError {
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
    #[error("resource report for {target:?} is not a successful canonical build")]
    UnsuccessfulTarget { target: String },
    #[error("resource report for {target:?} has stale {dimension}")]
    StaleTarget {
        target: String,
        dimension: &'static str,
    },
    #[error("could not encode canonical resource evidence: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Contract(#[from] contract::ContractIdentityError),
    #[error("could not publish canonical resource baseline: {0}")]
    Publish(#[from] BuildError),
}

pub(crate) struct BaselineOutcome {
    path: PathBuf,
    targets: usize,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CanonicalBaseline {
    schema_version: u32,
    report_schema_version: u32,
    targets: Vec<ResourceReport>,
}

pub(crate) fn refresh_baseline(
    repository: &Path,
    matrix: &Matrix<'_>,
    context: &BuildContext<'_>,
    report_paths: &[PathBuf],
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
        validate_target(target, context, &report)?;
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
    let path = repository.join(BASELINE_PATH);
    publish(&path, &bytes)?;
    Ok(BaselineOutcome {
        path,
        targets: baseline.targets.len(),
    })
}

fn validate_target(
    target: &crate::matrix::Target<'_>,
    context: &BuildContext<'_>,
    report: &ResourceReport,
) -> Result<(), BaselineError> {
    if !matches!(report.status, BuildStatus::Success) {
        return Err(BaselineError::UnsuccessfulTarget {
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
    )
}

fn require_current(
    current: bool,
    target: &str,
    dimension: &'static str,
) -> Result<(), BaselineError> {
    if current {
        Ok(())
    } else {
        Err(BaselineError::StaleTarget {
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
