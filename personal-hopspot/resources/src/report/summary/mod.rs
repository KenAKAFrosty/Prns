mod model;
mod render;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use personal_hopspot_builder::artifact::publish;
use personal_hopspot_builder::{BuildContext, BuildError};
use thiserror::Error;

use crate::matrix::Matrix;

use super::baseline::{self, BaselineError, CanonicalReportError, BASELINE_SCHEMA_VERSION};
use super::compare::{self, ComparisonError};
use super::model::{Evidence, RamCapacityIdentity, ResourceReport, SCHEMA_VERSION};
use model::{
    ByteMetric, MatrixSummary, TargetSummary, ToolchainRelation, ToolchainSummary,
    SCHEMA_VERSION as SUMMARY_SCHEMA_VERSION,
};

const JSON_FILENAME: &str = "matrix.json";
const MARKDOWN_FILENAME: &str = "matrix.md";

#[derive(Clone, Copy, Debug)]
pub(crate) enum EvidenceSet {
    Baseline,
    Current,
}

#[derive(Debug, Error)]
pub(crate) enum SummaryError {
    #[error(transparent)]
    Baseline(#[from] BaselineError),
    #[error(transparent)]
    CanonicalReport(#[from] CanonicalReportError),
    #[error(transparent)]
    Report(#[from] ComparisonError),
    #[error("resource report root is not a directory: {path}")]
    InvalidReportRoot { path: PathBuf },
    #[error("could not read resource report directory {path}: {source}")]
    ReadDirectory {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("resource evidence contains a symbolic link: {path}")]
    SymbolicLink { path: PathBuf },
    #[error("resource report has no canonical fragment layout: {path}")]
    InvalidReportLayout { path: PathBuf },
    #[error("could not inspect linker map {path}: {source}")]
    LinkerMapMetadata {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("linker map is not a regular file: {path}")]
    InvalidLinkerMap { path: PathBuf },
    #[error("linker map {path} contains {actual} bytes, but its report records {expected} bytes")]
    LinkerMapSize {
        path: PathBuf,
        actual: u64,
        expected: u64,
    },
    #[error("{set} resource evidence repeats target {target:?}")]
    DuplicateTarget { set: EvidenceSet, target: String },
    #[error("{set} resource evidence is missing target {target:?}")]
    MissingTarget { set: EvidenceSet, target: String },
    #[error("{set} resource evidence contains unexpected target {target:?}")]
    UnexpectedTarget { set: EvidenceSet, target: String },
    #[error("resource report path {path} does not identify target {target:?}")]
    MismatchedPath { path: PathBuf, target: String },
    #[error("resource report for {target:?} has no complete {evidence} evidence")]
    IncompleteEvidence {
        target: String,
        evidence: &'static str,
    },
    #[error("resource summary overflowed metric {metric:?} for target {target:?}")]
    MetricOverflow {
        target: String,
        metric: &'static str,
    },
    #[error("could not serialize resource matrix summary: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("could not publish resource matrix summary: {0}")]
    Publish(#[from] BuildError),
}

pub(crate) struct SummaryOutcome {
    json: PathBuf,
    markdown: PathBuf,
    targets: usize,
}

struct CurrentReport {
    path: PathBuf,
    report: ResourceReport,
}

struct RamTotals {
    static_bytes: u64,
    known_used_bytes: u64,
    known_headroom_bytes: u64,
    runtime_detected: Vec<String>,
}

pub(crate) fn summarize(
    matrix: &Matrix<'_>,
    context: &BuildContext<'_>,
    reports_root: &Path,
    baseline_path: &Path,
    output: &Path,
) -> Result<SummaryOutcome, SummaryError> {
    let baseline = baseline::load(baseline_path)?;
    let mut baseline_reports = BTreeMap::new();
    for report in baseline.targets {
        compare::validate_report(baseline_path, &report)?;
        let target = report.target.id.clone();
        if baseline_reports.insert(target.clone(), report).is_some() {
            return Err(SummaryError::DuplicateTarget {
                set: EvidenceSet::Baseline,
                target,
            });
        }
    }

    let mut current_reports = BTreeMap::new();
    for path in discover_reports(reports_root)? {
        let report = compare::load_report(&path)?;
        let target = report.target.id.clone();
        if path.file_stem().and_then(|stem| stem.to_str()) != Some(target.as_str()) {
            return Err(SummaryError::MismatchedPath { path, target });
        }
        if current_reports
            .insert(target.clone(), CurrentReport { path, report })
            .is_some()
        {
            return Err(SummaryError::DuplicateTarget {
                set: EvidenceSet::Current,
                target,
            });
        }
    }

    let mut targets = Vec::with_capacity(matrix.iter().count());
    for target in matrix.iter() {
        let baseline_report =
            baseline_reports
                .remove(target.id())
                .ok_or_else(|| SummaryError::MissingTarget {
                    set: EvidenceSet::Baseline,
                    target: target.id().to_string(),
                })?;
        let current =
            current_reports
                .remove(target.id())
                .ok_or_else(|| SummaryError::MissingTarget {
                    set: EvidenceSet::Current,
                    target: target.id().to_string(),
                })?;
        baseline::validate_target(target, context, &baseline_report)?;
        validate_linker_map(&current.path, &current.report)?;
        baseline::validate_target(target, context, &current.report)?;
        compare::require_matrix_compatible(&baseline_report, &current.report)?;
        targets.push(target_summary(
            target.id(),
            &baseline_report,
            &current.report,
        )?);
    }
    reject_unexpected(EvidenceSet::Baseline, baseline_reports.keys().next())?;
    reject_unexpected(EvidenceSet::Current, current_reports.keys().next())?;

    let summary = MatrixSummary {
        schema_version: SUMMARY_SCHEMA_VERSION,
        report_schema_version: SCHEMA_VERSION,
        baseline_schema_version: BASELINE_SCHEMA_VERSION,
        targets,
    };
    let mut json = serde_json::to_vec_pretty(&summary)?;
    json.push(b'\n');
    let markdown = render::markdown(&summary);
    let json_path = output.join(JSON_FILENAME);
    let markdown_path = output.join(MARKDOWN_FILENAME);
    publish(&json_path, &json)?;
    publish(&markdown_path, markdown.as_bytes())?;
    Ok(SummaryOutcome {
        json: json_path,
        markdown: markdown_path,
        targets: summary.targets.len(),
    })
}

fn discover_reports(root: &Path) -> Result<Vec<PathBuf>, SummaryError> {
    let metadata = fs::symlink_metadata(root).map_err(|source| SummaryError::ReadDirectory {
        path: root.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() {
        return Err(SummaryError::SymbolicLink {
            path: root.to_path_buf(),
        });
    }
    if !metadata.is_dir() {
        return Err(SummaryError::InvalidReportRoot {
            path: root.to_path_buf(),
        });
    }
    let mut reports = Vec::new();
    discover_directory(root, &mut reports)?;
    reports.sort();
    Ok(reports)
}

fn discover_directory(directory: &Path, reports: &mut Vec<PathBuf>) -> Result<(), SummaryError> {
    let entries = fs::read_dir(directory).map_err(|source| SummaryError::ReadDirectory {
        path: directory.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| SummaryError::ReadDirectory {
            path: directory.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let kind = entry
            .file_type()
            .map_err(|source| SummaryError::ReadDirectory {
                path: path.clone(),
                source,
            })?;
        if kind.is_symlink() {
            return Err(SummaryError::SymbolicLink { path });
        }
        if kind.is_dir() {
            discover_directory(&path, reports)?;
        } else if kind.is_file()
            && path.extension().and_then(|value| value.to_str()) == Some("json")
        {
            reports.push(path);
        }
    }
    Ok(())
}

fn validate_linker_map(path: &Path, report: &ResourceReport) -> Result<(), SummaryError> {
    let reports = path
        .parent()
        .filter(|parent| parent.file_name().and_then(|name| name.to_str()) == Some("reports"))
        .ok_or_else(|| SummaryError::InvalidReportLayout {
            path: path.to_path_buf(),
        })?;
    let fragment = reports
        .parent()
        .ok_or_else(|| SummaryError::InvalidReportLayout {
            path: path.to_path_buf(),
        })?;
    let linker_map = fragment
        .join("work")
        .join(&report.target.id)
        .join("linker.map");
    let metadata =
        fs::symlink_metadata(&linker_map).map_err(|source| SummaryError::LinkerMapMetadata {
            path: linker_map.clone(),
            source,
        })?;
    if metadata.file_type().is_symlink() {
        return Err(SummaryError::SymbolicLink { path: linker_map });
    }
    if !metadata.is_file() {
        return Err(SummaryError::InvalidLinkerMap { path: linker_map });
    }
    if metadata.len() != report.analysis.linker_map_bytes {
        return Err(SummaryError::LinkerMapSize {
            path: linker_map,
            actual: metadata.len(),
            expected: report.analysis.linker_map_bytes,
        });
    }
    Ok(())
}

fn reject_unexpected(set: EvidenceSet, target: Option<&String>) -> Result<(), SummaryError> {
    match target {
        Some(target) => Err(SummaryError::UnexpectedTarget {
            set,
            target: target.clone(),
        }),
        None => Ok(()),
    }
}

fn target_summary(
    target: &str,
    baseline: &ResourceReport,
    current: &ResourceReport,
) -> Result<TargetSummary, SummaryError> {
    let baseline_flash = complete_flash(target, baseline)?;
    let current_flash = complete_flash(target, current)?;
    let baseline_ram = ram_totals(target, baseline)?;
    let current_ram = ram_totals(target, current)?;
    Ok(TargetSummary {
        id: current.target.id.clone(),
        display_name: current.target.display_name.clone(),
        memory_profile: current.target.memory_profile.clone(),
        rust_target: current.architecture.rust_target.clone(),
        linker_flavor: current.architecture.linker_flavor.clone(),
        build_fingerprint: current.build.fingerprint.to_string(),
        toolchain: ToolchainSummary {
            baseline_fingerprint: baseline.toolchain.fingerprint.to_string(),
            current_fingerprint: current.toolchain.fingerprint.to_string(),
            relation: if baseline.toolchain == current.toolchain {
                ToolchainRelation::Exact
            } else {
                ToolchainRelation::Different
            },
        },
        flash_image: ByteMetric::new(baseline_flash.image_bytes, current_flash.image_bytes),
        flash_headroom: ByteMetric::new(
            baseline_flash.headroom_bytes,
            current_flash.headroom_bytes,
        ),
        static_ram: ByteMetric::new(baseline_ram.static_bytes, current_ram.static_bytes),
        known_ram_used: ByteMetric::new(
            baseline_ram.known_used_bytes,
            current_ram.known_used_bytes,
        ),
        known_ram_headroom: ByteMetric::new(
            baseline_ram.known_headroom_bytes,
            current_ram.known_headroom_bytes,
        ),
        runtime_detected_ram: current_ram.runtime_detected,
    })
}

fn complete_flash<'a>(
    target: &str,
    report: &'a ResourceReport,
) -> Result<&'a super::model::FirmwareFlashUsage, SummaryError> {
    match &report.firmware_flash {
        Evidence::Complete(flash) => Ok(flash),
        Evidence::Partial(_) | Evidence::Unavailable => Err(SummaryError::IncompleteEvidence {
            target: target.to_string(),
            evidence: "flash",
        }),
    }
}

fn ram_totals(target: &str, report: &ResourceReport) -> Result<RamTotals, SummaryError> {
    let ram = match &report.static_ram {
        Evidence::Complete(ram) => ram,
        Evidence::Partial(_) | Evidence::Unavailable => {
            return Err(SummaryError::IncompleteEvidence {
                target: target.to_string(),
                evidence: "RAM",
            });
        }
    };
    let mut totals = RamTotals {
        static_bytes: 0,
        known_used_bytes: 0,
        known_headroom_bytes: 0,
        runtime_detected: Vec::new(),
    };
    for usage in ram {
        totals.static_bytes = add(
            target,
            "static RAM",
            totals.static_bytes,
            usage.static_section_bytes,
        )?;
        match usage.capacity {
            RamCapacityIdentity::Known {
                bytes,
                headroom_bytes,
            } => {
                let used = bytes.checked_sub(headroom_bytes).ok_or_else(|| {
                    SummaryError::MetricOverflow {
                        target: target.to_string(),
                        metric: "known RAM used",
                    }
                })?;
                totals.known_used_bytes =
                    add(target, "known RAM used", totals.known_used_bytes, used)?;
                totals.known_headroom_bytes = add(
                    target,
                    "known RAM headroom",
                    totals.known_headroom_bytes,
                    headroom_bytes,
                )?;
            }
            RamCapacityIdentity::RuntimeDetected => {
                totals.runtime_detected.push(usage.backing_store.clone());
            }
        }
    }
    Ok(totals)
}

fn add(target: &str, metric: &'static str, left: u64, right: u64) -> Result<u64, SummaryError> {
    left.checked_add(right)
        .ok_or_else(|| SummaryError::MetricOverflow {
            target: target.to_string(),
            metric,
        })
}

impl SummaryOutcome {
    pub(crate) fn json(&self) -> &Path {
        &self.json
    }

    pub(crate) fn markdown(&self) -> &Path {
        &self.markdown
    }

    pub(crate) const fn targets(&self) -> usize {
        self.targets
    }
}

impl std::fmt::Display for EvidenceSet {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Baseline => "baseline",
            Self::Current => "current",
        })
    }
}
