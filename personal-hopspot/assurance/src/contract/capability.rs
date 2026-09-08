use serde::{Deserialize, Serialize};

use super::{ProofKind, ScenarioId};
use crate::contract::proof::Subject;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "reason", rename_all = "kebab-case")]
pub enum SupportLevel {
    Required,
    Pilot,
    Unsupported(CapabilityReason),
    NotApplicable(CapabilityReason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilityReason {
    ArchitectureDoesNotUseComponent,
    EmulatorDoesNotModelPlatform,
    EmulatorDoesNotModelPeripheral,
    ScenarioOutsidePlatformScope,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capability {
    pub subject: Subject,
    pub scenario: ScenarioId,
    pub proof: ProofKind,
    pub support: SupportLevel,
}
