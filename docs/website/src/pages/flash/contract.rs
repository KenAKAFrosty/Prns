use std::collections::BTreeSet;
use std::fmt;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

const CONTRACT_JSON: &str = include_str!("../../../web-flasher/bridge-contract.json");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BridgeContract {
    schema: u8,
    minimum_progress_total: u64,
    response_limits: ResponseLimits,
    operations: Vec<OperationDefinition>,
    phases: Vec<PhaseDefinition>,
    errors: Vec<BridgeErrorCode>,
    event_fields: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ResponseLimits {
    channel_bytes: u64,
    manifest_bytes: u64,
    signature_bytes: u64,
    artifact_bytes: u64,
}

impl ResponseLimits {
    pub(super) fn channel_bytes(&self) -> u64 {
        self.channel_bytes
    }

    pub(super) fn manifest_bytes(&self) -> u64 {
        self.manifest_bytes
    }

    pub(super) fn signature_bytes(&self) -> u64 {
        self.signature_bytes
    }

    #[cfg(test)]
    pub(super) fn artifact_bytes(&self) -> u64 {
        self.artifact_bytes
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OperationDefinition {
    wire: BridgeOperation,
    initial: Vec<BridgePhase>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PhaseDefinition {
    wire: BridgePhase,
    terminal: bool,
    busy: bool,
    label: String,
    tone: PhaseTone,
    next: Vec<BridgePhase>,
    error_policy: ErrorPolicy,
    progress: ProgressPolicy,
    parts: PartPolicy,
    detected_chip: bool,
    bytes: bool,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PhaseTone {
    Neutral,
    Ready,
    Blocked,
    Working,
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ErrorPolicy {
    Forbidden,
    Failure,
    Cancelled,
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ProgressPolicy {
    Forbidden,
    Required,
    Complete,
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum PartPolicy {
    Forbidden,
    Optional,
    Required,
}

prns_macros::iterable_enum! {
    #[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
    #[serde(rename_all = "snake_case")]
    pub(super) enum BridgeOperation {
        Preparation,
        Device,
    }
    const ALL;
}

impl BridgeOperation {
    pub(super) const fn wire(self) -> &'static str {
        match self {
            Self::Preparation => "preparation",
            Self::Device => "device",
        }
    }
}

prns_macros::iterable_enum! {
    /// A phase accepted by the shared JavaScript/Rust bridge contract.
    #[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
    #[serde(rename_all = "snake_case")]
    pub(super) enum BridgePhase {
        Idle,
        ValidatingManifest,
        Downloading,
        VerifyingArtifacts,
        Ready,
        RequestingPort,
        Connecting,
        AwaitingBootloaderPort,
        VerifyingTarget,
        Erasing,
        Writing,
        VerifyingFlash,
        Resetting,
        Success,
        DownloadRequested,
        Failed,
        Cancelled,
    }
    const ALL;
}

impl BridgePhase {
    pub(super) const fn wire(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::ValidatingManifest => "validating_manifest",
            Self::Downloading => "downloading",
            Self::VerifyingArtifacts => "verifying_artifacts",
            Self::Ready => "ready",
            Self::RequestingPort => "requesting_port",
            Self::Connecting => "connecting",
            Self::AwaitingBootloaderPort => "awaiting_bootloader_port",
            Self::VerifyingTarget => "verifying_target",
            Self::Erasing => "erasing",
            Self::Writing => "writing",
            Self::VerifyingFlash => "verifying_flash",
            Self::Resetting => "resetting",
            Self::Success => "success",
            Self::DownloadRequested => "download_requested",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

impl fmt::Display for BridgePhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.wire())
    }
}

prns_macros::iterable_enum! {
    /// An error code accepted by the shared JavaScript/Rust bridge contract.
    #[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
    #[serde(rename_all = "snake_case")]
    pub(super) enum BridgeErrorCode {
        InvalidRequest,
        InvalidConfig,
        UnsupportedBrowser,
        InsecureContext,
        PermissionDenied,
        BootloaderPermissionDenied,
        ConnectionFailure,
        AmbiguousDevice,
        ReconnectTimeout,
        WrongChip,
        WrongFlashSize,
        EraseFailure,
        ArtifactFetch,
        ArtifactSizeMismatch,
        ArtifactHashMismatch,
        DeviceLost,
        WriteFailure,
        MalformedAcknowledgement,
        RetriesExhausted,
        VerificationFailure,
        ResetFailure,
        Cancelled,
        NotPrepared,
        Busy,
        FlashFailed,
    }
    const ALL; //explicitly overridden to make private
}

impl BridgeErrorCode {
    pub(super) const fn wire(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::InvalidConfig => "invalid_config",
            Self::UnsupportedBrowser => "unsupported_browser",
            Self::InsecureContext => "insecure_context",
            Self::PermissionDenied => "permission_denied",
            Self::BootloaderPermissionDenied => "bootloader_permission_denied",
            Self::ConnectionFailure => "connection_failure",
            Self::AmbiguousDevice => "ambiguous_device",
            Self::ReconnectTimeout => "reconnect_timeout",
            Self::WrongChip => "wrong_chip",
            Self::WrongFlashSize => "wrong_flash_size",
            Self::EraseFailure => "erase_failure",
            Self::ArtifactFetch => "artifact_fetch",
            Self::ArtifactSizeMismatch => "artifact_size_mismatch",
            Self::ArtifactHashMismatch => "artifact_hash_mismatch",
            Self::DeviceLost => "device_lost",
            Self::WriteFailure => "write_failure",
            Self::MalformedAcknowledgement => "malformed_acknowledgement",
            Self::RetriesExhausted => "retries_exhausted",
            Self::VerificationFailure => "verification_failure",
            Self::ResetFailure => "reset_failure",
            Self::Cancelled => "cancelled",
            Self::NotPrepared => "not_prepared",
            Self::Busy => "busy",
            Self::FlashFailed => "flash_failed",
        }
    }

    pub(super) const fn retains_prepared_plan(self) -> bool {
        matches!(self, Self::PermissionDenied)
    }
}

impl fmt::Display for BridgeErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.wire())
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

    pub(super) fn next(&self) -> &[BridgePhase] {
        &self.next
    }

    pub(super) fn error_policy(&self) -> ErrorPolicy {
        self.error_policy
    }

    pub(super) fn progress_policy(&self) -> ProgressPolicy {
        self.progress
    }

    pub(super) fn part_policy(&self) -> PartPolicy {
        self.parts
    }

    pub(super) fn requires_detected_chip(&self) -> bool {
        self.detected_chip
    }

    pub(super) fn requires_bytes(&self) -> bool {
        self.bytes
    }
}

impl OperationDefinition {
    pub(super) fn initial(&self) -> &[BridgePhase] {
        &self.initial
    }
}

pub(super) fn schema() -> u8 {
    contract().schema
}

pub(super) fn minimum_progress_total() -> u64 {
    contract().minimum_progress_total
}

pub(super) fn response_limits() -> &'static ResponseLimits {
    &contract().response_limits
}

pub(super) fn phase(value: BridgePhase) -> &'static PhaseDefinition {
    phase_definition(value)
}

fn phase_definition(value: BridgePhase) -> &'static PhaseDefinition {
    contract()
        .phases
        .iter()
        .find(|phase| phase.wire == value)
        .expect("bridge phase enum must match bundled contract")
}

pub(super) fn operation(value: BridgeOperation) -> &'static OperationDefinition {
    contract()
        .operations
        .iter()
        .find(|operation| operation.wire == value)
        .expect("bridge operation enum must match bundled contract")
}

fn contract() -> &'static BridgeContract {
    static CONTRACT: OnceLock<BridgeContract> = OnceLock::new();
    CONTRACT.get_or_init(|| {
        let contract: BridgeContract = serde_json::from_str(CONTRACT_JSON)
            .expect("bundled bridge contract must be valid JSON");
        assert_eq!(contract.schema, 1, "unsupported bundled bridge contract");
        assert_eq!(
            contract.minimum_progress_total, 1,
            "unsupported bridge minimum progress total"
        );
        assert_eq!(
            (
                contract.response_limits.channel_bytes,
                contract.response_limits.manifest_bytes,
                contract.response_limits.signature_bytes,
                contract.response_limits.artifact_bytes,
            ),
            (64 * 1024, 512 * 1024, 64 * 1024, 64 * 1024 * 1024),
            "unsupported response safety limits"
        );
        assert_exact_values(
            contract.operations.iter().map(|operation| operation.wire),
            &BridgeOperation::ALL,
            "operation",
        );
        assert_exact_values(
            contract.phases.iter().map(|phase| phase.wire),
            &BridgePhase::ALL,
            "phase",
        );
        assert_exact_values(
            contract.errors.iter().copied(),
            &BridgeErrorCode::ALL,
            "error",
        );
        assert_unique(
            contract.event_fields.iter().map(String::as_str),
            "event field",
        );
        for operation in &contract.operations {
            assert_unique_values(
                &operation.initial,
                &format!("{} initial phase", operation.wire.wire()),
            );
            assert!(
                !operation.initial.is_empty(),
                "bridge operation {} has no initial phase",
                operation.wire.wire()
            );
        }
        for phase in &contract.phases {
            assert_unique_values(&phase.next, &format!("{} transition", phase.wire));
            assert!(
                !phase.terminal || phase.next.is_empty(),
                "terminal bridge phase {} has a successor",
                phase.wire
            );
        }
        contract
    })
}

fn assert_unique_values<T>(values: &[T], kind: &str)
where
    T: Copy + fmt::Debug + Ord,
{
    let unique = values.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(values.len(), unique.len(), "duplicate bridge {kind}");
}

fn assert_exact_values<T>(actual: impl Iterator<Item = T>, expected: &[T], kind: &str)
where
    T: Copy + fmt::Debug + Ord,
{
    let actual = actual.collect::<Vec<_>>();
    let unique = actual.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(actual.len(), unique.len(), "duplicate bridge {kind}");
    assert_eq!(
        unique,
        expected.iter().copied().collect(),
        "bridge {kind} enum and JSON contract disagree"
    );
}

fn assert_unique<'a>(values: impl Iterator<Item = &'a str>, kind: &str) {
    let mut unique = BTreeSet::new();
    for value in values {
        assert!(unique.insert(value), "duplicate bridge {kind} {value:?}");
    }
}

#[cfg(test)]
pub(super) fn phases() -> impl Iterator<Item = BridgePhase> {
    contract().phases.iter().map(|phase| phase.wire)
}

#[cfg(test)]
pub(super) fn event_fields() -> impl Iterator<Item = &'static str> {
    contract().event_fields.iter().map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_values_exactly_cover_the_bundled_contract() {
        assert_eq!(schema(), 1);
        assert_eq!(minimum_progress_total(), 1);
        assert_eq!(response_limits().channel_bytes(), 64 * 1024);
        assert_eq!(response_limits().manifest_bytes(), 512 * 1024);
        assert_eq!(response_limits().signature_bytes(), 64 * 1024);
        assert_eq!(response_limits().artifact_bytes(), 64 * 1024 * 1024);
        assert!(phase(BridgePhase::Writing).busy());
        assert!(phase(BridgePhase::Success).terminal());
        assert_eq!(
            phases().collect::<BTreeSet<_>>(),
            BridgePhase::ALL.into_iter().collect()
        );
        assert_eq!(
            contract().errors.iter().copied().collect::<BTreeSet<_>>(),
            BridgeErrorCode::ALL.into_iter().collect()
        );
        assert_eq!(
            contract()
                .operations
                .iter()
                .map(|operation| operation.wire)
                .collect::<BTreeSet<_>>(),
            BridgeOperation::ALL.into_iter().collect()
        );
        for phase in BridgePhase::ALL {
            assert_eq!(
                serde_json::to_string(&phase).expect("serialize phase"),
                format!("\"{}\"", phase.wire())
            );
        }
        for code in BridgeErrorCode::ALL {
            assert_eq!(
                serde_json::to_string(&code).expect("serialize error code"),
                format!("\"{}\"", code.wire())
            );
        }
    }

    #[test]
    fn only_device_picker_denial_retains_the_prepared_plan() {
        let retaining = BridgeErrorCode::ALL
            .into_iter()
            .filter(|code| code.retains_prepared_plan())
            .collect::<Vec<_>>();
        assert_eq!(retaining, vec![BridgeErrorCode::PermissionDenied]);
    }
}
