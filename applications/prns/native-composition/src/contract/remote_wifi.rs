use super::{RemoteControlAnnounceUnknownReason, RemoteManagementFailureStage};

/// Credentials cross the bridge once, then immediately enter zeroizing Rust storage.
/// Deliberately neither Debug nor Clone: never retain this input in a snapshot.
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct StartRemoteWifiTrialInput {
    pub target_identity_fingerprint: Vec<u8>,
    pub ssid: String,
    pub password: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct InspectRemoteWifiTrialInput {
    pub target_identity_fingerprint: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct FinishRemoteWifiTrialInput {
    pub target_identity_fingerprint: Vec<u8>,
    pub revision: u32,
    pub decision: RemoteWifiDecision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum RemoteWifiDecision {
    Keep,
    Restore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum RemoteWifiAction {
    Start,
    Inspect,
    Keep,
    Restore,
}

/// The node's observation, not a claim about which app attempt caused it.
/// AwaitingConfirmation does not establish that the new network is connected.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum RemoteWifiTransaction {
    FactoryProvisioning,
    Confirmed {
        revision: u32,
    },
    Staged {
        revision: u32,
    },
    AwaitingConfirmation {
        revision: u32,
        remaining_seconds: u8,
    },
    RollingBack {
        rejected_revision: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum RemoteWifiStatus {
    Pending,
    Observed {
        transaction: RemoteWifiTransaction,
    },
    Failed {
        stage: RemoteManagementFailureStage,
        detail: String,
    },
    OutcomeUnknown {
        reason: RemoteControlAnnounceUnknownReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct RemoteWifiOperation {
    pub operation_id: u64,
    pub generation_id: u64,
    pub target_identity_fingerprint: Vec<u8>,
    pub action: RemoteWifiAction,
    /// Present after a successful Stage response, or for a revision-bound decision.
    pub candidate_revision: Option<u32>,
    pub status: RemoteWifiStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum RemoteWifiCommandOutcome {
    Accepted {
        operation: RemoteWifiOperation,
    },
    Busy,
    Failed {
        stage: RemoteManagementFailureStage,
        detail: String,
    },
}
