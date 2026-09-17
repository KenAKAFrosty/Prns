use std::fs;

use tempfile::tempdir;

use super::{compare, ComparisonError};
use crate::contract::{
    EvidenceFingerprint, Failure, FailureKind, MatrixStatus, ResourceEvidence, SourceCommit,
    SourceCustody, Verdict,
};
use crate::evidence::assemble;

fn write_matrix(
    path: &std::path::Path,
    matrix: &crate::contract::AssuranceMatrix,
) -> Result<(), Box<dyn std::error::Error>> {
    fs::write(path, serde_json::to_vec_pretty(matrix)?)?;
    Ok(())
}

fn fingerprint(byte: char) -> Result<EvidenceFingerprint, crate::contract::ValueError> {
    EvidenceFingerprint::parse(byte.to_string().repeat(64))
}

#[test]
fn comparison_rejects_incompatible_target_contracts() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempdir()?;
    let before_path = directory.path().join("before.json");
    let after_path = directory.path().join("after.json");
    let before = assemble(Vec::new(), Vec::new())?;
    let mut after = before.clone();
    after.targets[0].memory_profile = "different-profile".to_string();
    write_matrix(&before_path, &before)?;
    write_matrix(&after_path, &after)?;
    assert!(matches!(
        compare(&before_path, &after_path),
        Err(ComparisonError::Incompatible("target contract"))
    ));
    Ok(())
}

#[test]
fn comparison_calls_out_fingerprint_changes() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempdir()?;
    let before_path = directory.path().join("before.json");
    let after_path = directory.path().join("after.json");
    let mut before = assemble(Vec::new(), Vec::new())?;
    before.targets[0].resource = Verdict::Passed {
        evidence: ResourceEvidence {
            source: SourceCustody::CleanCommit {
                commit: SourceCommit::parse("a".repeat(40))?,
            },
            report_fingerprint: fingerprint('a')?,
            build_fingerprint: fingerprint('b')?,
            toolchain_fingerprint: fingerprint('c')?,
            memory_contract_fingerprint: fingerprint('d')?,
        },
    };
    before.status = MatrixStatus::Failed {
        required_failures: 19,
    };
    let mut after = before.clone();
    if let Verdict::Passed { evidence } = &mut after.targets[0].resource {
        evidence.report_fingerprint = fingerprint('e')?;
    }
    write_matrix(&before_path, &before)?;
    write_matrix(&after_path, &after)?;
    let comparison = compare(&before_path, &after_path)?;
    assert!(comparison.contains("| different | exact | exact | exact |"));
    Ok(())
}

#[test]
fn matrix_report_exposes_failure_counts_and_typed_details() -> Result<(), Box<dyn std::error::Error>>
{
    let mut matrix = assemble(Vec::new(), Vec::new())?;
    matrix.targets[0].resource = Verdict::Failed {
        failure: Failure {
            kind: FailureKind::StructuralViolation,
            diagnostic: "bad | entry\npoint".to_string(),
        },
    };

    let markdown = super::render::matrix(&matrix);

    assert!(markdown.contains("failed (20 required failures)"));
    assert!(markdown.contains("structural-violation: bad \\| entry point"));
    assert!(markdown.contains("evidence-not-produced"));
    assert!(markdown.contains("contract: emulator-does-not-model-platform"));
    Ok(())
}
