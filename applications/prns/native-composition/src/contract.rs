mod remote_management;
pub use remote_management::*;
mod remote_wifi;
pub use remote_wifi::*;

pub const CONTRACT_FINGERPRINT: &str = env!("PRNS_APP_CONTRACT_FINGERPRINT");
pub const HOST_CONTRACT_FINGERPRINT: &str = prns_host::HOST_SEMANTIC_FINGERPRINT;

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
    pub bluetooth: LocalBluetoothSnapshot,
    pub network: LocalNetworkSnapshot,
    pub lxmf: LxmfHealth,
    pub controller_identity_fingerprint: Option<Vec<u8>>,
    pub pairing: RemoteControlPairingState,
    pub pairing_candidates: Vec<RemoteControlPairingCandidate>,
    pub paired_targets: Vec<RemoteControlTargetSnapshot>,
    pub last_announcement: Option<RemoteControlAnnounceOperation>,
    pub last_remote_change: Option<RemoteChangeOperation>,
    pub last_remote_wifi: Option<RemoteWifiOperation>,
    pub active_operation: Option<DevelopmentNodeOperation>,
    pub failure: Option<DevelopmentNodeFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct DevelopmentNodeStartInput {
    pub development_tcp_target: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct LocalBluetoothSnapshot {
    /// None until the application preference has been successfully read.
    pub desired_enabled: Option<bool>,
    pub state: LocalBluetoothState,
    /// Physical Bluetooth fleet members, never routes or RNS links.
    pub peers: Vec<LocalBluetoothPeerSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum LocalBluetoothState {
    Stopped,
    Starting,
    Disabled,
    Disabling,
    Connecting,
    WaitingForPeers,
    Connected,
    RadioOff,
    Unavailable { detail: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct LocalBluetoothPeerSnapshot {
    /// Opaque physical interface identifier; this is not an authenticated RNS identity.
    pub interface_id: Vec<u8>,
    pub name: Option<String>,
    pub connected: bool,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub details: Option<String>,
    pub rssi_dbm: Option<i16>,
}

/// App presentation over one native inspection. Ages are measured at that
/// inspection, not live JavaScript-clock estimates or historical message paths.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct LocalNetworkSnapshot {
    pub state: LocalNetworkState,
    pub routes: Vec<LocalNetworkRouteSnapshot>,
    /// Newest admission first; at most 200 rows from this native generation.
    pub announces: Vec<LocalAnnounceActivity>,
    pub activity_revision: u64,
    /// Retention-window evictions since clear, not packet or Bluetooth loss.
    pub dropped_announce_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum LocalNetworkState {
    Stopped,
    Starting,
    Ready,
    /// Inspection failed; this does not establish that the network is down.
    Unavailable {
        detail: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct LocalNetworkRouteSnapshot {
    pub destination: [u8; 16],
    pub via_identity: Option<[u8; 16]>,
    /// The canonical logical interface, not an inferred physical next hop.
    pub interface_id: Vec<u8>,
    pub hops: u8,
    pub learned_age_millis: u64,
    pub last_activity_age_millis: u64,
    pub expires_in_millis: u64,
    pub expired: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct LocalAnnounceActivity {
    /// Unique within the aggregate generation; clear never reuses record IDs.
    pub record_id: u64,
    pub destination: [u8; 16],
    pub announced_identity: [u8; 16],
    /// Exact ingress when accepted, even after that interface is retired.
    pub source_interface: Vec<u8>,
    pub hops: u8,
    pub age_millis: u64,
    pub is_path_response: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct ClearNetworkActivityInput {
    pub generation_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum ClearNetworkActivityOutcome {
    Cleared { activity_revision: u64 },
    GenerationChanged,
    LocalNodeStopped,
    Busy,
}

impl LocalNetworkSnapshot {
    pub fn stopped() -> Self {
        Self {
            state: LocalNetworkState::Stopped,
            routes: Vec::new(),
            announces: Vec::new(),
            activity_revision: 0,
            dropped_announce_count: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum LocalBluetoothSettingsOutcome {
    Ready { enabled: bool },
    Busy,
    Unavailable { detail: String },
}

impl LocalBluetoothSnapshot {
    pub fn stopped(desired_enabled: Option<bool>) -> Self {
        Self {
            desired_enabled,
            state: LocalBluetoothState::Stopped,
            peers: Vec::new(),
        }
    }
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
    pub announced_name: Option<String>,
    pub identity: Option<[u8; 16]>,
    pub pinned: bool,
    pub is_messaging: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct LocalMessagingProfile {
    pub display_name: String,
    pub destination: Option<[u8; 16]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum LocalMessagingProfileOutcome {
    Ready {
        profile: LocalMessagingProfile,
    },
    SavedButNotApplied {
        profile: LocalMessagingProfile,
        detail: String,
    },
    InvalidInput {
        detail: String,
    },
    Busy,
    Unavailable {
        detail: String,
    },
    DevelopmentResetRequired {
        reason: String,
    },
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
    pub identity: [u8; 16],
    pub display_name: Option<String>,
    pub required_stamp_cost: Option<u64>,
    pub last_observed_age_millis: u64,
    pub source_interface: Vec<u8>,
    pub hops: u8,
    pub is_path_response: bool,
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
    /// Committed mailbox changes, independent of unrelated network snapshots.
    pub mailbox_revision: u64,
    /// Process-local LXMF changes, including active sends and discovered peers.
    pub projection_revision: u64,
}

impl LxmfHealth {
    #[must_use]
    pub fn stopped() -> Self {
        Self {
            state: LxmfHealthState::Stopped,
            inbound_overflow_count: 0,
            mailbox_revision: 0,
            projection_revision: 0,
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
pub struct ListLxmfConversationsInput {
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
    Requested,
    NoUsableConnection,
    LocalNodeStopped,
    Busy,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum LxmfDiscoveryClearOutcome {
    Cleared,
    LocalNodeStopped,
    Busy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum SendDirectTextOutcome {
    Accepted {
        local_record_id: u64,
    },
    NeedsResource {
        wire_bytes: u32,
    },
    UnsupportedRemoteStampRequirement {
        required_stamp_cost: u64,
    },
    PeerIdentityUnavailable,
    IdentityConflict {
        expected: [u8; 16],
        observed: [u8; 16],
    },
    RecipientUnavailable {
        detail: String,
    },
    DevelopmentUnavailable {
        detail: String,
    },
    DevelopmentResetRequired {
        reason: String,
    },
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
        authority: RemoteControlControllerAuthority,
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
    InventoryInterfaces,
    SetInterfacePower,
    SleepRadios,
    WakeRadios,
    SetInterfaceMode,
    SetInterfaceGroup,
    InventoryInterfacePeers,
    InventoryInterfaceConfig,
    SetInterfaceLoRaProfile,
    DescribeBuild,
    SetInterfaceWifiStation,
    InventoryControllers,
    AuthorizeController,
    RevokeController,
    DescribePower,
    SetSystemPower,
    SetGnssPower,
    SetDisplayVisibility,
    SetDisplayAutoOff,
    SetStationUplink,
    SetEspRadioMode,
    StageWifiCredentials,
    ActivateWifiCredentials,
    ConfirmWifiCredentials,
    CancelWifiCredentials,
    InspectWifiTransaction,
    InventoryInterfaceDiscoveryGroups,
    ReplaceInterfaceDiscoveryGroups,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum RemoteControlControllerAuthority {
    Operator,
    Administrator,
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
    RemoteRead,
    RemoteChange,
    RemoteWifi,
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
            bluetooth: LocalBluetoothSnapshot::stopped(None),
            network: LocalNetworkSnapshot::stopped(),
            lxmf: LxmfHealth::stopped(),
            controller_identity_fingerprint: None,
            pairing: RemoteControlPairingState::Searching,
            pairing_candidates: Vec::new(),
            paired_targets: Vec::new(),
            last_announcement: None,
            last_remote_change: None,
            last_remote_wifi: None,
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
        snapshot.bluetooth = LocalBluetoothSnapshot {
            desired_enabled: Some(false),
            state: LocalBluetoothState::Disabling,
            peers: vec![LocalBluetoothPeerSnapshot {
                interface_id: vec![0x28; 8],
                name: None,
                connected: true,
                rx_bytes: u64::MAX,
                tx_bytes: 1_u64 << 53,
                details: Some("CoC".into()),
                rssi_dbm: Some(-60),
            }],
        };
        snapshot.network = LocalNetworkSnapshot {
            state: LocalNetworkState::Ready,
            routes: vec![LocalNetworkRouteSnapshot {
                destination: [0x51; 16],
                via_identity: Some([0x52; 16]),
                interface_id: vec![0x53; 8],
                hops: 2,
                learned_age_millis: u64::MAX,
                last_activity_age_millis: (1_u64 << 53) + 1,
                expires_in_millis: u64::MAX,
                expired: false,
            }],
            announces: vec![LocalAnnounceActivity {
                record_id: u64::MAX,
                destination: [0x51; 16],
                announced_identity: [0x52; 16],
                source_interface: vec![0x54; 8],
                hops: 3,
                age_millis: (1_u64 << 53) + 1,
                is_path_response: true,
            }],
            activity_revision: u64::MAX,
            dropped_announce_count: (1_u64 << 53) + 1,
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
