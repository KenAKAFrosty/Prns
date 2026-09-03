use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};
use ts_rs::{Config, TS};

pub const CONTRACT_FINGERPRINT: &str = env!("PRNS_APP_CONTRACT_FINGERPRINT");
pub const HOST_CONTRACT_FINGERPRINT: &str = env!("PRNS_HOST_CONTRACT_FINGERPRINT");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(type = "string")]
pub struct U64String(pub String);

impl From<u64> for U64String {
    fn from(value: u64) -> Self {
        Self(value.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct DevelopmentNodeSnapshot {
    pub contract_fingerprint: String,
    pub revision: U64String,
    pub runtime: DevelopmentNodeRuntime,
    pub primary_identity: PrimaryIdentityState,
    pub local_host: LocalHostState,
    pub controller_identity_fingerprint: Option<Vec<u8>>,
    pub pairing: RemoteControlPairingState,
    pub paired_targets: Vec<RemoteControlTargetSnapshot>,
    pub active_operation: Option<DevelopmentNodeOperation>,
    pub failure: Option<DevelopmentNodeFailure>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum DevelopmentNodeRuntime {
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PrimaryIdentityState {
    Missing,
    Present { identity_hash: Vec<u8> },
    Unavailable { detail: String },
    DevelopmentResetRequired { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, TS)]
#[ts(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum LocalHostState {
    Stopped {
        last_start_failure: Option<String>,
    },
    Running {
        #[ts(type = "HostSnapshot")]
        host: Box<prns_host::HostSnapshot>,
    },
    Unavailable {
        detail: String,
    },
    DevelopmentResetRequired {
        reason: String,
    },
}

impl Serialize for LocalHostState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Stopped { last_start_failure } => {
                let mut state = serializer.serialize_struct("LocalHostState", 2)?;
                state.serialize_field("type", "stopped")?;
                state.serialize_field("lastStartFailure", last_start_failure)?;
                state.end()
            }
            Self::Running { host } => {
                let encoded = prns_host::serialize_host_snapshot_json(host)
                    .map_err(serde::ser::Error::custom)?;
                let host: serde_json::Value =
                    serde_json::from_str(&encoded).map_err(serde::ser::Error::custom)?;
                let mut state = serializer.serialize_struct("LocalHostState", 2)?;
                state.serialize_field("type", "running")?;
                state.serialize_field("host", &host)?;
                state.end()
            }
            Self::Unavailable { detail } => {
                let mut state = serializer.serialize_struct("LocalHostState", 2)?;
                state.serialize_field("type", "unavailable")?;
                state.serialize_field("detail", detail)?;
                state.end()
            }
            Self::DevelopmentResetRequired { reason } => {
                let mut state = serializer.serialize_struct("LocalHostState", 2)?;
                state.serialize_field("type", "developmentResetRequired")?;
                state.serialize_field("reason", reason)?;
                state.end()
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum IdentityImportPreviewOutcome {
    Valid { identity_hash: Vec<u8> },
    InvalidLength,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum IdentityCreationOutcome {
    Created { identity_hash: Vec<u8> },
    AlreadyExists,
    InvalidLength,
    Unavailable { detail: String },
    DevelopmentResetRequired { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RemoteControlPairingState {
    BluetoothUnavailable,
    Searching,
    CandidateObserved {
        candidate: RemoteControlPairingCandidate,
    },
    InvitationSubmitted {
        candidate_id: String,
    },
    ConfirmationRequired {
        attempt_id: String,
        confirmation_code: String,
        target_identity_fingerprint: Vec<u8>,
        permissions: Vec<RemoteControlRequestKind>,
    },
    AwaitingTargetApproval {
        attempt_id: String,
    },
    Persisting {
        attempt_id: String,
    },
    Paired {
        attempt_id: String,
    },
    Rejected {
        detail: String,
    },
    Expired {
        detail: String,
    },
    Cancelled,
    Failed {
        stage: RemoteControlPairingFailureStage,
        detail: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct RemoteControlPairingCandidate {
    pub candidate_id: String,
    pub endpoint: Vec<u8>,
    pub observed_at_millis: U64String,
    pub expires_at_millis: U64String,
    pub public_app_data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum RemoteControlRequestKind {
    Describe,
    AnnounceSelf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct RemoteControlTargetSnapshot {
    pub target_identity_fingerprint: Vec<u8>,
    pub destination: Vec<u8>,
    pub controller_identity_fingerprint: Vec<u8>,
    pub permitted_requests: Vec<RemoteControlRequestKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct DevelopmentNodeOperation {
    pub kind: DevelopmentNodeOperationKind,
    pub started_at_millis: U64String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum DevelopmentNodeOperationKind {
    Pairing,
    Describe,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct DevelopmentNodeFailure {
    pub stage: DevelopmentNodeFailureStage,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum DevelopmentNodeFailureStage {
    Storage,
    Identity,
    Runtime,
    PersistenceRestore,
    Bluetooth,
    Node,
    Contract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum RemoteControlPairingFailureStage {
    Input,
    Candidate,
    Route,
    Link,
    Identification,
    Request,
    Confirmation,
    Persistence,
    Expired,
    Node,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum DevelopmentNodeStopStage {
    CommandAdmission,
    TargetConnection,
    Persistence,
    Node,
    Worker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum RemoteControlDescribeFailureStage {
    Input,
    Inventory,
    Route,
    Link,
    Identification,
    Permission,
    Request,
    Timeout,
    Node,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DevelopmentNodeStartOutcome {
    Started {
        snapshot: DevelopmentNodeSnapshot,
    },
    AlreadyRunning {
        snapshot: DevelopmentNodeSnapshot,
    },
    Failed {
        stage: DevelopmentNodeFailureStage,
        detail: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DevelopmentNodeStopOutcome {
    Stopped,
    AlreadyStopped,
    Failed {
        stage: DevelopmentNodeStopStage,
        detail: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RemoteControlPairingCommandOutcome {
    Accepted {
        snapshot: Box<DevelopmentNodeSnapshot>,
    },
    Busy,
    Failed {
        stage: RemoteControlPairingFailureStage,
        detail: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RemoteControlDescribeOutcome {
    Described {
        target: RemoteControlTargetSnapshot,
        available_requests: Vec<RemoteControlRequestKind>,
        rtt_millis: U64String,
        snapshot: Box<DevelopmentNodeSnapshot>,
    },
    Busy,
    Failed {
        stage: RemoteControlDescribeFailureStage,
        detail: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct InitiateRemoteControlPairingInput {
    pub candidate_id: String,
    pub invitation_code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct RemoteControlPairingDecisionInput {
    pub attempt_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct DescribeRemoteControlTargetInput {
    pub target_identity_fingerprint: Vec<u8>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TaggedContractFixtures {
    primary_identity_states: Vec<PrimaryIdentityState>,
    local_host_states: Vec<LocalHostState>,
    identity_import_preview_outcomes: Vec<IdentityImportPreviewOutcome>,
    identity_creation_outcomes: Vec<IdentityCreationOutcome>,
    pairing_states: Vec<RemoteControlPairingState>,
    start_outcomes: Vec<DevelopmentNodeStartOutcome>,
    stop_outcomes: Vec<DevelopmentNodeStopOutcome>,
    pairing_outcomes: Vec<RemoteControlPairingCommandOutcome>,
    describe_outcomes: Vec<RemoteControlDescribeOutcome>,
}

impl DevelopmentNodeSnapshot {
    #[must_use]
    pub fn stopped() -> Self {
        Self {
            contract_fingerprint: CONTRACT_FINGERPRINT.to_owned(),
            revision: U64String::from(0),
            runtime: DevelopmentNodeRuntime::Stopped,
            primary_identity: PrimaryIdentityState::Missing,
            local_host: LocalHostState::Stopped {
                last_start_failure: None,
            },
            controller_identity_fingerprint: None,
            pairing: RemoteControlPairingState::Searching,
            paired_targets: Vec::new(),
            active_operation: None,
            failure: None,
        }
    }
}

pub fn export_typescript() -> String {
    let config = Config::default();
    let mut output = String::from(
        "// Generated by prns-app-native. Do not edit.\n\n\
         import type { HostSnapshot } from \"personal-rns/contract\";\n\n",
    );
    output.push_str("export const NATIVE_CONTRACT_FINGERPRINT = ");
    output.push_str(
        &serde_json::to_string(CONTRACT_FINGERPRINT).unwrap_or_else(|_| "\"invalid\"".to_owned()),
    );
    output.push_str(" as const;\n\n");
    output.push_str("export const HOST_CONTRACT_FINGERPRINT = ");
    output.push_str(
        &serde_json::to_string(HOST_CONTRACT_FINGERPRINT)
            .unwrap_or_else(|_| "\"invalid\"".to_owned()),
    );
    output.push_str(" as const;\n\n");
    macro_rules! export {
        ($type:ty) => {{
            output.push_str("export ");
            output.push_str(&<$type as TS>::decl(&config));
            output.push_str("\n\n");
        }};
    }
    export!(U64String);
    export!(DevelopmentNodeRuntime);
    export!(PrimaryIdentityState);
    export!(LocalHostState);
    export!(IdentityImportPreviewOutcome);
    export!(IdentityCreationOutcome);
    export!(RemoteControlRequestKind);
    export!(RemoteControlPairingCandidate);
    export!(RemoteControlPairingState);
    export!(RemoteControlTargetSnapshot);
    export!(DevelopmentNodeOperationKind);
    export!(DevelopmentNodeOperation);
    export!(DevelopmentNodeFailureStage);
    export!(DevelopmentNodeFailure);
    export!(RemoteControlPairingFailureStage);
    export!(DevelopmentNodeStopStage);
    export!(RemoteControlDescribeFailureStage);
    export!(DevelopmentNodeSnapshot);
    export!(DevelopmentNodeStartOutcome);
    export!(DevelopmentNodeStopOutcome);
    export!(RemoteControlPairingCommandOutcome);
    export!(RemoteControlDescribeOutcome);
    export!(InitiateRemoteControlPairingInput);
    export!(RemoteControlPairingDecisionInput);
    export!(DescribeRemoteControlTargetInput);
    output.push_str("export const NATIVE_CONTRACT_FIXTURES = ");
    output.push_str(
        &serde_json::to_string(&tagged_contract_fixtures()).unwrap_or_else(|_| "{}".to_owned()),
    );
    output.push_str(
        " as const satisfies {\n\
         \treadonly primaryIdentityStates: readonly PrimaryIdentityState[];\n\
         \treadonly localHostStates: readonly unknown[];\n\
         \treadonly identityImportPreviewOutcomes: readonly IdentityImportPreviewOutcome[];\n\
         \treadonly identityCreationOutcomes: readonly IdentityCreationOutcome[];\n\
         \treadonly pairingStates: readonly RemoteControlPairingState[];\n\
         \treadonly startOutcomes: readonly DevelopmentNodeStartOutcome[];\n\
         \treadonly stopOutcomes: readonly DevelopmentNodeStopOutcome[];\n\
         \treadonly pairingOutcomes: readonly RemoteControlPairingCommandOutcome[];\n\
         \treadonly describeOutcomes: readonly RemoteControlDescribeOutcome[];\n\
         };\n",
    );
    output
}

fn tagged_contract_fixtures() -> TaggedContractFixtures {
    let hash = vec![0x11; 16];
    let destination = vec![0x22; 16];
    let target = RemoteControlTargetSnapshot {
        target_identity_fingerprint: hash.clone(),
        destination: destination.clone(),
        controller_identity_fingerprint: vec![0x33; 16],
        permitted_requests: vec![
            RemoteControlRequestKind::Describe,
            RemoteControlRequestKind::AnnounceSelf,
        ],
    };
    let mut snapshot = DevelopmentNodeSnapshot::stopped();
    snapshot.revision = U64String::from(u64::MAX);
    snapshot.controller_identity_fingerprint = Some(vec![0x33; 16]);
    snapshot.paired_targets.push(target.clone());

    let primary_identity_states = vec![
        PrimaryIdentityState::Missing,
        PrimaryIdentityState::Present {
            identity_hash: hash.clone(),
        },
        PrimaryIdentityState::Unavailable {
            detail: "identity unavailable fixture".to_owned(),
        },
        PrimaryIdentityState::DevelopmentResetRequired {
            reason: "identity reset fixture".to_owned(),
        },
    ];
    let local_host_states = vec![
        LocalHostState::Stopped {
            last_start_failure: None,
        },
        LocalHostState::Stopped {
            last_start_failure: Some("start failure fixture".to_owned()),
        },
        LocalHostState::Running {
            host: Box::new(host_fixture()),
        },
        LocalHostState::Unavailable {
            detail: "host unavailable fixture".to_owned(),
        },
        LocalHostState::DevelopmentResetRequired {
            reason: "host reset fixture".to_owned(),
        },
    ];
    let identity_import_preview_outcomes = vec![
        IdentityImportPreviewOutcome::Valid {
            identity_hash: hash.clone(),
        },
        IdentityImportPreviewOutcome::InvalidLength,
    ];
    let identity_creation_outcomes = vec![
        IdentityCreationOutcome::Created {
            identity_hash: hash.clone(),
        },
        IdentityCreationOutcome::AlreadyExists,
        IdentityCreationOutcome::InvalidLength,
        IdentityCreationOutcome::Unavailable {
            detail: "create unavailable fixture".to_owned(),
        },
        IdentityCreationOutcome::DevelopmentResetRequired {
            reason: "create reset fixture".to_owned(),
        },
    ];
    let pairing_states = vec![
        RemoteControlPairingState::BluetoothUnavailable,
        RemoteControlPairingState::Searching,
        RemoteControlPairingState::CandidateObserved {
            candidate: RemoteControlPairingCandidate {
                candidate_id: "candidate-fixture".to_owned(),
                endpoint: destination,
                observed_at_millis: U64String::from((1_u64 << 53) - 1),
                expires_at_millis: U64String::from(1_u64 << 53),
                public_app_data: vec![0, 127, 255],
            },
        },
        RemoteControlPairingState::InvitationSubmitted {
            candidate_id: "candidate-fixture".to_owned(),
        },
        RemoteControlPairingState::ConfirmationRequired {
            attempt_id: "00112233445566778899aabbccddeeff".to_owned(),
            confirmation_code: "012345".to_owned(),
            target_identity_fingerprint: hash,
            permissions: vec![RemoteControlRequestKind::Describe],
        },
        RemoteControlPairingState::AwaitingTargetApproval {
            attempt_id: "attempt-fixture".to_owned(),
        },
        RemoteControlPairingState::Persisting {
            attempt_id: "attempt-fixture".to_owned(),
        },
        RemoteControlPairingState::Paired {
            attempt_id: "attempt-fixture".to_owned(),
        },
        RemoteControlPairingState::Rejected {
            detail: "rejected fixture".to_owned(),
        },
        RemoteControlPairingState::Expired {
            detail: "expired fixture".to_owned(),
        },
        RemoteControlPairingState::Cancelled,
        RemoteControlPairingState::Failed {
            stage: RemoteControlPairingFailureStage::Persistence,
            detail: "pairing failure fixture".to_owned(),
        },
    ];
    let start_outcomes = vec![
        DevelopmentNodeStartOutcome::Started {
            snapshot: snapshot.clone(),
        },
        DevelopmentNodeStartOutcome::AlreadyRunning {
            snapshot: snapshot.clone(),
        },
        DevelopmentNodeStartOutcome::Failed {
            stage: DevelopmentNodeFailureStage::Runtime,
            detail: "start failure fixture".to_owned(),
        },
    ];
    let stop_outcomes = vec![
        DevelopmentNodeStopOutcome::Stopped,
        DevelopmentNodeStopOutcome::AlreadyStopped,
        DevelopmentNodeStopOutcome::Failed {
            stage: DevelopmentNodeStopStage::Persistence,
            detail: "stop failure fixture".to_owned(),
        },
    ];
    let pairing_outcomes = vec![
        RemoteControlPairingCommandOutcome::Accepted {
            snapshot: Box::new(snapshot.clone()),
        },
        RemoteControlPairingCommandOutcome::Busy,
        RemoteControlPairingCommandOutcome::Failed {
            stage: RemoteControlPairingFailureStage::Link,
            detail: "pairing command failure fixture".to_owned(),
        },
    ];
    let describe_outcomes = vec![
        RemoteControlDescribeOutcome::Described {
            target,
            available_requests: vec![RemoteControlRequestKind::Describe],
            rtt_millis: U64String::from(0),
            snapshot: Box::new(snapshot),
        },
        RemoteControlDescribeOutcome::Busy,
        RemoteControlDescribeOutcome::Failed {
            stage: RemoteControlDescribeFailureStage::Timeout,
            detail: "describe failure fixture".to_owned(),
        },
    ];
    TaggedContractFixtures {
        primary_identity_states,
        local_host_states,
        identity_import_preview_outcomes,
        identity_creation_outcomes,
        pairing_states,
        start_outcomes,
        stop_outcomes,
        pairing_outcomes,
        describe_outcomes,
    }
}

fn host_fixture() -> prns_host::HostSnapshot {
    prns_host::HostSnapshot {
        revision: u64::MAX,
        backend: prns_host::BackendInfo::new(
            prns_host::BackendKind::Native,
            [prns_host::Capability::Bluetooth],
            [prns_host::InterfaceKind::AutomaticBluetoothLe],
        ),
        interfaces: vec![prns_host::InterfaceSnapshot {
            interface_id: prns_host::InterfaceId::new([0x44; 8]),
            name: Some("Bluetooth Auto".to_owned()),
            kind: Some(prns_host::InterfaceKind::AutomaticBluetoothLe),
            health: prns_host::InterfaceHealth::Connected,
            failure_detail: None,
            rx_bytes: u64::MAX,
            tx_bytes: 1_u64 << 53,
            rx_bps: Some(7),
            tx_bps: Some(8),
            route_count: 1,
            link_count: 2,
            transported_link_count: 3,
        }],
        routes: vec![prns_host::RouteSnapshot {
            destination: prns_host::DestinationHash::new([0x55; 16]),
            hops: 1,
            via_identity: Some(prns_host::IdentityHash::new([0x66; 16])),
            interface_id: prns_host::InterfaceId::new([0x44; 8]),
            learned_at_millis: 11,
            last_route_activity_at_millis: 12,
            expires_at_millis: 13,
        }],
        active_link_count: 2,
        destination_identities: vec![prns_host::DestinationIdentitySnapshot {
            destination: prns_host::DestinationHash::new([0x55; 16]),
            identity: prns_host::IdentityHash::new([0x77; 16]),
        }],
        runtime: prns_host::RuntimeHealthSnapshot {
            running: true,
            uptime_millis: 14,
            interface_count: 1,
            online_interface_count: 1,
            route_count: 1,
            link_count: 2,
            transported_link_count: 3,
            rx_bytes: u64::MAX,
            tx_bytes: 1_u64 << 53,
            rx_bps: 7,
            tx_bps: 8,
        },
        persistence: prns_host::PersistenceSnapshot {
            persistent: true,
            restored: true,
            last_flush_cause: Some(prns_host::PersistenceFlushCause::Startup),
            last_failure_detail: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_u64_fixture_is_an_exact_decimal_string() {
        for value in [0, (1_u64 << 53) - 1, 1_u64 << 53, u64::MAX] {
            let encoded = serde_json::to_string(&U64String::from(value)).unwrap_or_default();
            assert_eq!(encoded, format!("\"{value}\""));
        }
    }

    #[test]
    fn stopped_snapshot_carries_the_contract_fingerprint() {
        assert_eq!(
            DevelopmentNodeSnapshot::stopped().contract_fingerprint,
            CONTRACT_FINGERPRINT
        );
    }

    #[test]
    fn fixtures_cover_every_tagged_contract_variant() {
        let fixtures = tagged_contract_fixtures();
        assert_eq!(fixtures.primary_identity_states.len(), 4);
        assert_eq!(fixtures.local_host_states.len(), 5);
        assert_eq!(fixtures.identity_import_preview_outcomes.len(), 2);
        assert_eq!(fixtures.identity_creation_outcomes.len(), 5);
        assert_eq!(fixtures.pairing_states.len(), 12);
        assert_eq!(fixtures.start_outcomes.len(), 3);
        assert_eq!(fixtures.stop_outcomes.len(), 3);
        assert_eq!(fixtures.pairing_outcomes.len(), 3);
        assert_eq!(fixtures.describe_outcomes.len(), 3);
        let encoded = serde_json::to_string(&fixtures).unwrap_or_default();
        assert!(encoded.contains("\"18446744073709551615\""));
        assert!(encoded.contains("\"9007199254740991\""));
        assert!(encoded.contains("\"9007199254740992\""));
        assert!(encoded.contains("\"expiresAtMillis\":13"));
        assert!(encoded.contains("\"interfaceId\":[68,68,68,68,68,68,68,68]"));
    }
}
