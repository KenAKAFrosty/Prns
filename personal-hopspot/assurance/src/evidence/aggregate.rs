use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};

use personal_hopspot_memory::ProcessorArchitecture;
use personal_hopspot_resources::matrix::{canonical_targets, CanonicalMatrixError};
use personal_hopspot_resources::report::{BuildOutcome, SCHEMA_VERSION};
use thiserror::Error;

use super::discovery::{ProofDocument, ResourceDocument};
use crate::capabilities;
use crate::contract::{
    ArchitectureId, AssuranceMatrix, Capability, CapabilityResult, EvidenceFingerprint, Failure,
    FailureKind, IdentifierError, MatrixStatus, MiriCoverage, ProofEvidence, ProofFragment,
    ProofKind, ResourceEvidence, Subject, SupportLevel, TargetEvidence, TargetId,
    UnavailableReason, ValueError, Verdict, ASSURANCE_MATRIX_SCHEMA_VERSION,
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct CapabilityKey {
    subject: Subject,
    scenario: crate::contract::ScenarioId,
    proof: ProofKind,
}

enum ProofPrecedence {
    Existing,
    Incoming,
    Incompatible,
}

impl From<&Capability> for CapabilityKey {
    fn from(capability: &Capability) -> Self {
        Self {
            subject: capability.subject.clone(),
            scenario: capability.scenario.clone(),
            proof: capability.proof,
        }
    }
}

impl From<&ProofFragment> for CapabilityKey {
    fn from(fragment: &ProofFragment) -> Self {
        Self {
            subject: fragment.subject.clone(),
            scenario: fragment.scenario.clone(),
            proof: fragment.proof,
        }
    }
}

#[derive(Debug, Error)]
pub enum AggregateError {
    #[error(transparent)]
    CanonicalMatrix(#[from] CanonicalMatrixError),
    #[error(transparent)]
    Identifier(#[from] IdentifierError),
    #[error("resource report contains an invalid SHA-256 fingerprint")]
    Fingerprint(#[from] ValueError),
    #[error("canonical target uses unsupported Rust architecture {rust_target:?}")]
    UnsupportedArchitecture { rust_target: String },
    #[error("resource evidence repeats target {target:?} at {first} and {second}")]
    DuplicateResource {
        target: String,
        first: std::path::PathBuf,
        second: std::path::PathBuf,
    },
    #[error("resource evidence contains unexpected target {target:?} at {path}")]
    UnexpectedResource {
        target: String,
        path: std::path::PathBuf,
    },
    #[error("resource report {path} does not match canonical {dimension} for {target:?}")]
    ResourceIdentity {
        path: std::path::PathBuf,
        target: String,
        dimension: &'static str,
    },
    #[error("assurance capability registry repeats {0:?}")]
    DuplicateCapability(Subject),
    #[error("assurance proof repeats {subject:?} scenario {scenario:?} at {first} and {second}")]
    DuplicateProof {
        subject: Subject,
        scenario: crate::contract::ScenarioId,
        first: std::path::PathBuf,
        second: std::path::PathBuf,
    },
    #[error("assurance proof at {path} has no declared capability")]
    UnexpectedProof { path: std::path::PathBuf },
    #[error("assurance proof at {path} targets a capability declared unsupported")]
    ProofForUnsupportedCapability { path: std::path::PathBuf },
    #[error(transparent)]
    Runner(#[from] capabilities::RunnerContractError),
}

pub fn assemble(
    resources: Vec<ResourceDocument>,
    proofs: Vec<ProofDocument>,
) -> Result<AssuranceMatrix, AggregateError> {
    let mut resource_by_target = BTreeMap::new();
    for resource in resources {
        let target = resource.document.target_id().to_string();
        match resource_by_target.entry(target.clone()) {
            Entry::Vacant(entry) => {
                entry.insert(resource);
            }
            Entry::Occupied(entry) => {
                return Err(AggregateError::DuplicateResource {
                    target,
                    first: entry.get().path.clone(),
                    second: resource.path,
                });
            }
        }
    }

    let canonical = canonical_targets()?;
    let canonical_ids = canonical
        .iter()
        .map(|target| target.id())
        .collect::<BTreeSet<_>>();
    if let Some((target, resource)) = resource_by_target
        .iter()
        .find(|(target, _)| !canonical_ids.contains(target.as_str()))
    {
        return Err(AggregateError::UnexpectedResource {
            target: target.clone(),
            path: resource.path.clone(),
        });
    }

    let mut targets = Vec::with_capacity(canonical.len());
    for target in canonical {
        let architecture = architecture_id(target.rust_target())?;
        let target_id = TargetId::parse(target.id())?;
        let resource = resource_by_target.remove(target.id());
        let verdict = match resource {
            Some(resource) => {
                validate_identity(&resource, &target)?;
                match resource.document.build_outcome() {
                    BuildOutcome::Success => Verdict::Passed {
                        evidence: ResourceEvidence {
                            source: resource.document.source_custody().clone(),
                            report_fingerprint: parse_fingerprint(
                                resource.document.fingerprint().as_str(),
                            )?,
                            build_fingerprint: parse_fingerprint(
                                resource.document.build_fingerprint().as_str(),
                            )?,
                            toolchain_fingerprint: parse_fingerprint(
                                resource.document.toolchain_fingerprint().as_str(),
                            )?,
                            memory_contract_fingerprint: parse_fingerprint(
                                resource.document.memory_contract_fingerprint().as_str(),
                            )?,
                        },
                    },
                    BuildOutcome::MemoryOverflow => Verdict::Failed {
                        failure: Failure {
                            kind: FailureKind::MemoryOverflow,
                            diagnostic: "canonical resource build reported memory overflow"
                                .to_string(),
                        },
                    },
                }
            }
            None => Verdict::Unavailable {
                reason: UnavailableReason::EvidenceNotProduced,
            },
        };
        targets.push(TargetEvidence {
            id: target_id,
            display_name: target.display_name().to_string(),
            memory_profile: target.memory_profile().to_string(),
            architecture,
            rust_target: target.rust_target().to_string(),
            architecture_adapter: target.architecture_adapter().to_string(),
            resource: verdict,
        });
    }

    let capability_contracts = capabilities::canonical()?;
    let mut known = BTreeSet::new();
    for capability in &capability_contracts {
        if !known.insert(CapabilityKey::from(capability)) {
            return Err(AggregateError::DuplicateCapability(
                capability.subject.clone(),
            ));
        }
    }

    let mut proof_by_capability = BTreeMap::new();
    for proof in proofs {
        let key = CapabilityKey::from(&proof.fragment);
        match proof_by_capability.entry(key.clone()) {
            Entry::Vacant(entry) => {
                entry.insert(proof);
            }
            Entry::Occupied(entry) => {
                match proof_precedence(&entry.get().fragment, &proof.fragment) {
                    ProofPrecedence::Existing => {}
                    ProofPrecedence::Incoming => {
                        *entry.into_mut() = proof;
                    }
                    ProofPrecedence::Incompatible => {
                        return Err(AggregateError::DuplicateProof {
                            subject: key.subject,
                            scenario: key.scenario,
                            first: entry.get().path.clone(),
                            second: proof.path,
                        });
                    }
                }
            }
        }
    }

    let mut results = Vec::with_capacity(capability_contracts.len());
    for capability in capability_contracts {
        let key = CapabilityKey::from(&capability);
        let proof = proof_by_capability.remove(&key);
        match (&capability.support, proof) {
            (SupportLevel::Required | SupportLevel::Pilot, Some(proof)) => {
                capabilities::validate_runner(&capability, &proof.fragment)?;
                results.push(CapabilityResult::Observed {
                    capability,
                    proof: Box::new(proof.fragment),
                });
            }
            (SupportLevel::Required | SupportLevel::Pilot, None) => {
                results.push(CapabilityResult::Unavailable {
                    capability,
                    reason: UnavailableReason::EvidenceNotProduced,
                });
            }
            (SupportLevel::Unsupported(_) | SupportLevel::NotApplicable(_), Some(proof)) => {
                return Err(AggregateError::ProofForUnsupportedCapability { path: proof.path });
            }
            (SupportLevel::Unsupported(_) | SupportLevel::NotApplicable(_), None) => {
                results.push(CapabilityResult::Unavailable {
                    capability,
                    reason: UnavailableReason::UnsupportedByContract,
                });
            }
        }
    }
    if let Some((_, proof)) = proof_by_capability.into_iter().next() {
        return Err(AggregateError::UnexpectedProof { path: proof.path });
    }

    let required_failures = required_failures(&targets, &results);
    let status = if required_failures == 0 {
        MatrixStatus::Passed
    } else {
        MatrixStatus::Failed { required_failures }
    };
    Ok(AssuranceMatrix {
        schema_version: ASSURANCE_MATRIX_SCHEMA_VERSION,
        resource_report_schema_version: SCHEMA_VERSION,
        status,
        targets,
        capabilities: results,
    })
}

fn proof_precedence(existing: &ProofFragment, incoming: &ProofFragment) -> ProofPrecedence {
    if existing.source != incoming.source || existing.tools != incoming.tools {
        return ProofPrecedence::Incompatible;
    }
    let (
        Verdict::Passed {
            evidence: ProofEvidence::Miri {
                coverage: existing, ..
            },
        },
        Verdict::Passed {
            evidence: ProofEvidence::Miri {
                coverage: incoming, ..
            },
        },
    ) = (&existing.verdict, &incoming.verdict)
    else {
        return ProofPrecedence::Incompatible;
    };
    match (existing, incoming) {
        (MiriCoverage::Stacked, MiriCoverage::StackedAndTree) => ProofPrecedence::Incoming,
        (MiriCoverage::StackedAndTree, MiriCoverage::Stacked) => ProofPrecedence::Existing,
        (MiriCoverage::Stacked, MiriCoverage::Stacked)
        | (MiriCoverage::StackedAndTree, MiriCoverage::StackedAndTree) => {
            ProofPrecedence::Incompatible
        }
    }
}

fn validate_identity(
    resource: &ResourceDocument,
    target: &personal_hopspot_resources::matrix::CanonicalTarget,
) -> Result<(), AggregateError> {
    let values = [
        (
            resource.document.display_name(),
            target.display_name(),
            "display name",
        ),
        (
            resource.document.memory_profile(),
            target.memory_profile(),
            "memory profile",
        ),
        (
            resource.document.rust_target(),
            target.rust_target(),
            "Rust target",
        ),
        (
            resource.document.architecture_adapter(),
            target.architecture_adapter(),
            "architecture adapter",
        ),
    ];
    if let Some((_, _, dimension)) = values
        .iter()
        .find(|(actual, expected, _)| actual != expected)
    {
        Err(AggregateError::ResourceIdentity {
            path: resource.path.clone(),
            target: target.id().to_string(),
            dimension,
        })
    } else {
        Ok(())
    }
}

fn parse_fingerprint(value: &str) -> Result<EvidenceFingerprint, ValueError> {
    EvidenceFingerprint::parse(value)
}

fn architecture_id(rust_target: &str) -> Result<ArchitectureId, AggregateError> {
    let architecture = ProcessorArchitecture::from_rust_target(rust_target).ok_or_else(|| {
        AggregateError::UnsupportedArchitecture {
            rust_target: rust_target.to_string(),
        }
    })?;
    ArchitectureId::parse(architecture.id()).map_err(AggregateError::from)
}

fn required_failures(targets: &[TargetEvidence], results: &[CapabilityResult]) -> usize {
    let resource_failures = targets
        .iter()
        .filter(|target| !matches!(target.resource, Verdict::Passed { .. }))
        .count();
    let proof_failures = results
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
    resource_failures.saturating_add(proof_failures)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use personal_hopspot_resources::report::Document;

    use super::{assemble, proof_precedence, AggregateError, ProofPrecedence};
    use crate::contract::{
        ArchitectureId, ComponentId, EvidenceArtifact, EvidenceFingerprint, EvidencePath,
        MatrixStatus, MiriCoverage, PlatformId, PlatformMilestone, ProofArtifactKind,
        ProofEvidence, ProofFragment, ProofKind, RunnerId, ScenarioId, SourceCommit, SourceCustody,
        SourceIdentity, Subject, ToolIdentity, ToolKind, Verdict, PROOF_FRAGMENT_SCHEMA_VERSION,
    };
    use crate::evidence::discovery::{ProofDocument, ResourceDocument};

    fn resource_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../resources/experiments/lto/t-echo-s140-v6-fat.json")
    }

    fn platform_proof() -> Result<ProofDocument, Box<dyn std::error::Error>> {
        let platform = PlatformId::parse("esp32c6")?;
        Ok(ProofDocument {
            path: PathBuf::from("esp32c6.assurance.json"),
            fragment: ProofFragment {
                schema_version: PROOF_FRAGMENT_SCHEMA_VERSION,
                subject: Subject::Platform(platform.clone()),
                scenario: ScenarioId::parse("platform-startup")?,
                proof: ProofKind::PlatformEmulation,
                runner: RunnerId::parse("qemu-esp32c6")?,
                source: SourceIdentity {
                    custody: SourceCustody::CleanCommit {
                        commit: SourceCommit::parse("a".repeat(40))?,
                    },
                    scenario_fingerprint: EvidenceFingerprint::parse("b".repeat(64))?,
                },
                tools: vec![ToolIdentity {
                    kind: ToolKind::Qemu,
                    version: "qemu 1".to_string(),
                }],
                verdict: Verdict::Passed {
                    evidence: ProofEvidence::PlatformEmulation {
                        platform,
                        milestone: PlatformMilestone::ApplicationEntry,
                        transcript_fingerprint: EvidenceFingerprint::parse("c".repeat(64))?,
                    },
                },
                artifacts: Vec::new(),
            },
        })
    }

    fn miri_proof(coverage: MiriCoverage) -> Result<ProofFragment, Box<dyn std::error::Error>> {
        Ok(ProofFragment {
            schema_version: PROOF_FRAGMENT_SCHEMA_VERSION,
            subject: Subject::Component(ComponentId::parse("sx126x")?),
            scenario: ScenarioId::parse("sx126x-state-machine")?,
            proof: ProofKind::Miri,
            runner: RunnerId::parse(match coverage {
                MiriCoverage::Stacked => "miri-stacked",
                MiriCoverage::StackedAndTree => "miri-stacked-tree",
            })?,
            source: SourceIdentity {
                custody: SourceCustody::CleanCommit {
                    commit: SourceCommit::parse("a".repeat(40))?,
                },
                scenario_fingerprint: EvidenceFingerprint::parse("b".repeat(64))?,
            },
            tools: vec![ToolIdentity {
                kind: ToolKind::Miri,
                version: "miri 1".to_string(),
            }],
            verdict: Verdict::Passed {
                evidence: ProofEvidence::Miri {
                    coverage,
                    completed_tests: 16,
                },
            },
            artifacts: Vec::new(),
        })
    }

    fn target_isa_proof(runner: &str) -> Result<ProofDocument, Box<dyn std::error::Error>> {
        let architecture = ArchitectureId::parse("thumbv7em")?;
        let transcript_fingerprint = EvidenceFingerprint::parse("c".repeat(64))?;
        Ok(ProofDocument {
            path: PathBuf::from("thumbv7em.assurance.json"),
            fragment: ProofFragment {
                schema_version: PROOF_FRAGMENT_SCHEMA_VERSION,
                subject: Subject::Architecture(architecture.clone()),
                scenario: ScenarioId::parse("shared-state-machines")?,
                proof: ProofKind::TargetIsa,
                runner: RunnerId::parse(runner)?,
                source: SourceIdentity {
                    custody: SourceCustody::CleanCommit {
                        commit: SourceCommit::parse("a".repeat(40))?,
                    },
                    scenario_fingerprint: EvidenceFingerprint::parse("b".repeat(64))?,
                },
                tools: [ToolKind::Cargo, ToolKind::Rustc, ToolKind::Qemu]
                    .into_iter()
                    .map(|kind| ToolIdentity {
                        kind,
                        version: "tool 1".to_string(),
                    })
                    .collect(),
                verdict: Verdict::Passed {
                    evidence: ProofEvidence::TargetIsa {
                        architecture,
                        completed_scenarios: 2,
                        transcript_fingerprint: transcript_fingerprint.clone(),
                    },
                },
                artifacts: vec![
                    EvidenceArtifact {
                        kind: ProofArtifactKind::Transcript,
                        path: EvidencePath::parse("transcript.bin")?,
                        bytes: 1,
                        fingerprint: transcript_fingerprint,
                    },
                    EvidenceArtifact {
                        kind: ProofArtifactKind::Executable,
                        path: EvidencePath::parse("kernel.elf")?,
                        bytes: 1,
                        fingerprint: EvidenceFingerprint::parse("d".repeat(64))?,
                    },
                    EvidenceArtifact {
                        kind: ProofArtifactKind::Log,
                        path: EvidencePath::parse("qemu.log")?,
                        bytes: 1,
                        fingerprint: EvidenceFingerprint::parse("e".repeat(64))?,
                    },
                ],
            },
        })
    }

    #[test]
    fn absent_evidence_is_preserved_as_required_failure() -> Result<(), AggregateError> {
        let matrix = assemble(Vec::new(), Vec::new())?;
        assert_eq!(matrix.targets.len(), 14);
        assert_eq!(
            matrix.status,
            MatrixStatus::Failed {
                required_failures: 20,
            }
        );
        Ok(())
    }

    #[test]
    fn duplicate_resource_targets_are_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let path = resource_path();
        let resources = vec![
            ResourceDocument {
                path: path.clone(),
                document: Document::load(&path)?,
            },
            ResourceDocument {
                path: PathBuf::from("duplicate.json"),
                document: Document::load(&path)?,
            },
        ];
        assert!(matches!(
            assemble(resources, Vec::new()),
            Err(AggregateError::DuplicateResource { .. })
        ));
        Ok(())
    }

    #[test]
    fn proof_for_unsupported_capability_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        assert!(matches!(
            assemble(Vec::new(), vec![platform_proof()?]),
            Err(AggregateError::ProofForUnsupportedCapability { .. })
        ));
        Ok(())
    }

    #[test]
    fn duplicate_proofs_are_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let first = platform_proof()?;
        let mut second = platform_proof()?;
        second.path = PathBuf::from("duplicate.assurance.json");
        assert!(matches!(
            assemble(Vec::new(), vec![first, second]),
            Err(AggregateError::DuplicateProof { .. })
        ));
        Ok(())
    }

    #[test]
    fn canonical_architecture_runner_is_enforced_during_aggregation(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let proof = target_isa_proof("arbitrary-qemu")?;
        proof.fragment.validate()?;

        assert!(matches!(
            assemble(Vec::new(), vec![proof]),
            Err(AggregateError::Runner(_))
        ));
        Ok(())
    }

    #[test]
    fn full_miri_proof_supersedes_matching_stacked_only_evidence(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let quick = miri_proof(MiriCoverage::Stacked)?;
        let full = miri_proof(MiriCoverage::StackedAndTree)?;
        assert!(matches!(
            proof_precedence(&quick, &full),
            ProofPrecedence::Incoming
        ));
        assert!(matches!(
            proof_precedence(&full, &quick),
            ProofPrecedence::Existing
        ));
        assert!(matches!(
            proof_precedence(&quick, &quick),
            ProofPrecedence::Incompatible
        ));
        Ok(())
    }
}
