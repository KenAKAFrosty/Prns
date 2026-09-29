use std::path::Path;

use thiserror::Error;

use crate::contract::{AssuranceMatrix, CapabilityResult, MatrixStatus, Verdict};
use crate::evidence::{load_matrix, MatrixValidationError};

#[derive(Debug, Error)]
pub enum ComparisonError {
    #[error(transparent)]
    Matrix(#[from] MatrixValidationError),
    #[error("assurance matrices are incompatible in {0}")]
    Incompatible(&'static str),
}

pub fn compare(before: &Path, after: &Path) -> Result<String, ComparisonError> {
    let before_matrix = load_matrix(before)?;
    let after_matrix = load_matrix(after)?;
    require_compatible(&before_matrix, &after_matrix)?;
    Ok(render(before, after, &before_matrix, &after_matrix))
}

fn require_compatible(
    before: &AssuranceMatrix,
    after: &AssuranceMatrix,
) -> Result<(), ComparisonError> {
    let target_contract = before
        .targets
        .iter()
        .map(|target| {
            (
                &target.id,
                &target.memory_profile,
                &target.architecture,
                &target.rust_target,
                &target.architecture_adapter,
            )
        })
        .eq(after.targets.iter().map(|target| {
            (
                &target.id,
                &target.memory_profile,
                &target.architecture,
                &target.rust_target,
                &target.architecture_adapter,
            )
        }));
    if !target_contract {
        return Err(ComparisonError::Incompatible("target contract"));
    }
    let capability_contract = before
        .capabilities
        .iter()
        .map(CapabilityResult::capability)
        .eq(after.capabilities.iter().map(CapabilityResult::capability));
    if capability_contract {
        Ok(())
    } else {
        Err(ComparisonError::Incompatible("capability contract"))
    }
}

fn render(
    before_path: &Path,
    after_path: &Path,
    before: &AssuranceMatrix,
    after: &AssuranceMatrix,
) -> String {
    let mut output = format!(
        "# Embedded assurance comparison\n\n- Before: `{}` ({})\n- After: `{}` ({})\n\n## Canonical targets\n\n| Target | Resource verdict | Report | Build | Toolchain | Memory contract |\n| --- | --- | --- | --- | --- | --- |\n",
        before_path.display(),
        status(&before.status),
        after_path.display(),
        status(&after.status),
    );
    for (before_target, after_target) in before.targets.iter().zip(&after.targets) {
        let (report, build, toolchain, memory) =
            match (&before_target.resource, &after_target.resource) {
                (
                    Verdict::Passed {
                        evidence: before_evidence,
                    },
                    Verdict::Passed {
                        evidence: after_evidence,
                    },
                ) => (
                    relation(
                        &before_evidence.report_fingerprint,
                        &after_evidence.report_fingerprint,
                    ),
                    relation(
                        &before_evidence.build_fingerprint,
                        &after_evidence.build_fingerprint,
                    ),
                    relation(
                        &before_evidence.toolchain_fingerprint,
                        &after_evidence.toolchain_fingerprint,
                    ),
                    relation(
                        &before_evidence.memory_contract_fingerprint,
                        &after_evidence.memory_contract_fingerprint,
                    ),
                ),
                _ => ("n/a", "n/a", "n/a", "n/a"),
            };
        output.push_str(&format!(
            "| `{}` | {} → {} | {} | {} | {} | {} |\n",
            before_target.id,
            crate::report::render::verdict(&before_target.resource),
            crate::report::render::verdict(&after_target.resource),
            report,
            build,
            toolchain,
            memory,
        ));
    }
    output.push_str(
        "\n## Executable proofs\n\n| Subject | Scenario | Verdict | Evidence |\n| --- | --- | --- | --- |\n",
    );
    for (before_result, after_result) in before.capabilities.iter().zip(&after.capabilities) {
        let capability = before_result.capability();
        output.push_str(&format!(
            "| `{}` | `{}` | {} → {} | {} |\n",
            capability.subject,
            capability.scenario,
            capability_verdict(before_result),
            capability_verdict(after_result),
            if before_result == after_result {
                "exact"
            } else {
                "different"
            },
        ));
    }
    output
}

fn capability_verdict(result: &CapabilityResult) -> &'static str {
    match result {
        CapabilityResult::Observed { proof, .. } => crate::report::render::verdict(&proof.verdict),
        CapabilityResult::Unavailable { .. } => "unavailable",
    }
}

fn status(value: &MatrixStatus) -> &'static str {
    crate::report::render::matrix_status(value)
}

fn relation<T: PartialEq>(before: &T, after: &T) -> &'static str {
    if before == after {
        "exact"
    } else {
        "different"
    }
}
