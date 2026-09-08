use std::path::{Path, PathBuf};

use personal_hopspot_builder::artifact::publish;
use personal_hopspot_builder::BuildError;
use thiserror::Error;

use crate::contract::{
    CapabilityResult, MatrixStatus, MiriCoverage, ProofEvidence, SourceCommit, SourceCustody,
    Verdict,
};
use crate::evidence::{load_canonical_matrix, MatrixValidationError};

const BASELINE_PATH: &str = "personal-hopspot/assurance/baseline/canonical.json";

#[derive(Debug, Error)]
pub enum BaselineError {
    #[error(transparent)]
    Matrix(#[from] MatrixValidationError),
    #[error("assurance baseline requires a matrix with every required check passing")]
    RequiredEvidenceFailed,
    #[error("assurance baseline requires clean-commit proof custody")]
    WorkingTreeEvidence,
    #[error("assurance baseline combines proof evidence from multiple commits")]
    MixedSourceCommits,
    #[error("assurance baseline requires stacked-and-tree Miri coverage")]
    IncompleteMiriCoverage,
    #[error("could not serialize assurance baseline: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("could not publish assurance baseline: {0}")]
    Publish(#[from] BuildError),
}

pub struct BaselineOutcome {
    path: PathBuf,
    targets: usize,
    capabilities: usize,
}

impl BaselineOutcome {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub const fn targets(&self) -> usize {
        self.targets
    }

    #[must_use]
    pub const fn capabilities(&self) -> usize {
        self.capabilities
    }
}

pub fn path(repository: &Path) -> PathBuf {
    repository.join(BASELINE_PATH)
}

pub fn refresh(matrix_path: &Path, destination: &Path) -> Result<BaselineOutcome, BaselineError> {
    let matrix = load_canonical_matrix(matrix_path)?;
    if !matches!(matrix.status, MatrixStatus::Passed) {
        return Err(BaselineError::RequiredEvidenceFailed);
    }
    validate_proof_custody(&matrix.capabilities)?;
    let mut bytes = serde_json::to_vec_pretty(&matrix)?;
    bytes.push(b'\n');
    publish(destination, &bytes)?;
    Ok(BaselineOutcome {
        path: destination.to_path_buf(),
        targets: matrix.targets.len(),
        capabilities: matrix.capabilities.len(),
    })
}

fn validate_proof_custody(results: &[CapabilityResult]) -> Result<(), BaselineError> {
    let mut commit: Option<&SourceCommit> = None;
    for result in results {
        let CapabilityResult::Observed { proof, .. } = result else {
            continue;
        };
        let SourceCustody::CleanCommit {
            commit: proof_commit,
        } = &proof.source.custody
        else {
            return Err(BaselineError::WorkingTreeEvidence);
        };
        if commit.is_some_and(|commit| commit != proof_commit) {
            return Err(BaselineError::MixedSourceCommits);
        }
        commit = Some(proof_commit);
        if matches!(
            &proof.verdict,
            Verdict::Passed {
                evidence: ProofEvidence::Miri {
                    coverage: MiriCoverage::Stacked,
                    ..
                }
            }
        ) {
            return Err(BaselineError::IncompleteMiriCoverage);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{refresh, validate_proof_custody, BaselineError};
    use crate::contract::{
        Capability, CapabilityResult, ComponentId, EvidenceFingerprint, MiriCoverage,
        ProofEvidence, ProofFragment, ProofKind, RunnerId, ScenarioId, SourceCommit, SourceCustody,
        SourceIdentity, Subject, SupportLevel, ToolIdentity, ToolKind, Verdict,
        PROOF_FRAGMENT_SCHEMA_VERSION,
    };
    use crate::evidence::assemble;

    fn observed_miri(
        component: &str,
        commit: char,
        custody: fn(SourceCommit, EvidenceFingerprint) -> SourceCustody,
        coverage: MiriCoverage,
    ) -> Result<CapabilityResult, Box<dyn std::error::Error>> {
        let subject = Subject::Component(ComponentId::parse(component)?);
        let scenario = ScenarioId::parse(format!("{component}-state-machine"))?;
        let source_commit = SourceCommit::parse(commit.to_string().repeat(40))?;
        let diff_fingerprint = EvidenceFingerprint::parse("d".repeat(64))?;
        Ok(CapabilityResult::Observed {
            capability: Capability {
                subject: subject.clone(),
                scenario: scenario.clone(),
                proof: ProofKind::Miri,
                support: SupportLevel::Required,
            },
            proof: Box::new(ProofFragment {
                schema_version: PROOF_FRAGMENT_SCHEMA_VERSION,
                subject,
                scenario,
                proof: ProofKind::Miri,
                runner: RunnerId::parse("miri")?,
                source: SourceIdentity {
                    custody: custody(source_commit, diff_fingerprint),
                    scenario_fingerprint: EvidenceFingerprint::parse("e".repeat(64))?,
                },
                tools: vec![ToolIdentity {
                    kind: ToolKind::Miri,
                    version: "miri 1".to_string(),
                }],
                verdict: Verdict::Passed {
                    evidence: ProofEvidence::Miri {
                        coverage,
                        completed_tests: 1,
                    },
                },
                artifacts: Vec::new(),
            }),
        })
    }

    fn clean(commit: SourceCommit, _: EvidenceFingerprint) -> SourceCustody {
        SourceCustody::CleanCommit { commit }
    }

    fn working_tree(commit: SourceCommit, diff_fingerprint: EvidenceFingerprint) -> SourceCustody {
        SourceCustody::WorkingTree {
            head: commit,
            diff_fingerprint,
        }
    }

    #[test]
    fn incomplete_matrix_cannot_replace_the_baseline() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let matrix_path = directory.path().join("matrix.json");
        let baseline_path = directory.path().join("baseline.json");
        fs::write(
            &matrix_path,
            serde_json::to_vec_pretty(&assemble(Vec::new(), Vec::new())?)?,
        )?;
        assert!(matches!(
            refresh(&matrix_path, &baseline_path),
            Err(BaselineError::RequiredEvidenceFailed)
        ));
        assert!(!baseline_path.exists());
        Ok(())
    }

    #[test]
    fn canonical_baseline_requires_full_miri_coverage() -> Result<(), Box<dyn std::error::Error>> {
        let results = vec![observed_miri("sx126x", 'a', clean, MiriCoverage::Stacked)?];
        assert!(matches!(
            validate_proof_custody(&results),
            Err(BaselineError::IncompleteMiriCoverage)
        ));
        Ok(())
    }

    #[test]
    fn canonical_baseline_rejects_working_tree_evidence() -> Result<(), Box<dyn std::error::Error>>
    {
        let results = vec![observed_miri(
            "sx126x",
            'a',
            working_tree,
            MiriCoverage::StackedAndTree,
        )?];
        assert!(matches!(
            validate_proof_custody(&results),
            Err(BaselineError::WorkingTreeEvidence)
        ));
        Ok(())
    }

    #[test]
    fn canonical_baseline_rejects_mixed_commits() -> Result<(), Box<dyn std::error::Error>> {
        let results = vec![
            observed_miri("sx126x", 'a', clean, MiriCoverage::StackedAndTree)?,
            observed_miri("lr1110", 'b', clean, MiriCoverage::StackedAndTree)?,
        ];
        assert!(matches!(
            validate_proof_custody(&results),
            Err(BaselineError::MixedSourceCommits)
        ));
        Ok(())
    }
}
