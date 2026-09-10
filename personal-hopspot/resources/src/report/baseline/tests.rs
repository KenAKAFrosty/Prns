use std::path::{Path, PathBuf};

use personal_hopspot_builder::{
    BuildContext, BuildIntent, BuildVersion, LtoMode, RepositoryCommit, SourceCustody,
};

use super::*;
use crate::report::tests::{report_value, retarget_executable};

#[test]
fn refresh_writes_the_complete_matrix_in_canonical_order() -> Result<(), Box<dyn std::error::Error>>
{
    let temporary = tempfile::tempdir()?;
    let catalog = prns_flash_manifest::board_catalog()?;
    let matrix = Matrix::from_catalog(&catalog)?;
    let output = temporary.path().join("output");
    let context = context(temporary.path(), &output, LtoMode::Configured)?;
    let reports = write_reports(temporary.path(), &matrix, &context)?;

    let outcome = refresh_baseline(
        temporary.path(),
        &matrix,
        &context,
        &reports,
        &source_custody()?,
    )?;
    let baseline: CanonicalBaseline = serde_json::from_slice(&std::fs::read(outcome.path())?)?;
    assert_eq!(outcome.targets(), 14);
    assert_eq!(baseline.schema_version, BASELINE_SCHEMA_VERSION);
    assert_eq!(baseline.report_schema_version, SCHEMA_VERSION);
    assert_eq!(
        baseline
            .targets
            .iter()
            .map(|report| report.target.id.as_str())
            .collect::<Vec<_>>(),
        matrix
            .iter()
            .map(crate::matrix::Target::id)
            .collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn refresh_rejects_incomplete_and_duplicate_matrices() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let catalog = prns_flash_manifest::board_catalog()?;
    let matrix = Matrix::from_catalog(&catalog)?;
    let output = temporary.path().join("output");
    let context = context(temporary.path(), &output, LtoMode::Configured)?;
    let mut reports = write_reports(temporary.path(), &matrix, &context)?;
    let baseline_path = temporary.path().join(BASELINE_PATH);
    std::fs::create_dir_all(
        baseline_path
            .parent()
            .ok_or("baseline path has no parent")?,
    )?;
    std::fs::write(&baseline_path, b"preserved")?;
    let missing = reports.pop().ok_or("matrix produced no reports")?;
    assert!(matches!(
        refresh_baseline(
            temporary.path(),
            &matrix,
            &context,
            &reports,
            &source_custody()?,
        ),
        Err(BaselineError::MissingTarget { target }) if target == "mesh-tower-v2"
    ));
    assert_eq!(std::fs::read(&baseline_path)?, b"preserved");
    reports.push(missing);
    reports.push(reports[0].clone());
    assert!(matches!(
        refresh_baseline(
            temporary.path(),
            &matrix,
            &context,
            &reports,
            &source_custody()?,
        ),
        Err(BaselineError::DuplicateTarget { target }) if target == "heltec-v4"
    ));
    assert_eq!(std::fs::read(&baseline_path)?, b"preserved");
    Ok(())
}

#[test]
fn refresh_rejects_experimental_codegen() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let catalog = prns_flash_manifest::board_catalog()?;
    let matrix = Matrix::from_catalog(&catalog)?;
    let output = temporary.path().join("output");
    let context = context(temporary.path(), &output, LtoMode::Thin)?;
    assert!(matches!(
        refresh_baseline(temporary.path(), &matrix, &context, &[], &source_custody()?,),
        Err(BaselineError::NonCanonicalBuild)
    ));
    Ok(())
}

#[test]
fn refresh_rejects_stale_source_custody() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let catalog = prns_flash_manifest::board_catalog()?;
    let matrix = Matrix::from_catalog(&catalog)?;
    let output = temporary.path().join("output");
    let context = context(temporary.path(), &output, LtoMode::Configured)?;
    let reports = write_reports(temporary.path(), &matrix, &context)?;
    let stale = RepositoryCommit::parse("d".repeat(40))
        .map(|commit| SourceCustody::CleanCommit { commit })?;

    assert!(matches!(
        refresh_baseline(temporary.path(), &matrix, &context, &reports, &stale),
        Err(BaselineError::CanonicalReport(
            CanonicalReportError::StaleTarget {
                dimension: "source custody",
                ..
            }
        ))
    ));
    Ok(())
}

#[test]
fn refresh_rejects_missing_scenario_future_evidence() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let catalog = prns_flash_manifest::board_catalog()?;
    let matrix = Matrix::from_catalog(&catalog)?;
    let output = temporary.path().join("output");
    let context = context(temporary.path(), &output, LtoMode::Configured)?;
    let reports = write_reports(temporary.path(), &matrix, &context)?;
    let path = &reports[0];
    let mut value: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
    value["analysis"]["async_memory"]["value"]["scenario_futures"] = serde_json::json!({
        "kind": "unavailable",
        "reason": "semantic-harness-not-produced"
    });
    std::fs::write(path, serde_json::to_vec(&value)?)?;

    assert!(matches!(
        refresh_baseline(
            temporary.path(),
            &matrix,
            &context,
            &reports,
            &source_custody()?,
        ),
        Err(BaselineError::CanonicalReport(
            CanonicalReportError::StaleTarget {
                dimension: "scenario-future evidence",
                ..
            }
        ))
    ));
    Ok(())
}

#[test]
fn load_rejects_unknown_baseline_and_report_schemas() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let path = temporary.path().join("baseline.json");
    std::fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({
            "schema_version": BASELINE_SCHEMA_VERSION + 1,
            "report_schema_version": SCHEMA_VERSION,
            "targets": []
        }))?,
    )?;
    assert!(matches!(
        load(&path),
        Err(BaselineError::UnsupportedSchema { actual, .. })
            if actual == BASELINE_SCHEMA_VERSION + 1
    ));
    std::fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({
            "schema_version": BASELINE_SCHEMA_VERSION,
            "report_schema_version": SCHEMA_VERSION + 1,
            "targets": []
        }))?,
    )?;
    assert!(matches!(
        load(&path),
        Err(BaselineError::UnsupportedReportSchema { actual, .. })
            if actual == SCHEMA_VERSION + 1
    ));
    Ok(())
}

fn context<'a>(
    repository: &'a Path,
    output: &'a Path,
    lto: LtoMode,
) -> Result<BuildContext<'a>, BuildError> {
    BuildContext::new(repository, output, BuildVersion::Developer("0.1.0"))
        .map(|context| context.with_intent(BuildIntent::ResourceReport { lto }))
}

fn source_custody() -> Result<SourceCustody, personal_hopspot_builder::SourceCaptureError> {
    RepositoryCommit::parse("c".repeat(40)).map(|commit| SourceCustody::CleanCommit { commit })
}

fn write_reports(
    root: &Path,
    matrix: &Matrix<'_>,
    context: &BuildContext<'_>,
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    matrix
        .iter()
        .map(|target| {
            let mut report: ResourceReport = serde_json::from_value(report_value())?;
            report.target = target_identity(target);
            report.architecture = architecture_identity(target);
            retarget_executable(&mut report, target);
            report.build = build_identity(context, target.recipe_identity())?;
            report.memory_contract = contract::identity(target.profile())?;
            let path = root.join("reports").join(format!("{}.json", target.id()));
            std::fs::create_dir_all(path.parent().ok_or("report path has no parent")?)?;
            std::fs::write(&path, serde_json::to_vec(&report)?)?;
            Ok(path)
        })
        .collect()
}
