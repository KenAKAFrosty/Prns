use std::collections::BTreeSet;
use std::fmt;
use std::sync::OnceLock;

use serde::Deserialize;

const CONTRACT_JSON: &str = include_str!("../../../web-flasher/bridge-contract.json");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BridgeContract {
    schema: u8,
    phases: Vec<PhaseDefinition>,
    errors: Vec<String>,
    event_fields: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PhaseDefinition {
    wire: String,
    terminal: bool,
    busy: bool,
    label: String,
    tone: PhaseTone,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PhaseTone {
    Neutral,
    Ready,
    Blocked,
    Working,
}

#[derive(Debug)]
pub(super) enum ContractViolation {
    UnknownPhase(String),
    UnknownError(String),
    Schema(u8),
}

impl fmt::Display for ContractViolation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownPhase(phase) => write!(formatter, "unknown bridge phase {phase:?}"),
            Self::UnknownError(code) => write!(formatter, "unknown bridge error {code:?}"),
            Self::Schema(schema) => write!(formatter, "unsupported bridge event schema {schema}"),
        }
    }
}

impl PhaseDefinition {
    pub(super) fn terminal(&self) -> bool {
        self.terminal
    }

    pub(super) fn busy(&self) -> bool {
        self.busy
    }

    pub(super) fn label(&self) -> &str {
        &self.label
    }

    pub(super) fn status_class(&self) -> &'static str {
        match self.tone {
            PhaseTone::Neutral => "flash-status-chip",
            PhaseTone::Ready => "flash-status-chip flash-status-chip--ready",
            PhaseTone::Blocked => "flash-status-chip flash-status-chip--blocked",
            PhaseTone::Working => "flash-status-chip flash-status-chip--pending",
        }
    }
}

pub(super) fn schema() -> u8 {
    contract().schema
}

pub(super) fn phase(wire: &str) -> Result<&'static PhaseDefinition, ContractViolation> {
    contract()
        .phases
        .iter()
        .find(|phase| phase.wire == wire)
        .ok_or_else(|| ContractViolation::UnknownPhase(wire.to_string()))
}

pub(super) fn validate_event(
    event_schema: u8,
    phase_wire: &str,
    error_code: Option<&str>,
) -> Result<(), ContractViolation> {
    if event_schema != schema() {
        return Err(ContractViolation::Schema(event_schema));
    }
    phase(phase_wire)?;
    if let Some(code) = error_code {
        if !contract().errors.iter().any(|known| known == code) {
            return Err(ContractViolation::UnknownError(code.to_string()));
        }
    }
    Ok(())
}

fn contract() -> &'static BridgeContract {
    static CONTRACT: OnceLock<BridgeContract> = OnceLock::new();
    CONTRACT.get_or_init(|| {
        let contract: BridgeContract = serde_json::from_str(CONTRACT_JSON)
            .expect("bundled bridge contract must be valid JSON");
        assert_eq!(contract.schema, 1, "unsupported bundled bridge contract");
        assert_unique(
            contract.phases.iter().map(|phase| phase.wire.as_str()),
            "phase",
        );
        assert_unique(contract.errors.iter().map(String::as_str), "error");
        assert_unique(
            contract.event_fields.iter().map(String::as_str),
            "event field",
        );
        contract
    })
}

fn assert_unique<'a>(values: impl Iterator<Item = &'a str>, kind: &str) {
    let mut unique = BTreeSet::new();
    for value in values {
        assert!(unique.insert(value), "duplicate bridge {kind} {value:?}");
    }
}

#[cfg(test)]
pub(super) fn phase_names() -> impl Iterator<Item = &'static str> {
    contract().phases.iter().map(|phase| phase.wire.as_str())
}

#[cfg(test)]
pub(super) fn event_fields() -> impl Iterator<Item = &'static str> {
    contract().event_fields.iter().map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_parses_and_rejects_unknown_wire_values() {
        assert_eq!(schema(), 1);
        assert!(phase("writing").expect("known phase").busy());
        assert!(phase("success").expect("known phase").terminal());
        assert!(matches!(
            phase("invented"),
            Err(ContractViolation::UnknownPhase(_))
        ));
        assert!(matches!(
            validate_event(1, "failed", Some("invented")),
            Err(ContractViolation::UnknownError(_))
        ));
    }
}
