use std::collections::BTreeSet;
use std::fmt;

pub use personal_hopspot_builder::SourceCustody;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{
    ArchitectureId, ComponentId, EvidenceFingerprint, EvidencePath, PlatformId, RunnerId,
    ScenarioId, TargetId,
};

pub const PROOF_FRAGMENT_SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "kebab-case")]
pub enum Subject {
    Architecture(ArchitectureId),
    Component(ComponentId),
    Platform(PlatformId),
    Target(TargetId),
}

impl fmt::Display for Subject {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Architecture(id) => write!(formatter, "architecture:{id}"),
            Self::Component(id) => write!(formatter, "component:{id}"),
            Self::Platform(id) => write!(formatter, "platform:{id}"),
            Self::Target(id) => write!(formatter, "target:{id}"),
        }
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProofKind {
    Miri,
    PlatformEmulation,
    TargetIsa,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceIdentity {
    pub custody: SourceCustody,
    pub scenario_fingerprint: EvidenceFingerprint,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolKind {
    Cargo,
    Linker,
    Miri,
    Qemu,
    Renode,
    Rustc,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolIdentity {
    pub kind: ToolKind,
    pub version: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MiriCoverage {
    Stacked,
    StackedAndTree,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlatformMilestone {
    ApplicationEntry,
    RuntimeInitialized,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ProofEvidence {
    Miri {
        coverage: MiriCoverage,
        completed_tests: u32,
    },
    PlatformEmulation {
        platform: PlatformId,
        milestone: PlatformMilestone,
        transcript_fingerprint: EvidenceFingerprint,
    },
    TargetIsa {
        architecture: ArchitectureId,
        completed_scenarios: u32,
        transcript_fingerprint: EvidenceFingerprint,
    },
}

impl ProofEvidence {
    #[must_use]
    pub const fn kind(&self) -> ProofKind {
        match self {
            Self::Miri { .. } => ProofKind::Miri,
            Self::PlatformEmulation { .. } => ProofKind::PlatformEmulation,
            Self::TargetIsa { .. } => ProofKind::TargetIsa,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailureKind {
    Crash,
    MemoryOverflow,
    ScenarioMismatch,
    StructuralViolation,
    Timeout,
    ToolFailure,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Failure {
    pub kind: FailureKind,
    pub diagnostic: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceGap {
    Assembly,
    IndirectCall,
    InterruptNesting,
    MissingMetadata,
    MissingRoot,
    UnsupportedPeripheral,
    VendorObject,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UnavailableReason {
    EvidenceNotProduced,
    EmulatorUnavailable,
    RunnerNotInstalled,
    ToolchainUnavailable,
    UnsupportedByContract,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Verdict<T> {
    Passed { evidence: T },
    Failed { failure: Failure },
    Partial { evidence: T, gaps: Vec<EvidenceGap> },
    Unavailable { reason: UnavailableReason },
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProofArtifactKind {
    Executable,
    Log,
    Transcript,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceArtifact {
    pub kind: ProofArtifactKind,
    pub path: EvidencePath,
    pub bytes: u64,
    pub fingerprint: EvidenceFingerprint,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProofFragment {
    pub schema_version: u32,
    pub subject: Subject,
    pub scenario: ScenarioId,
    pub proof: ProofKind,
    pub runner: RunnerId,
    pub source: SourceIdentity,
    pub tools: Vec<ToolIdentity>,
    pub verdict: Verdict<ProofEvidence>,
    pub artifacts: Vec<EvidenceArtifact>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProofContractError {
    #[error("proof fragment uses schema {actual}, expected {expected}")]
    UnsupportedSchema { actual: u32, expected: u32 },
    #[error("proof fragment has no tool identities")]
    MissingTools,
    #[error("proof fragment has an empty tool version for {0:?}")]
    EmptyToolVersion(ToolKind),
    #[error("proof fragment repeats tool identity {0:?}")]
    DuplicateTool(ToolKind),
    #[error("{proof:?} proof fragment is missing required tool identity {tool:?}")]
    MissingRequiredTool { proof: ProofKind, tool: ToolKind },
    #[error("{proof:?} proof fragment contains unexpected tool identity {tool:?}")]
    UnexpectedTool { proof: ProofKind, tool: ToolKind },
    #[error("platform-emulation proof fragment must identify exactly one emulator")]
    PlatformEmulatorIdentity,
    #[error("proof fragment uses runner {actual:?}, expected {expected:?}")]
    RunnerMismatch {
        actual: RunnerId,
        expected: &'static str,
    },
    #[error("proof fragment has an empty failure diagnostic")]
    EmptyFailureDiagnostic,
    #[error("partial proof fragment has no evidence gaps")]
    MissingEvidenceGaps,
    #[error("proof fragment declares {declared:?} but contains {actual:?} evidence")]
    EvidenceKindMismatch {
        declared: ProofKind,
        actual: ProofKind,
    },
    #[error("proof fragment evidence does not belong to subject {0:?}")]
    EvidenceSubjectMismatch(Subject),
    #[error("proof fragment contains an empty artifact")]
    EmptyArtifact,
    #[error("proof fragment repeats artifact {0}")]
    DuplicateArtifact(EvidencePath),
    #[error("proof fragment has no log artifact")]
    MissingLogArtifact,
    #[error("{proof:?} proof fragment contains unexpected artifact kind {artifact:?}")]
    UnexpectedArtifact {
        proof: ProofKind,
        artifact: ProofArtifactKind,
    },
    #[error("executable proof fragment has no transcript artifact")]
    MissingTranscriptArtifact,
    #[error("executable proof fragment has multiple transcript artifacts")]
    DuplicateTranscriptArtifact,
    #[error("executable proof transcript fingerprint does not match its artifact")]
    TranscriptFingerprintMismatch,
    #[error("executable proof fragment has no executable artifact")]
    MissingExecutableArtifact,
    #[error("executable proof fragment has multiple executable artifacts")]
    DuplicateExecutableArtifact,
}

impl ProofFragment {
    pub fn validate(&self) -> Result<(), ProofContractError> {
        if self.schema_version != PROOF_FRAGMENT_SCHEMA_VERSION {
            return Err(ProofContractError::UnsupportedSchema {
                actual: self.schema_version,
                expected: PROOF_FRAGMENT_SCHEMA_VERSION,
            });
        }
        validate_verdict(self.proof, &self.subject, &self.verdict)?;
        validate_runner(&self.runner, &self.verdict)?;
        validate_tools(self.proof, &self.verdict, &self.tools)?;
        validate_artifacts(&self.artifacts)?;
        validate_proof_artifacts(self.proof, &self.verdict, &self.artifacts)
    }
}

fn validate_runner(
    runner: &RunnerId,
    verdict: &Verdict<ProofEvidence>,
) -> Result<(), ProofContractError> {
    let evidence = match verdict {
        Verdict::Passed { evidence } | Verdict::Partial { evidence, .. } => evidence,
        Verdict::Failed { .. } | Verdict::Unavailable { .. } => return Ok(()),
    };
    let expected = match evidence {
        ProofEvidence::Miri {
            coverage: MiriCoverage::Stacked,
            ..
        } => Some("miri-stacked"),
        ProofEvidence::Miri {
            coverage: MiriCoverage::StackedAndTree,
            ..
        } => Some("miri-stacked-tree"),
        ProofEvidence::PlatformEmulation { .. } | ProofEvidence::TargetIsa { .. } => None,
    };
    if let Some(expected) = expected {
        if runner.as_str() != expected {
            return Err(ProofContractError::RunnerMismatch {
                actual: runner.clone(),
                expected,
            });
        }
    }
    Ok(())
}

fn validate_tools(
    proof: ProofKind,
    verdict: &Verdict<ProofEvidence>,
    tools: &[ToolIdentity],
) -> Result<(), ProofContractError> {
    if tools.is_empty() {
        return Err(ProofContractError::MissingTools);
    }
    let mut kinds = BTreeSet::new();
    for tool in tools {
        if tool.version.trim().is_empty() {
            return Err(ProofContractError::EmptyToolVersion(tool.kind));
        }
        if !kinds.insert(tool.kind) {
            return Err(ProofContractError::DuplicateTool(tool.kind));
        }
    }
    let allowed = allowed_tools(proof);
    if let Some(tool) = kinds.iter().find(|tool| !allowed.contains(tool)) {
        return Err(ProofContractError::UnexpectedTool { proof, tool: *tool });
    }
    if matches!(verdict, Verdict::Unavailable { .. }) {
        return Ok(());
    }
    for tool in required_tools(proof) {
        if !kinds.contains(tool) {
            return Err(ProofContractError::MissingRequiredTool { proof, tool: *tool });
        }
    }
    if matches!(proof, ProofKind::PlatformEmulation) {
        let emulators = [ToolKind::Qemu, ToolKind::Renode]
            .into_iter()
            .filter(|tool| kinds.contains(tool))
            .count();
        if emulators != 1 {
            return Err(ProofContractError::PlatformEmulatorIdentity);
        }
    }
    Ok(())
}

const fn allowed_tools(proof: ProofKind) -> &'static [ToolKind] {
    match proof {
        ProofKind::Miri => &[ToolKind::Rustc, ToolKind::Miri],
        ProofKind::TargetIsa => &[ToolKind::Cargo, ToolKind::Rustc, ToolKind::Qemu],
        ProofKind::PlatformEmulation => &[
            ToolKind::Cargo,
            ToolKind::Rustc,
            ToolKind::Linker,
            ToolKind::Qemu,
            ToolKind::Renode,
        ],
    }
}

const fn required_tools(proof: ProofKind) -> &'static [ToolKind] {
    match proof {
        ProofKind::Miri => &[ToolKind::Rustc, ToolKind::Miri],
        ProofKind::TargetIsa => &[ToolKind::Cargo, ToolKind::Rustc, ToolKind::Qemu],
        ProofKind::PlatformEmulation => &[ToolKind::Cargo, ToolKind::Rustc],
    }
}

fn validate_verdict(
    declared: ProofKind,
    subject: &Subject,
    verdict: &Verdict<ProofEvidence>,
) -> Result<(), ProofContractError> {
    match verdict {
        Verdict::Passed { evidence } => validate_evidence(declared, subject, evidence),
        Verdict::Failed { failure } => {
            if failure.diagnostic.trim().is_empty() {
                Err(ProofContractError::EmptyFailureDiagnostic)
            } else {
                Ok(())
            }
        }
        Verdict::Partial { evidence, gaps } => {
            if gaps.is_empty() {
                return Err(ProofContractError::MissingEvidenceGaps);
            }
            validate_evidence(declared, subject, evidence)
        }
        Verdict::Unavailable { .. } => Ok(()),
    }
}

fn validate_evidence(
    declared: ProofKind,
    subject: &Subject,
    evidence: &ProofEvidence,
) -> Result<(), ProofContractError> {
    if evidence.kind() != declared {
        return Err(ProofContractError::EvidenceKindMismatch {
            declared,
            actual: evidence.kind(),
        });
    }
    let matches = match (subject, evidence) {
        (
            Subject::Component(_),
            ProofEvidence::Miri {
                completed_tests, ..
            },
        ) => *completed_tests > 0,
        (
            Subject::Architecture(subject),
            ProofEvidence::TargetIsa {
                architecture,
                completed_scenarios,
                ..
            },
        ) => subject == architecture && *completed_scenarios > 0,
        (Subject::Platform(subject), ProofEvidence::PlatformEmulation { platform, .. }) => {
            subject == platform
        }
        _ => false,
    };
    if matches {
        Ok(())
    } else {
        Err(ProofContractError::EvidenceSubjectMismatch(subject.clone()))
    }
}

fn validate_artifacts(artifacts: &[EvidenceArtifact]) -> Result<(), ProofContractError> {
    let mut paths = BTreeSet::new();
    for artifact in artifacts {
        if artifact.bytes == 0 {
            return Err(ProofContractError::EmptyArtifact);
        }
        if !paths.insert(artifact.path.clone()) {
            return Err(ProofContractError::DuplicateArtifact(artifact.path.clone()));
        }
    }
    Ok(())
}

fn validate_proof_artifacts(
    proof: ProofKind,
    verdict: &Verdict<ProofEvidence>,
    artifacts: &[EvidenceArtifact],
) -> Result<(), ProofContractError> {
    let evidence = match verdict {
        Verdict::Passed { evidence } | Verdict::Partial { evidence, .. } => evidence,
        Verdict::Failed { .. } => return require_log(artifacts),
        Verdict::Unavailable { .. } => return Ok(()),
    };
    let fingerprint = match evidence {
        ProofEvidence::TargetIsa {
            transcript_fingerprint,
            ..
        }
        | ProofEvidence::PlatformEmulation {
            transcript_fingerprint,
            ..
        } => transcript_fingerprint,
        ProofEvidence::Miri { .. } => {
            if let Some(artifact) = artifacts
                .iter()
                .find(|artifact| artifact.kind != ProofArtifactKind::Log)
            {
                return Err(ProofContractError::UnexpectedArtifact {
                    proof,
                    artifact: artifact.kind,
                });
            }
            return require_log(artifacts);
        }
    };
    require_log(artifacts)?;
    let mut transcripts = artifacts
        .iter()
        .filter(|artifact| artifact.kind == ProofArtifactKind::Transcript);
    let Some(transcript) = transcripts.next() else {
        return Err(ProofContractError::MissingTranscriptArtifact);
    };
    if transcripts.next().is_some() {
        return Err(ProofContractError::DuplicateTranscriptArtifact);
    }
    if transcript.fingerprint != *fingerprint {
        return Err(ProofContractError::TranscriptFingerprintMismatch);
    }
    let mut executables = artifacts
        .iter()
        .filter(|artifact| artifact.kind == ProofArtifactKind::Executable);
    if executables.next().is_none() {
        return Err(ProofContractError::MissingExecutableArtifact);
    }
    if executables.next().is_some() {
        return Err(ProofContractError::DuplicateExecutableArtifact);
    }
    Ok(())
}

fn require_log(artifacts: &[EvidenceArtifact]) -> Result<(), ProofContractError> {
    if artifacts
        .iter()
        .any(|artifact| artifact.kind == ProofArtifactKind::Log)
    {
        Ok(())
    } else {
        Err(ProofContractError::MissingLogArtifact)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ArchitectureId, ComponentId, EvidenceArtifact, EvidenceFingerprint, EvidenceGap,
        EvidencePath, Failure, FailureKind, MiriCoverage, ProofArtifactKind, ProofContractError,
        ProofEvidence, ProofFragment, ProofKind, RunnerId, ScenarioId, SourceCustody,
        SourceIdentity, Subject, ToolIdentity, ToolKind, Verdict, PROOF_FRAGMENT_SCHEMA_VERSION,
    };
    use personal_hopspot_builder::RepositoryCommit;

    fn fingerprint(byte: char) -> Result<EvidenceFingerprint, crate::contract::ValueError> {
        EvidenceFingerprint::parse(byte.to_string().repeat(64))
    }

    fn fragment() -> Result<ProofFragment, Box<dyn std::error::Error>> {
        Ok(ProofFragment {
            schema_version: PROOF_FRAGMENT_SCHEMA_VERSION,
            subject: Subject::Component(ComponentId::parse("sx126x")?),
            scenario: ScenarioId::parse("sx126x-state-machine")?,
            proof: ProofKind::Miri,
            runner: RunnerId::parse("miri-stacked")?,
            source: SourceIdentity {
                custody: SourceCustody::CleanCommit {
                    commit: RepositoryCommit::parse("a".repeat(40))?,
                },
                scenario_fingerprint: fingerprint('b')?,
            },
            tools: vec![
                ToolIdentity {
                    kind: ToolKind::Rustc,
                    version: "rustc 1".to_string(),
                },
                ToolIdentity {
                    kind: ToolKind::Miri,
                    version: "miri 1".to_string(),
                },
            ],
            verdict: Verdict::Passed {
                evidence: ProofEvidence::Miri {
                    coverage: MiriCoverage::Stacked,
                    completed_tests: 4,
                },
            },
            artifacts: vec![EvidenceArtifact {
                kind: ProofArtifactKind::Log,
                path: EvidencePath::parse("miri.log")?,
                bytes: 4,
                fingerprint: fingerprint('c')?,
            }],
        })
    }

    #[test]
    fn valid_component_proof_satisfies_the_contract() -> Result<(), Box<dyn std::error::Error>> {
        fragment()?.validate()?;
        Ok(())
    }

    #[test]
    fn partial_proof_requires_a_named_gap() -> Result<(), Box<dyn std::error::Error>> {
        let mut fragment = fragment()?;
        fragment.verdict = Verdict::Partial {
            evidence: ProofEvidence::Miri {
                coverage: MiriCoverage::StackedAndTree,
                completed_tests: 4,
            },
            gaps: Vec::new(),
        };
        assert_eq!(
            fragment.validate(),
            Err(ProofContractError::MissingEvidenceGaps)
        );
        if let Verdict::Partial { gaps, .. } = &mut fragment.verdict {
            gaps.push(EvidenceGap::MissingMetadata);
        }
        fragment.runner = RunnerId::parse("miri-stacked-tree")?;
        fragment.validate()?;
        Ok(())
    }

    #[test]
    fn evidence_kind_and_subject_must_match() -> Result<(), Box<dyn std::error::Error>> {
        let mut fragment = fragment()?;
        fragment.verdict = Verdict::Passed {
            evidence: ProofEvidence::TargetIsa {
                architecture: ArchitectureId::parse("thumbv7em")?,
                completed_scenarios: 1,
                transcript_fingerprint: fingerprint('c')?,
            },
        };
        assert!(matches!(
            fragment.validate(),
            Err(ProofContractError::EvidenceKindMismatch {
                declared: ProofKind::Miri,
                actual: ProofKind::TargetIsa,
            })
        ));
        fragment.proof = ProofKind::TargetIsa;
        assert!(matches!(
            fragment.validate(),
            Err(ProofContractError::EvidenceSubjectMismatch(_))
        ));
        Ok(())
    }

    #[test]
    fn tool_identities_are_unique_and_nonempty() -> Result<(), Box<dyn std::error::Error>> {
        let mut fragment = fragment()?;
        fragment.tools.push(ToolIdentity {
            kind: ToolKind::Miri,
            version: "miri 2".to_string(),
        });
        assert_eq!(
            fragment.validate(),
            Err(ProofContractError::DuplicateTool(ToolKind::Miri))
        );
        Ok(())
    }

    #[test]
    fn proof_kinds_require_their_exact_toolchain() -> Result<(), Box<dyn std::error::Error>> {
        let mut fragment = fragment()?;
        fragment.tools.retain(|tool| tool.kind != ToolKind::Rustc);
        assert_eq!(
            fragment.validate(),
            Err(ProofContractError::MissingRequiredTool {
                proof: ProofKind::Miri,
                tool: ToolKind::Rustc,
            })
        );
        fragment.tools.push(ToolIdentity {
            kind: ToolKind::Rustc,
            version: "rustc 1".to_string(),
        });
        fragment.tools.push(ToolIdentity {
            kind: ToolKind::Qemu,
            version: "qemu 1".to_string(),
        });
        assert_eq!(
            fragment.validate(),
            Err(ProofContractError::UnexpectedTool {
                proof: ProofKind::Miri,
                tool: ToolKind::Qemu,
            })
        );
        Ok(())
    }

    #[test]
    fn successful_proofs_require_logs() -> Result<(), Box<dyn std::error::Error>> {
        let mut fragment = fragment()?;
        fragment.artifacts.clear();
        assert_eq!(
            fragment.validate(),
            Err(ProofContractError::MissingLogArtifact)
        );
        Ok(())
    }

    #[test]
    fn failed_proofs_retain_toolchain_and_log_evidence() -> Result<(), Box<dyn std::error::Error>> {
        let mut fragment = fragment()?;
        fragment.verdict = Verdict::Failed {
            failure: Failure {
                kind: FailureKind::ToolFailure,
                diagnostic: "Miri failed".to_string(),
            },
        };
        fragment.artifacts.clear();
        assert_eq!(
            fragment.validate(),
            Err(ProofContractError::MissingLogArtifact)
        );
        fragment.tools.retain(|tool| tool.kind != ToolKind::Rustc);
        assert_eq!(
            fragment.validate(),
            Err(ProofContractError::MissingRequiredTool {
                proof: ProofKind::Miri,
                tool: ToolKind::Rustc,
            })
        );
        Ok(())
    }

    #[test]
    fn target_isa_proofs_require_qemu_identity() -> Result<(), Box<dyn std::error::Error>> {
        let mut fragment = fragment()?;
        fragment.subject = Subject::Architecture(ArchitectureId::parse("thumbv7em")?);
        fragment.proof = ProofKind::TargetIsa;
        fragment.tools = vec![
            ToolIdentity {
                kind: ToolKind::Cargo,
                version: "cargo 1".to_string(),
            },
            ToolIdentity {
                kind: ToolKind::Rustc,
                version: "rustc 1".to_string(),
            },
        ];
        fragment.verdict = Verdict::Passed {
            evidence: ProofEvidence::TargetIsa {
                architecture: ArchitectureId::parse("thumbv7em")?,
                completed_scenarios: 2,
                transcript_fingerprint: fingerprint('c')?,
            },
        };

        assert_eq!(
            fragment.validate(),
            Err(ProofContractError::MissingRequiredTool {
                proof: ProofKind::TargetIsa,
                tool: ToolKind::Qemu,
            })
        );
        Ok(())
    }

    #[test]
    fn executable_evidence_is_bound_to_one_transcript_artifact(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut fragment = fragment()?;
        fragment.subject = Subject::Architecture(ArchitectureId::parse("thumbv7em")?);
        fragment.proof = ProofKind::TargetIsa;
        fragment.tools = vec![
            ToolIdentity {
                kind: ToolKind::Cargo,
                version: "cargo 1".to_string(),
            },
            ToolIdentity {
                kind: ToolKind::Rustc,
                version: "rustc 1".to_string(),
            },
            ToolIdentity {
                kind: ToolKind::Qemu,
                version: "qemu 1".to_string(),
            },
        ];
        fragment.artifacts.clear();
        fragment.artifacts.push(EvidenceArtifact {
            kind: ProofArtifactKind::Log,
            path: EvidencePath::parse("qemu.log")?,
            bytes: 32,
            fingerprint: fingerprint('a')?,
        });
        fragment.verdict = Verdict::Passed {
            evidence: ProofEvidence::TargetIsa {
                architecture: ArchitectureId::parse("thumbv7em")?,
                completed_scenarios: 2,
                transcript_fingerprint: fingerprint('c')?,
            },
        };
        assert_eq!(
            fragment.validate(),
            Err(ProofContractError::MissingTranscriptArtifact)
        );
        fragment.artifacts.push(EvidenceArtifact {
            kind: ProofArtifactKind::Transcript,
            path: EvidencePath::parse("transcript.bin")?,
            bytes: 32,
            fingerprint: fingerprint('d')?,
        });
        assert_eq!(
            fragment.validate(),
            Err(ProofContractError::TranscriptFingerprintMismatch)
        );
        fragment.artifacts[1].fingerprint = fingerprint('c')?;
        assert_eq!(
            fragment.validate(),
            Err(ProofContractError::MissingExecutableArtifact)
        );
        fragment.artifacts.push(EvidenceArtifact {
            kind: ProofArtifactKind::Executable,
            path: EvidencePath::parse("kernel.elf")?,
            bytes: 32,
            fingerprint: fingerprint('e')?,
        });
        fragment.validate()?;
        fragment.artifacts.push(EvidenceArtifact {
            kind: ProofArtifactKind::Transcript,
            path: EvidencePath::parse("second-transcript.bin")?,
            bytes: 32,
            fingerprint: fingerprint('c')?,
        });
        assert_eq!(
            fragment.validate(),
            Err(ProofContractError::DuplicateTranscriptArtifact)
        );
        fragment.artifacts.pop();
        fragment.artifacts.push(EvidenceArtifact {
            kind: ProofArtifactKind::Executable,
            path: EvidencePath::parse("second-kernel.elf")?,
            bytes: 32,
            fingerprint: fingerprint('f')?,
        });
        assert_eq!(
            fragment.validate(),
            Err(ProofContractError::DuplicateExecutableArtifact)
        );
        Ok(())
    }
}
