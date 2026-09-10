pub const CONTRACT_FINGERPRINT: &str = env!("PRNS_APP_CONTRACT_FINGERPRINT");
pub const HOST_CONTRACT_FINGERPRINT: &str = env!("PRNS_HOST_CONTRACT_FINGERPRINT");

/// Storage initialization runs only on the native lifecycle/background queue.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum NativeStoragePreparationOutcome {
    Prepared,
    Unavailable { detail: String },
    DevelopmentResetRequired { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct DevelopmentNodeSnapshot {
    pub contract_fingerprint: String,
    pub revision: u64,
    pub generation_id: u64,
    pub runtime: DevelopmentNodeRuntime,
    pub primary_identity: PrimaryIdentityState,
    pub local_host: LocalHostState,
    pub lxmf: LxmfHealth,
    pub controller_identity_fingerprint: Option<Vec<u8>>,
    pub pairing: RemoteControlPairingState,
    pub pairing_candidates: Vec<RemoteControlPairingCandidate>,
    pub paired_targets: Vec<RemoteControlTargetSnapshot>,
    pub last_announcement: Option<RemoteControlAnnounceOperation>,
    pub active_operation: Option<DevelopmentNodeOperation>,
    pub failure: Option<DevelopmentNodeFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct DevelopmentNodeStartInput {
    pub development_tcp_target: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum DevelopmentNodeRuntime {
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum PrimaryIdentityState {
    Missing,
    Present { identity_hash: Vec<u8> },
    Unavailable { detail: String },
    DevelopmentResetRequired { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum LocalHostState {
    Stopped { last_start_failure: Option<String> },
    Running { host: Box<prns_host::HostSnapshot> },
    Unavailable { detail: String },
    DevelopmentResetRequired { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum IdentityImportPreviewOutcome {
    Valid { identity_hash: Vec<u8> },
    InvalidLength,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum IdentityCreationOutcome {
    Created { identity_hash: Vec<u8> },
    AlreadyExists,
    InvalidLength,
    Unavailable { detail: String },
    DevelopmentResetRequired { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct Contact {
    pub destination: [u8; 16],
    pub alias: Option<String>,
    pub identity: Option<[u8; 16]>,
    pub pinned: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct ContactDestinationInput {
    pub destination: [u8; 16],
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct CreateManualContactInput {
    pub destination: [u8; 16],
    pub identity: Option<[u8; 16]>,
    pub alias: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct SetContactAliasInput {
    pub destination: [u8; 16],
    pub alias: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct SetContactPinnedInput {
    pub destination: [u8; 16],
    pub pinned: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum ContactMutationOutcome {
    Saved {
        contact: Contact,
    },
    Updated {
        contact: Contact,
    },
    Deleted,
    Existing {
        contact: Contact,
    },
    AlreadyExists {
        contact: Contact,
    },
    NotFound,
    LocalNodeStopped,
    NotObserved,
    IdentityConflict {
        existing: [u8; 16],
        attempted: [u8; 16],
    },
    MissingIdentity,
    DevelopmentUnavailable {
        detail: String,
    },
    DevelopmentResetRequired {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum ContactLookupOutcome {
    Found { contact: Contact },
    NotFound,
    DevelopmentUnavailable { detail: String },
    DevelopmentResetRequired { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum ContactListOutcome {
    Listed { contacts: Vec<Contact> },
    DevelopmentUnavailable { detail: String },
    DevelopmentResetRequired { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum LxmfText {
    Utf8 { value: String },
    InvalidUtf8 { bytes: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct LxmfPeerSummary {
    pub destination: [u8; 16],
    pub display_name: Option<String>,
    pub required_stamp_cost: Option<u64>,
    pub last_observed_age_millis: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum LxmfDirection {
    Inbound,
    Outbound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum LxmfVerification {
    Verified,
    SourceUnknown,
    InvalidSignature,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum LxmfDeliveryFailure {
    NoRoute,
    LinkFailed,
    DeliveryTimedOut,
    LocalNodeStopped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum LxmfDeliveryState {
    Received,
    Queued {
        failed_attempts: u64,
    },
    Sending {
        failed_attempts: u64,
    },
    Delivered {
        delivered_at: u64,
        rtt: Option<u64>,
    },
    Failed {
        failed_attempts: u64,
        last_failure: LxmfDeliveryFailure,
    },
    Cancelled {
        cancelled_at: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct LxmfMessage {
    pub local_record_id: u64,
    pub message_id: [u8; 32],
    pub source: [u8; 16],
    pub destination: [u8; 16],
    pub timestamp: u64,
    pub title: LxmfText,
    pub content: LxmfText,
    pub direction: LxmfDirection,
    pub verification: LxmfVerification,
    pub delivery_state: LxmfDeliveryState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum LxmfHealthState {
    Ready,
    Degraded,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct LxmfHealth {
    pub state: LxmfHealthState,
    pub inbound_overflow_count: u64,
}

impl LxmfHealth {
    #[must_use]
    pub fn stopped() -> Self {
        Self {
            state: LxmfHealthState::Stopped,
            inbound_overflow_count: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct SendDirectTextInput {
    pub destination: [u8; 16],
    pub title: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct MeasureLxmfTextInput {
    pub title: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct ListLxmfMessagesInput {
    pub peer: Option<[u8; 16]>,
    pub before: Option<u64>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct RetryLxmfMessageInput {
    pub local_record_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct CancelLxmfMessageInput {
    pub local_record_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum LxmfPeerListOutcome {
    Listed { peers: Vec<LxmfPeerSummary> },
    LocalNodeStopped,
    Busy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum LxmfMessageListOutcome {
    Listed { messages: Vec<LxmfMessage> },
    InvalidInput { detail: String },
    DevelopmentUnavailable { detail: String },
    DevelopmentResetRequired { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum AnnounceLxmfOutcome {
    Announced,
    LocalNodeStopped,
    Busy,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum SendDirectTextOutcome {
    Accepted { local_record_id: u64 },
    NeedsResource { wire_bytes: u32 },
    UnsupportedRemoteStampRequirement { required_stamp_cost: u64 },
    PeerIdentityUnavailable,
    DevelopmentUnavailable { detail: String },
    DevelopmentResetRequired { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum RetryLxmfMessageOutcome {
    Accepted { local_record_id: u64 },
    NotFound,
    NotFailed { current: LxmfDeliveryState },
    DevelopmentUnavailable { detail: String },
    DevelopmentResetRequired { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum CancelLxmfMessageOutcome {
    Cancelled { local_record_id: u64 },
    NotFound,
    AlreadyDelivered,
    AlreadyCancelled,
    NotCancellable { current: LxmfDeliveryState },
    DevelopmentUnavailable { detail: String },
    DevelopmentResetRequired { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum MeasureLxmfTextOutcome {
    Measured {
        wire_bytes: u32,
        remaining_bytes: u32,
    },
    NeedsResource {
        wire_bytes: u32,
    },
    InvalidMessage,
    LocalNodeStopped,
    Busy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum RemoteControlPairingState {
    BluetoothUnavailable,
    Searching,
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

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct RemoteControlPairingCandidate {
    pub candidate_id: String,
    pub display_name: Option<String>,
    pub observed_at_millis: u64,
    pub expires_at_millis: u64,
    pub expires_in_millis: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum RemoteControlRequestKind {
    Describe,
    AnnounceSelf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct RemoteControlTargetSnapshot {
    pub target_identity_fingerprint: Vec<u8>,
    pub destination: Vec<u8>,
    pub controller_identity_fingerprint: Vec<u8>,
    pub permitted_requests: Vec<RemoteControlRequestKind>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct DevelopmentNodeOperation {
    pub kind: DevelopmentNodeOperationKind,
    pub started_at_millis: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum DevelopmentNodeOperationKind {
    Pairing,
    Describe,
    AnnounceSelf,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct DevelopmentNodeFailure {
    pub stage: DevelopmentNodeFailureStage,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum DevelopmentNodeFailureStage {
    Storage,
    Identity,
    Runtime,
    PersistenceRestore,
    Bluetooth,
    Node,
    Contract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum AppleBluetoothRestorationPreparationFailureStage {
    Contract,
    Storage,
    Identity,
    Runtime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum AppleBluetoothRestorationPreparationOutcome {
    Prepared,
    AlreadyPrepared,
    AlreadyRunning,
    Failed {
        stage: AppleBluetoothRestorationPreparationFailureStage,
        detail: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum RemoteControlPairingFailureStage {
    Input,
    Candidate,
    Route,
    Link,
    Identification,
    Request,
    Timeout,
    Confirmation,
    Persistence,
    Expired,
    Node,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum DevelopmentNodeStopStage {
    CommandAdmission,
    TargetConnection,
    Persistence,
    Node,
    Worker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
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

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
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

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum DevelopmentNodeStopOutcome {
    Stopped,
    AlreadyStopped,
    Failed {
        stage: DevelopmentNodeStopStage,
        detail: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
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

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum RemoteControlDescribeOutcome {
    Described {
        target: RemoteControlTargetSnapshot,
        available_requests: Vec<RemoteControlRequestKind>,
        rtt_millis: u64,
        snapshot: Box<DevelopmentNodeSnapshot>,
    },
    Busy,
    Failed {
        stage: RemoteControlDescribeFailureStage,
        detail: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct InitiateRemoteControlPairingInput {
    pub candidate_id: String,
    pub invitation_code: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct RemoteControlPairingDecisionInput {
    pub attempt_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct DescribeRemoteControlTargetInput {
    pub target_identity_fingerprint: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct AnnounceRemoteControlTargetInput {
    pub target_identity_fingerprint: Vec<u8>,
}

/// One process-local result, retained independently of a React subscription.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct RemoteControlAnnounceOperation {
    pub operation_id: u64,
    pub target_identity_fingerprint: Vec<u8>,
    pub status: RemoteControlAnnounceStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum RemoteControlAnnounceStatus {
    Pending,
    Announced {
        rtt_millis: u64,
    },
    Unavailable,
    Rejected,
    WriteFailed,
    Failed {
        stage: RemoteControlAnnounceFailureStage,
    },
    OutcomeUnknown {
        reason: RemoteControlAnnounceUnknownReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum RemoteControlAnnounceFailureStage {
    Busy,
    Input,
    Inventory,
    Route,
    Link,
    Identification,
    Permission,
    Request,
    Node,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum RemoteControlAnnounceUnknownReason {
    DeliveryUnconfirmed,
    Timeout,
    ConnectionLost,
    ResponseInvalid,
    NodeStopped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum RemoteControlAnnounceOutcome {
    Accepted {
        operation: RemoteControlAnnounceOperation,
        snapshot: Box<DevelopmentNodeSnapshot>,
    },
    Busy,
    Failed {
        stage: RemoteControlAnnounceFailureStage,
    },
}

impl DevelopmentNodeSnapshot {
    #[must_use]
    pub fn stopped() -> Self {
        Self {
            contract_fingerprint: CONTRACT_FINGERPRINT.to_owned(),
            revision: 0,
            generation_id: 0,
            runtime: DevelopmentNodeRuntime::Stopped,
            primary_identity: PrimaryIdentityState::Missing,
            local_host: LocalHostState::Stopped {
                last_start_failure: None,
            },
            lxmf: LxmfHealth::stopped(),
            controller_identity_fingerprint: None,
            pairing: RemoteControlPairingState::Searching,
            pairing_candidates: Vec::new(),
            paired_targets: Vec::new(),
            last_announcement: None,
            active_operation: None,
            failure: None,
        }
    }
}

#[cfg(all(test, feature = "uniffi-bindings"))]
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

    #[cfg(feature = "uniffi-bindings")]
    #[test]
    fn uniffi_full_snapshot_roundtrip_preserves_canonical_host_and_exact_integers() {
        let mut snapshot = DevelopmentNodeSnapshot::stopped();
        snapshot.generation_id = u64::MAX;
        snapshot.revision = 1_u64 << 53;
        snapshot.local_host = LocalHostState::Running {
            host: Box::new(host_fixture()),
        };
        let bytes =
            <DevelopmentNodeSnapshot as uniffi::Lower<crate::UniFfiTag>>::lower(snapshot.clone());
        let decoded = <DevelopmentNodeSnapshot as uniffi::Lift<crate::UniFfiTag>>::try_lift(bytes)
            .expect("typed snapshot");
        assert_eq!(decoded, snapshot);
    }

    #[test]
    fn stopped_snapshot_carries_the_contract_fingerprint() {
        assert_eq!(
            DevelopmentNodeSnapshot::stopped().contract_fingerprint,
            CONTRACT_FINGERPRINT
        );
    }
}
