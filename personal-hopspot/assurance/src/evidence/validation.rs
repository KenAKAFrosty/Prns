use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use personal_hopspot_resources::report::SCHEMA_VERSION as RESOURCE_SCHEMA_VERSION;
use thiserror::Error;

use crate::contract::{
    AssuranceMatrix, CapabilityResult, MatrixStatus, ProofContractError, SupportLevel, TargetId,
    UnavailableReason, Verdict, ASSURANCE_MATRIX_SCHEMA_VERSION,
};

use super::{assemble, AggregateError};

#[derive(Debug, Error)]
pub enum MatrixValidationError {
    #[error("could not read assurance matrix {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not parse assurance matrix {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("assurance matrix uses schema {actual}, expected {expected}")]
    UnsupportedSchema { actual: u32, expected: u32 },
    #[error("assurance matrix contains resource schema {actual}, expected {expected}")]
    UnsupportedResourceSchema { actual: u32, expected: u32 },
    #[error("assurance matrix repeats target {0}")]
    DuplicateTarget(TargetId),
    #[error("assurance matrix has an empty {dimension} for target {target}")]
    EmptyTargetIdentity {
        target: TargetId,
        dimension: &'static str,
    },
    #[error("assurance matrix repeats capability for {0:?}")]
    DuplicateCapability(crate::contract::Subject),
    #[error("observed proof does not match its declared capability")]
    MismatchedCapabilityProof,
    #[error("proof for {subject:?} violates its contract: {source}")]
    InvalidProof {
        subject: crate::contract::Subject,
        #[source]
        source: ProofContractError,
    },
    #[error("unsupported or inapplicable capability contains observed evidence")]
    EvidenceForExcludedCapability,
    #[error("excluded capability has an incorrect unavailable reason")]
    IncorrectExcludedReason,
    #[error("assurance matrix status does not match {required_failures} required failures")]
    IncorrectStatus { required_failures: usize },
    #[error("could not resolve the canonical assurance contract: {0}")]
    CanonicalContract(#[from] AggregateError),
    #[error("assurance matrix target contract does not match the canonical matrix")]
    IncompatibleTargetContract,
    #[error("assurance matrix capability contract does not match the canonical registry")]
    IncompatibleCapabilityContract,
}

pub fn load_matrix(path: &Path) -> Result<AssuranceMatrix, MatrixValidationError> {
    let bytes = fs::read(path).map_err(|source| MatrixValidationError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let matrix = serde_json::from_slice::<AssuranceMatrix>(&bytes).map_err(|source| {
        MatrixValidationError::Parse {
            path: path.to_path_buf(),
            source,
        }
    })?;
    validate(&matrix)?;
    Ok(matrix)
}

pub fn load_canonical_matrix(path: &Path) -> Result<AssuranceMatrix, MatrixValidationError> {
    let matrix = load_matrix(path)?;
    validate_canonical_contract(&matrix)?;
    Ok(matrix)
}

pub fn validate_canonical(matrix: &AssuranceMatrix) -> Result<(), MatrixValidationError> {
    validate(matrix)?;
    validate_canonical_contract(matrix)
}

pub fn validate(matrix: &AssuranceMatrix) -> Result<(), MatrixValidationError> {
    if matrix.schema_version != ASSURANCE_MATRIX_SCHEMA_VERSION {
        return Err(MatrixValidationError::UnsupportedSchema {
            actual: matrix.schema_version,
            expected: ASSURANCE_MATRIX_SCHEMA_VERSION,
        });
    }
    if matrix.resource_report_schema_version != RESOURCE_SCHEMA_VERSION {
        return Err(MatrixValidationError::UnsupportedResourceSchema {
            actual: matrix.resource_report_schema_version,
            expected: RESOURCE_SCHEMA_VERSION,
        });
    }
    let mut targets = BTreeSet::new();
    for target in &matrix.targets {
        if !targets.insert(target.id.clone()) {
            return Err(MatrixValidationError::DuplicateTarget(target.id.clone()));
        }
        for (value, dimension) in [
            (target.display_name.as_str(), "display name"),
            (target.memory_profile.as_str(), "memory profile"),
            (target.rust_target.as_str(), "Rust target"),
            (target.architecture_adapter.as_str(), "architecture adapter"),
        ] {
            if value.trim().is_empty() {
                return Err(MatrixValidationError::EmptyTargetIdentity {
                    target: target.id.clone(),
                    dimension,
                });
            }
        }
    }
    let mut capabilities = BTreeSet::new();
    for result in &matrix.capabilities {
        let capability = result.capability();
        let key = (
            capability.subject.clone(),
            capability.scenario.clone(),
            capability.proof,
        );
        if !capabilities.insert(key) {
            return Err(MatrixValidationError::DuplicateCapability(
                capability.subject.clone(),
            ));
        }
        validate_capability(result)?;
    }
    let required_failures = count_required_failures(matrix);
    let status_matches = match matrix.status {
        MatrixStatus::Passed => required_failures == 0,
        MatrixStatus::Failed {
            required_failures: recorded,
        } => required_failures != 0 && recorded == required_failures,
    };
    if !status_matches {
        Err(MatrixValidationError::IncorrectStatus { required_failures })
    } else {
        Ok(())
    }
}

fn validate_canonical_contract(matrix: &AssuranceMatrix) -> Result<(), MatrixValidationError> {
    let canonical = assemble(Vec::new(), Vec::new())?;
    let targets_match = matrix
        .targets
        .iter()
        .map(target_contract)
        .eq(canonical.targets.iter().map(target_contract));
    if !targets_match {
        return Err(MatrixValidationError::IncompatibleTargetContract);
    }
    let capabilities_match = matrix
        .capabilities
        .iter()
        .map(CapabilityResult::capability)
        .eq(canonical
            .capabilities
            .iter()
            .map(CapabilityResult::capability));
    if capabilities_match {
        Ok(())
    } else {
        Err(MatrixValidationError::IncompatibleCapabilityContract)
    }
}

fn target_contract(
    target: &crate::contract::TargetEvidence,
) -> (
    &TargetId,
    &str,
    &str,
    &crate::contract::ArchitectureId,
    &str,
    &str,
) {
    (
        &target.id,
        &target.display_name,
        &target.memory_profile,
        &target.architecture,
        &target.rust_target,
        &target.architecture_adapter,
    )
}

fn validate_capability(result: &CapabilityResult) -> Result<(), MatrixValidationError> {
    let capability = result.capability();
    match result {
        CapabilityResult::Observed { proof, .. } => {
            if matches!(
                capability.support,
                SupportLevel::Unsupported(_) | SupportLevel::NotApplicable(_)
            ) {
                return Err(MatrixValidationError::EvidenceForExcludedCapability);
            }
            if capability.subject != proof.subject
                || capability.scenario != proof.scenario
                || capability.proof != proof.proof
            {
                return Err(MatrixValidationError::MismatchedCapabilityProof);
            }
            proof
                .validate()
                .map_err(|source| MatrixValidationError::InvalidProof {
                    subject: proof.subject.clone(),
                    source,
                })
        }
        CapabilityResult::Unavailable { reason, .. } => {
            let excluded = matches!(
                capability.support,
                SupportLevel::Unsupported(_) | SupportLevel::NotApplicable(_)
            );
            if excluded == matches!(reason, UnavailableReason::UnsupportedByContract) {
                Ok(())
            } else {
                Err(MatrixValidationError::IncorrectExcludedReason)
            }
        }
    }
}

fn count_required_failures(matrix: &AssuranceMatrix) -> usize {
    let target_failures = matrix
        .targets
        .iter()
        .filter(|target| !matches!(target.resource, Verdict::Passed { .. }))
        .count();
    let proof_failures = matrix
        .capabilities
        .iter()
        .filter(|result| matches!(result.capability().support, SupportLevel::Required))
        .filter(|result| {
            !matches!(
                result,
                CapabilityResult::Observed { proof, .. }
                    if matches!(proof.verdict, Verdict::Passed { .. })
            )
        })
        .count();
    target_failures.saturating_add(proof_failures)
}

#[cfg(test)]
mod tests {
    use super::{validate, validate_canonical, MatrixValidationError};
    use crate::contract::MatrixStatus;
    use crate::evidence::assemble;

    #[test]
    fn duplicate_target_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let mut matrix = assemble(Vec::new(), Vec::new())?;
        matrix.targets.push(matrix.targets[0].clone());
        assert!(matches!(
            validate(&matrix),
            Err(MatrixValidationError::DuplicateTarget(_))
        ));
        Ok(())
    }

    #[test]
    fn recorded_status_must_match_required_failures() -> Result<(), Box<dyn std::error::Error>> {
        let mut matrix = assemble(Vec::new(), Vec::new())?;
        matrix.status = MatrixStatus::Passed;
        assert!(matches!(
            validate(&matrix),
            Err(MatrixValidationError::IncorrectStatus {
                required_failures: 20,
            })
        ));
        Ok(())
    }

    #[test]
    fn passed_matrix_cannot_omit_canonical_targets() -> Result<(), Box<dyn std::error::Error>> {
        let mut matrix = assemble(Vec::new(), Vec::new())?;
        matrix.targets.clear();
        matrix.capabilities.clear();
        matrix.status = MatrixStatus::Passed;
        assert!(matches!(
            validate_canonical(&matrix),
            Err(MatrixValidationError::IncompatibleTargetContract)
        ));
        Ok(())
    }

    #[test]
    fn matrix_cannot_omit_canonical_capabilities() -> Result<(), Box<dyn std::error::Error>> {
        let mut matrix = assemble(Vec::new(), Vec::new())?;
        matrix.capabilities.clear();
        matrix.status = MatrixStatus::Failed {
            required_failures: matrix.targets.len(),
        };
        assert!(matches!(
            validate_canonical(&matrix),
            Err(MatrixValidationError::IncompatibleCapabilityContract)
        ));
        Ok(())
    }

    #[test]
    fn malformed_fingerprint_is_rejected_during_decode() -> Result<(), Box<dyn std::error::Error>> {
        let matrix = assemble(Vec::new(), Vec::new())?;
        let mut value = serde_json::to_value(matrix)?;
        value["targets"][0]["resource"] = serde_json::json!({
            "kind": "passed",
            "evidence": {
                "report_fingerprint": "invalid",
                "build_fingerprint": "a".repeat(64),
                "toolchain_fingerprint": "b".repeat(64),
                "memory_contract_fingerprint": "c".repeat(64)
            }
        });
        assert!(serde_json::from_value::<crate::contract::AssuranceMatrix>(value).is_err());
        Ok(())
    }
}
