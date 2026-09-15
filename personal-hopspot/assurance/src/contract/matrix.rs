use personal_hopspot_builder::SourceCustody;
use serde::{Deserialize, Serialize};

use super::{
    ArchitectureId, Capability, EvidenceFingerprint, ProofFragment, TargetId, UnavailableReason,
    Verdict,
};

pub const ASSURANCE_MATRIX_SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum MatrixStatus {
    Passed,
    Failed { required_failures: usize },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceEvidence {
    pub source: SourceCustody,
    pub report_fingerprint: EvidenceFingerprint,
    pub build_fingerprint: EvidenceFingerprint,
    pub toolchain_fingerprint: EvidenceFingerprint,
    pub memory_contract_fingerprint: EvidenceFingerprint,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetEvidence {
    pub id: TargetId,
    pub display_name: String,
    pub memory_profile: String,
    pub architecture: ArchitectureId,
    pub rust_target: String,
    pub architecture_adapter: String,
    pub resource: Verdict<ResourceEvidence>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum CapabilityResult {
    Observed {
        capability: Capability,
        proof: Box<ProofFragment>,
    },
    Unavailable {
        capability: Capability,
        reason: UnavailableReason,
    },
}

impl CapabilityResult {
    #[must_use]
    pub const fn capability(&self) -> &Capability {
        match self {
            Self::Observed { capability, .. } | Self::Unavailable { capability, .. } => capability,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssuranceMatrix {
    pub schema_version: u32,
    pub resource_report_schema_version: u32,
    pub status: MatrixStatus,
    pub targets: Vec<TargetEvidence>,
    pub capabilities: Vec<CapabilityResult>,
}
