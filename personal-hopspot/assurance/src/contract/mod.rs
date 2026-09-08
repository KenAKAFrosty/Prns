mod capability;
mod identifier;
mod matrix;
mod proof;
mod value;

pub use capability::{Capability, CapabilityReason, SupportLevel};
pub use identifier::{
    ArchitectureId, ComponentId, IdentifierError, PlatformId, RunnerId, ScenarioId, TargetId,
};
pub use matrix::{
    AssuranceMatrix, CapabilityResult, MatrixStatus, ResourceEvidence, TargetEvidence,
    ASSURANCE_MATRIX_SCHEMA_VERSION,
};
pub use proof::{
    EvidenceArtifact, EvidenceGap, Failure, FailureKind, MiriCoverage, PlatformMilestone,
    ProofArtifactKind, ProofContractError, ProofEvidence, ProofFragment, ProofKind, SourceCustody,
    SourceIdentity, Subject, ToolIdentity, ToolKind, UnavailableReason, Verdict,
    PROOF_FRAGMENT_SCHEMA_VERSION,
};
pub use value::{EvidenceFingerprint, EvidencePath, SourceCommit, ValueError};
