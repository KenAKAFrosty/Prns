use std::path::{Path, PathBuf};

use personal_hopspot_builder::{BuildContext, BuildError, BuildIntent, BuildVersion, LtoMode};
use serde_json::{json, Value};

use super::model::{ByteDelta, MatrixSummary, ToolchainRelation};
use super::*;
use crate::matrix::{Matrix, Target, TargetPlatform};
use crate::report::build::{architecture_identity, build_identity, target_identity};
use crate::report::contract;
use crate::report::tests::report_value;

#[test]
fn summary_merges_catalog_order_and_reports_numeric_deltas(
) -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let catalog = prns_flash_manifest::board_catalog()?;
    let matrix = Matrix::from_catalog(&catalog)?;
    let build_output = temporary.path().join("build");
    let context = context(temporary.path(), &build_output)?;
    let baseline = prepare_baseline(temporary.path(), &matrix, &context)?;
    let reports = temporary.path().join("fragments");
    write_reports(&reports, &matrix, &context, |target, value| {
        if target.id() == "t114" {
            value["toolchain"]["fingerprint"] =
                json!("54c4eff625da0c1a71c36e76752830d74114685d1a231dd1b876eea7a6fd3e9a");
            value["toolchain"]["rustc_version"] = json!("rustc from CI");
            value["firmware_flash"]["value"]["image_bytes"] = json!(700_100);
            value["firmware_flash"]["value"]["headroom_bytes"] = json!(65_852);
            value["static_ram"]["value"][0]["static_section_bytes"] = json!(138_406);
            value["static_ram"]["value"][0]["capacity"]["headroom_bytes"] = json!(4_954);
        }
    })?;
    let cargo_metadata = reports
        .join("embedded-resources-esp")
        .join("work")
        .join("heltec-v4")
        .join("cargo")
        .join(".rustc_info.json");
    std::fs::create_dir_all(
        cargo_metadata
            .parent()
            .ok_or("Cargo metadata has no parent")?,
    )?;
    std::fs::write(&cargo_metadata, b"not a resource report")?;

    let output = temporary.path().join("summary");
    let outcome = summarize(&matrix, &context, &reports, &baseline, &output)?;
    let summary: MatrixSummary = serde_json::from_slice(&std::fs::read(outcome.json())?)?;
    assert_eq!(outcome.targets(), 14);
    assert_eq!(summary.schema_version, model::SCHEMA_VERSION);
    assert_eq!(
        summary
            .targets
            .iter()
            .map(|target| target.id.as_str())
            .collect::<Vec<_>>(),
        matrix.iter().map(Target::id).collect::<Vec<_>>()
    );
    let t114 = summary
        .targets
        .iter()
        .find(|target| target.id == "t114")
        .ok_or("summary omitted t114")?;
    assert_eq!(
        (
            t114.flash_image.current_bytes,
            t114.flash_image.delta,
            t114.static_ram.delta,
            t114.known_ram_used.delta,
            t114.known_ram_headroom.delta,
            t114.toolchain.relation,
        ),
        (
            700_100,
            ByteDelta::Increase(100),
            ByteDelta::Increase(50),
            ByteDelta::Increase(50),
            ByteDelta::Decrease(50),
            ToolchainRelation::Different,
        )
    );
    let markdown = std::fs::read_to_string(outcome.markdown())?;
    assert!(markdown.contains("Exact byte counts; deltas are current minus"));
    assert!(markdown.contains("(`t114`)"));
    assert!(markdown.contains("| 700100 | +100 | 65852 (-100) |"));
    Ok(())
}

#[test]
fn summary_rejects_duplicate_and_missing_fragment_targets_before_writing(
) -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let catalog = prns_flash_manifest::board_catalog()?;
    let matrix = Matrix::from_catalog(&catalog)?;
    let build_output = temporary.path().join("build");
    let context = context(temporary.path(), &build_output)?;
    let baseline = prepare_baseline(temporary.path(), &matrix, &context)?;
    let reports = temporary.path().join("fragments");
    let paths = write_reports(&reports, &matrix, &context, |_, _| {})?;
    let duplicate = reports
        .join("duplicate")
        .join("reports")
        .join("heltec-v4.json");
    std::fs::create_dir_all(duplicate.parent().ok_or("duplicate has no parent")?)?;
    std::fs::copy(&paths[0], &duplicate)?;
    let output = temporary.path().join("summary");
    assert!(matches!(
        summarize(&matrix, &context, &reports, &baseline, &output),
        Err(SummaryError::DuplicateTarget {
            set: EvidenceSet::Current,
            target,
        }) if target == "heltec-v4"
    ));
    assert!(!output.exists());

    std::fs::remove_file(duplicate)?;
    let last = paths.last().ok_or("matrix produced no reports")?;
    let linker_map = last
        .parent()
        .and_then(Path::parent)
        .ok_or("report has no fragment root")?
        .join("work")
        .join("mesh-tower-v2")
        .join("linker.map");
    std::fs::remove_file(&linker_map)?;
    assert!(matches!(
        summarize(&matrix, &context, &reports, &baseline, &output),
        Err(SummaryError::LinkerMapMetadata { path, .. }) if path == linker_map
    ));
    assert!(!output.exists());

    std::fs::write(&linker_map, vec![0; 128])?;
    let missing = paths.last().ok_or("matrix produced no reports")?;
    std::fs::remove_file(missing)?;
    assert!(matches!(
        summarize(&matrix, &context, &reports, &baseline, &output),
        Err(SummaryError::MissingTarget {
            set: EvidenceSet::Current,
            target,
        }) if target == "mesh-tower-v2"
    ));
    assert!(!output.exists());
    Ok(())
}

#[test]
fn summary_rejects_stale_build_identity_before_writing() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let catalog = prns_flash_manifest::board_catalog()?;
    let matrix = Matrix::from_catalog(&catalog)?;
    let build_output = temporary.path().join("build");
    let context = context(temporary.path(), &build_output)?;
    let baseline = prepare_baseline(temporary.path(), &matrix, &context)?;
    let reports = temporary.path().join("fragments");
    write_reports(&reports, &matrix, &context, |target, value| {
        if target.id() == "t114" {
            value["build"]["fingerprint"] = Value::String("b".repeat(64));
        }
    })?;
    let output = temporary.path().join("summary");
    assert!(matches!(
        summarize(&matrix, &context, &reports, &baseline, &output),
        Err(SummaryError::CanonicalReport(CanonicalReportError::StaleTarget {
            target,
            dimension: "build recipe",
        })) if target == "t114"
    ));
    assert!(!output.exists());
    Ok(())
}

fn context<'a>(repository: &'a Path, output: &'a Path) -> Result<BuildContext<'a>, BuildError> {
    BuildContext::new(repository, output, BuildVersion::Developer("0.1.0")).map(|context| {
        context.with_intent(BuildIntent::ResourceReport {
            lto: LtoMode::Configured,
        })
    })
}

fn prepare_baseline(
    root: &Path,
    matrix: &Matrix<'_>,
    context: &BuildContext<'_>,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let reports = root.join("baseline-reports");
    let paths = write_reports(&reports, matrix, context, |_, _| {})?;
    Ok(baseline::refresh_baseline(root, matrix, context, &paths)?
        .path()
        .to_path_buf())
}

fn write_reports(
    root: &Path,
    matrix: &Matrix<'_>,
    context: &BuildContext<'_>,
    mutate: impl Fn(&Target<'_>, &mut Value),
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    matrix
        .iter()
        .map(|target| {
            let mut report: ResourceReport = serde_json::from_value(report_value())?;
            report.target = target_identity(target);
            report.architecture = architecture_identity(target);
            report.build = build_identity(context, target.recipe_identity())?;
            report.memory_contract = contract::identity(target.profile())?;
            let mut value = serde_json::to_value(report)?;
            mutate(target, &mut value);
            let report: ResourceReport = serde_json::from_value(value)?;
            let platform = match target.platform() {
                TargetPlatform::Esp => "embedded-resources-esp",
                TargetPlatform::Nrf52840 => "embedded-resources-nrf52840",
            };
            let path = root
                .join(platform)
                .join("reports")
                .join(format!("{}.json", target.id()));
            std::fs::create_dir_all(path.parent().ok_or("report has no parent")?)?;
            std::fs::write(&path, serde_json::to_vec(&report)?)?;
            let linker_map = root
                .join(platform)
                .join("work")
                .join(target.id())
                .join("linker.map");
            std::fs::create_dir_all(linker_map.parent().ok_or("map has no parent")?)?;
            std::fs::write(
                linker_map,
                vec![0; usize::try_from(report.analysis.linker_map_bytes)?],
            )?;
            Ok(path)
        })
        .collect()
}
