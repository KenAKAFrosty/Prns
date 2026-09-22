use super::{RemoteControlAnnounceUnknownReason, RemoteControlRequestKind};

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct ReadRemoteNodeInput {
    pub target_identity_fingerprint: Vec<u8>,
    pub query: RemoteNodeQuery,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum RemoteNodeQuery {
    Overview,
    Interfaces {
        after: Option<Vec<u8>>,
    },
    Interface {
        interface_id: Vec<u8>,
    },
    Peers {
        interface_id: Vec<u8>,
        after: Option<Vec<u8>>,
    },
    Controllers {
        after: Option<Vec<u8>>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum ReadRemoteNodeOutcome {
    Read {
        available_requests: Vec<RemoteControlRequestKind>,
        data: RemoteNodeData,
        rtt_millis: u64,
    },
    Busy,
    Failed {
        stage: RemoteManagementFailureStage,
        detail: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum RemoteNodeData {
    Overview { overview: RemoteNodeOverview },
    Interfaces { page: RemoteInterfacePage },
    Interface { details: RemoteInterfaceDetails },
    Peers { page: RemotePeerPage },
    Controllers { page: RemoteControllerPage },
}

/// The node reports identities only, not names, authority or individual permissions.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct RemoteControllerPage {
    pub identities: Vec<Vec<u8>>,
    pub next: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct RemoteNodeOverview {
    /// None means the live capability set does not offer this observation.
    pub firmware: Option<String>,
    pub power: Option<RemoteNodePower>,
    pub interfaces: Option<RemoteInterfacePage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct RemoteNodePower {
    pub battery_percent: Option<u8>,
    pub external_power: RemoteExternalPower,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum RemoteExternalPower {
    Unknown,
    Absent,
    Present,
    Charging,
    Idle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct RemoteInterfacePage {
    pub entries: Vec<RemoteInterfaceEntry>,
    pub next: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct RemoteInterfaceEntry {
    pub interface_id: Vec<u8>,
    pub kind: String,
    pub mode: RemoteInterfaceMode,
    pub connection: RemoteConnectionState,
    pub enabled: bool,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub links: u32,
    pub rate_bytes_per_sec: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum RemoteInterfaceMode {
    Full,
    PointToPoint,
    AccessPoint,
    Roaming,
    Boundary,
    Gateway,
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum RemoteConnectionState {
    Initializing,
    Connected,
    Degraded,
    Reconnecting,
    Failed,
    Disconnected,
    Disabled,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct RemoteInterfaceDetails {
    pub interface_id: Vec<u8>,
    pub configuration: RemoteInterfaceConfiguration,
    pub discovery_groups: RemoteDiscoveryGroups,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum RemoteInterfaceConfiguration {
    Available { card: RemoteInterfaceCard },
    Unavailable,
    UnknownInterface,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct RemoteInterfaceCard {
    pub name: String,
    pub group: String,
    pub configuration: String,
    pub failure: String,
    pub destinations: u32,
    pub transported_links: u32,
    pub lora_profile: Option<RemoteLoRaProfile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct RemoteLoRaProfile {
    pub region: RemoteLoRaRegion,
    pub frequency_hz: u32,
    pub spreading_factor: u8,
    pub bandwidth_hz: u32,
    /// Denominator of the coding rate (5 means 4/5).
    pub coding_rate: u8,
    pub tx_power_dbm: i8,
    pub preamble_symbols: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum RemoteLoRaRegion {
    Us915,
    Au915,
    Eu433,
    Eu865,
    Eu868,
    Eu869,
    As923,
    In865,
    Cn470,
    Kr920,
    Jp920,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum RemoteDiscoveryGroups {
    Available { groups: Vec<String> },
    Unavailable,
    UnknownInterface,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct RemotePeerPage {
    pub interface_id: Vec<u8>,
    pub entries: Vec<RemotePeerEntry>,
    pub next: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct RemotePeerEntry {
    pub peer_id: Vec<u8>,
    pub connection: RemoteConnectionState,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub links: u32,
    pub destinations: u32,
    pub rate_bytes_per_sec: u32,
    pub radio: RemotePeerRadio,
    pub details: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum RemotePeerRadio {
    NotRadio,
    Pending {
        family: RemoteRadioFamily,
    },
    Unavailable {
        family: RemoteRadioFamily,
    },
    Measured {
        family: RemoteRadioFamily,
        rssi_dbm: i16,
        snr_quarter_db: Option<i16>,
        quality_tenths_percent: Option<u16>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum RemoteRadioFamily {
    Bluetooth,
    Wifi,
    LoRa,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct ChangeRemoteNodeInput {
    pub target_identity_fingerprint: Vec<u8>,
    pub change: RemoteNodeChange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum RemoteNodeChange {
    InterfacePower {
        interface_id: Vec<u8>,
        enabled: bool,
    },
    InterfaceMode {
        interface_id: Vec<u8>,
        mode: RemoteInterfaceMode,
    },
    InterfaceGroup {
        interface_id: Vec<u8>,
        group: String,
    },
    InterfaceLoRa {
        interface_id: Vec<u8>,
        profile: RemoteLoRaProfile,
    },
    DiscoveryGroups {
        interface_id: Vec<u8>,
        groups: Vec<String>,
    },
    GnssPower {
        enabled: bool,
    },
    DisplayVisibility {
        visible: bool,
    },
    DisplayAutoOff {
        enabled: bool,
    },
    SystemPower {
        awake: bool,
    },
    StationUplink {
        interface_id: Vec<u8>,
        enabled: bool,
    },
    RadioMode {
        mode: RemoteRadioMode,
    },
    SleepRadios,
    WakeRadios,
    RevokeController {
        controller_identity_fingerprint: Vec<u8>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum RemoteRadioMode {
    Bluetooth,
    AccessPoint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum RemoteManagementFailureStage {
    Input,
    Node,
    Inventory,
    Route,
    Link,
    Identification,
    Permission,
    Unsupported,
    UnknownInterface,
    Request,
    Timeout,
    Busy,
    Response,
    Persistence,
    Rollback,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum RemoteChangeStatus {
    Pending,
    Applied,
    Unchanged,
    Scheduled,
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
pub struct RemoteChangeOperation {
    pub operation_id: u64,
    pub generation_id: u64,
    pub target_identity_fingerprint: Vec<u8>,
    pub change: RemoteNodeChange,
    pub status: RemoteChangeStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
pub enum ChangeRemoteNodeOutcome {
    Accepted {
        operation: RemoteChangeOperation,
    },
    Busy,
    Failed {
        stage: RemoteManagementFailureStage,
        detail: String,
    },
}
