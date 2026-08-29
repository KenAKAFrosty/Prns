pub const HOST_SCHEMA_VERSION: u32 = 1;
pub const HOST_SCHEMA_ABI: u32 = 1;
pub const HOST_SCHEMA_PRODUCT_VERSION: &str = "0.3.7";
pub const DESTINATION_HASH_LENGTH: usize = 16;
pub const IDENTITY_HASH_LENGTH: usize = 16;
pub const INTERFACE_ID_LENGTH: usize = 8;
pub const LINK_ID_LENGTH: usize = 16;
pub const PACKET_HASH_LENGTH: usize = 32;
pub const REQUEST_ID_LENGTH: usize = 16;
pub const REQUEST_PATH_HASH_LENGTH: usize = 16;
pub const RESOURCE_HASH_LENGTH: usize = 32;
pub const IDENTITY_SECRET_LENGTH: usize = 64;
pub const SAFE_INT_MIN: i64 = -9007199254740991;
pub const SAFE_INT_MAX: i64 = 9007199254740991;
pub const SAFE_UINT_MAX: u64 = 9007199254740991;
pub const BALANCED_PENDING_COMMANDS: usize = 256;
pub const BALANCED_APPLICATION_EVENTS: usize = 1024;
pub const BALANCED_RETAINED_EVENT_BYTES: usize = 8388608;
pub const BALANCED_DIAGNOSTICS: usize = 1024;
pub const HOST_OPERATION_NAMES: &[&str] = &[
    "contractInfo",
    "backendInfo",
    "hostCreate",
    "hostRelease",
    "hostLifecycle",
    "hostSnapshot",
    "hostSnapshotRead",
    "hostSnapshotRelease",
    "hostIdentityHash",
    "hostDestinationCount",
    "hostDestinationHash",
    "hostAttachSuppliedPipe",
    "suppliedPipeClaimAttachment",
    "suppliedPipeNextOpenRequest",
    "suppliedPipeRegisterReadiness",
    "suppliedPipeInterruptWait",
    "suppliedPipeRelease",
    "suppliedPipeOpenRequestProvide",
    "suppliedPipeOpenRequestDecline",
    "suppliedPipeOpenRequestRelease",
    "hostBeginResourceUpload",
    "resourceUploadWrite",
    "resourceUploadIsWritable",
    "resourceUploadFinish",
    "resourceUploadAbort",
    "resourceUploadRelease",
    "hostStop",
    "commandWait",
    "commandRegisterReadiness",
    "commandInterruptWait",
    "commandRelease",
    "hostClaimApplicationEvents",
    "hostClaimDiagnostics",
    "eventStreamRegisterReadiness",
    "readinessRegistrationRelease",
    "eventStreamInterruptWait",
    "eventStreamRelease",
    "eventStreamNext",
    "eventRelease",
    "eventKind",
    "eventBytes",
    "eventString",
    "eventU64",
    "eventU128",
    "eventResourceStream",
    "resourceStreamRelease",
    "resourceStreamNext",
    "hostAnnounce",
    "hostSendSinglePacket",
    "hostCloseLink",
    "hostAttachTcpServer",
    "hostAttachTcpClient",
    "hostAttachUdp",
    "hostDetachInterface",
    "hostEstablishLink",
    "hostRequestPath",
    "hostIdentify",
    "hostSendLinkPacket",
    "hostRequest",
    "hostRespond",
    "hostSendResource",
    "hostSetLinkResourceStrategy",
    "hostSetDestinationResourceStrategy",
    "hostSendChannelMessage",
    "hostAllowRequester",
    "hostAttachInterface",
];

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Status {
    Ok = 0,
    InvalidArgument = 1,
    ContractMismatch = 2,
    InvalidHandle = 3,
    NotReady = 4,
    AlreadyClaimed = 5,
    WouldBlock = 6,
    TimedOut = 7,
    QueueFull = 8,
    Stopped = 9,
    BackendFailed = 10,
    Panic = 11,
    Interrupted = 12,
    Unsupported = 13,
    PermissionDenied = 14,
    Unavailable = 15,
}

impl Status {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::Ok => "Ok",
            Self::InvalidArgument => "InvalidArgument",
            Self::ContractMismatch => "ContractMismatch",
            Self::InvalidHandle => "InvalidHandle",
            Self::NotReady => "NotReady",
            Self::AlreadyClaimed => "AlreadyClaimed",
            Self::WouldBlock => "WouldBlock",
            Self::TimedOut => "TimedOut",
            Self::QueueFull => "QueueFull",
            Self::Stopped => "Stopped",
            Self::BackendFailed => "BackendFailed",
            Self::Panic => "Panic",
            Self::Interrupted => "Interrupted",
            Self::Unsupported => "Unsupported",
            Self::PermissionDenied => "PermissionDenied",
            Self::Unavailable => "Unavailable",
        }
    }
}

impl TryFrom<u32> for Status {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Ok),
            1 => Ok(Self::InvalidArgument),
            2 => Ok(Self::ContractMismatch),
            3 => Ok(Self::InvalidHandle),
            4 => Ok(Self::NotReady),
            5 => Ok(Self::AlreadyClaimed),
            6 => Ok(Self::WouldBlock),
            7 => Ok(Self::TimedOut),
            8 => Ok(Self::QueueFull),
            9 => Ok(Self::Stopped),
            10 => Ok(Self::BackendFailed),
            11 => Ok(Self::Panic),
            12 => Ok(Self::Interrupted),
            13 => Ok(Self::Unsupported),
            14 => Ok(Self::PermissionDenied),
            15 => Ok(Self::Unavailable),
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BackendKind {
    Native = 1,
    Browser = 2,
    Cooperative = 3,
}

impl BackendKind {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::Native => "Native",
            Self::Browser => "Browser",
            Self::Cooperative => "Cooperative",
        }
    }
}

impl TryFrom<u32> for BackendKind {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Native),
            2 => Ok(Self::Browser),
            3 => Ok(Self::Cooperative),
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Capability {
    Loopback = 1,
    TcpClient = 2,
    TcpServer = 3,
    Udp = 4,
    Serial = 5,
    Usb = 6,
    Bluetooth = 7,
    Wifi = 8,
    WebSocket = 9,
    BrowserRendezvous = 10,
    I2p = 11,
    Weave = 12,
    SuppliedPipe = 13,
}

impl Capability {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::Loopback => "Loopback",
            Self::TcpClient => "TcpClient",
            Self::TcpServer => "TcpServer",
            Self::Udp => "Udp",
            Self::Serial => "Serial",
            Self::Usb => "Usb",
            Self::Bluetooth => "Bluetooth",
            Self::Wifi => "Wifi",
            Self::WebSocket => "WebSocket",
            Self::BrowserRendezvous => "BrowserRendezvous",
            Self::I2p => "I2p",
            Self::Weave => "Weave",
            Self::SuppliedPipe => "SuppliedPipe",
        }
    }
}

impl TryFrom<u32> for Capability {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Loopback),
            2 => Ok(Self::TcpClient),
            3 => Ok(Self::TcpServer),
            4 => Ok(Self::Udp),
            5 => Ok(Self::Serial),
            6 => Ok(Self::Usb),
            7 => Ok(Self::Bluetooth),
            8 => Ok(Self::Wifi),
            9 => Ok(Self::WebSocket),
            10 => Ok(Self::BrowserRendezvous),
            11 => Ok(Self::I2p),
            12 => Ok(Self::Weave),
            13 => Ok(Self::SuppliedPipe),
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InterfaceKind {
    AutoLan = 1,
    TcpClient = 2,
    TcpServer = 3,
    Udp = 4,
    Serial = 5,
    Kiss = 6,
    Ax25Kiss = 7,
    RNode = 8,
    MultiRNode = 9,
    Pipe = 10,
    BackboneClient = 11,
    BackboneServer = 12,
    I2p = 13,
    Weave = 14,
    AutomaticUsb = 15,
    AutomaticBluetoothLe = 16,
    WebSocketClient = 17,
    WebSocketServer = 18,
    BrowserRendezvous = 19,
}

impl InterfaceKind {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::AutoLan => "AutoLan",
            Self::TcpClient => "TcpClient",
            Self::TcpServer => "TcpServer",
            Self::Udp => "Udp",
            Self::Serial => "Serial",
            Self::Kiss => "Kiss",
            Self::Ax25Kiss => "Ax25Kiss",
            Self::RNode => "RNode",
            Self::MultiRNode => "MultiRNode",
            Self::Pipe => "Pipe",
            Self::BackboneClient => "BackboneClient",
            Self::BackboneServer => "BackboneServer",
            Self::I2p => "I2p",
            Self::Weave => "Weave",
            Self::AutomaticUsb => "AutomaticUsb",
            Self::AutomaticBluetoothLe => "AutomaticBluetoothLe",
            Self::WebSocketClient => "WebSocketClient",
            Self::WebSocketServer => "WebSocketServer",
            Self::BrowserRendezvous => "BrowserRendezvous",
        }
    }
}

impl TryFrom<u32> for InterfaceKind {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::AutoLan),
            2 => Ok(Self::TcpClient),
            3 => Ok(Self::TcpServer),
            4 => Ok(Self::Udp),
            5 => Ok(Self::Serial),
            6 => Ok(Self::Kiss),
            7 => Ok(Self::Ax25Kiss),
            8 => Ok(Self::RNode),
            9 => Ok(Self::MultiRNode),
            10 => Ok(Self::Pipe),
            11 => Ok(Self::BackboneClient),
            12 => Ok(Self::BackboneServer),
            13 => Ok(Self::I2p),
            14 => Ok(Self::Weave),
            15 => Ok(Self::AutomaticUsb),
            16 => Ok(Self::AutomaticBluetoothLe),
            17 => Ok(Self::WebSocketClient),
            18 => Ok(Self::WebSocketServer),
            19 => Ok(Self::BrowserRendezvous),
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InterfaceMode {
    Full = 1,
    PointToPoint = 2,
    AccessPoint = 3,
    Roaming = 4,
    Boundary = 5,
    Gateway = 6,
    Internal = 7,
}

impl InterfaceMode {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::Full => "Full",
            Self::PointToPoint => "PointToPoint",
            Self::AccessPoint => "AccessPoint",
            Self::Roaming => "Roaming",
            Self::Boundary => "Boundary",
            Self::Gateway => "Gateway",
            Self::Internal => "Internal",
        }
    }
}

impl TryFrom<u32> for InterfaceMode {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Full),
            2 => Ok(Self::PointToPoint),
            3 => Ok(Self::AccessPoint),
            4 => Ok(Self::Roaming),
            5 => Ok(Self::Boundary),
            6 => Ok(Self::Gateway),
            7 => Ok(Self::Internal),
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WebSocketFramingSelection {
    RawPacket = 1,
    Hdlc = 2,
    Kiss = 3,
    Auto = 4,
}

impl WebSocketFramingSelection {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::RawPacket => "RawPacket",
            Self::Hdlc => "Hdlc",
            Self::Kiss => "Kiss",
            Self::Auto => "Auto",
        }
    }
}

impl TryFrom<u32> for WebSocketFramingSelection {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::RawPacket),
            2 => Ok(Self::Hdlc),
            3 => Ok(Self::Kiss),
            4 => Ok(Self::Auto),
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InterfaceHealth {
    Initializing = 1,
    Connected = 2,
    Degraded = 3,
    Reconnecting = 4,
    Failed = 5,
    Disconnected = 6,
    Disabled = 7,
    Unknown = 8,
}

impl InterfaceHealth {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::Initializing => "Initializing",
            Self::Connected => "Connected",
            Self::Degraded => "Degraded",
            Self::Reconnecting => "Reconnecting",
            Self::Failed => "Failed",
            Self::Disconnected => "Disconnected",
            Self::Disabled => "Disabled",
            Self::Unknown => "Unknown",
        }
    }
}

impl TryFrom<u32> for InterfaceHealth {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Initializing),
            2 => Ok(Self::Connected),
            3 => Ok(Self::Degraded),
            4 => Ok(Self::Reconnecting),
            5 => Ok(Self::Failed),
            6 => Ok(Self::Disconnected),
            7 => Ok(Self::Disabled),
            8 => Ok(Self::Unknown),
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DiscoveryScope {
    Link = 1,
    Admin = 2,
    Site = 3,
    Organization = 4,
    Global = 5,
}

impl DiscoveryScope {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::Link => "Link",
            Self::Admin => "Admin",
            Self::Site => "Site",
            Self::Organization => "Organization",
            Self::Global => "Global",
        }
    }
}

impl TryFrom<u32> for DiscoveryScope {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Link),
            2 => Ok(Self::Admin),
            3 => Ok(Self::Site),
            4 => Ok(Self::Organization),
            5 => Ok(Self::Global),
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MulticastAddressType {
    Temporary = 1,
    Permanent = 2,
}

impl MulticastAddressType {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::Temporary => "Temporary",
            Self::Permanent => "Permanent",
        }
    }
}

impl TryFrom<u32> for MulticastAddressType {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Temporary),
            2 => Ok(Self::Permanent),
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SerialDataBits {
    Five = 5,
    Six = 6,
    Seven = 7,
    Eight = 8,
}

impl SerialDataBits {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::Five => "Five",
            Self::Six => "Six",
            Self::Seven => "Seven",
            Self::Eight => "Eight",
        }
    }
}

impl TryFrom<u32> for SerialDataBits {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            5 => Ok(Self::Five),
            6 => Ok(Self::Six),
            7 => Ok(Self::Seven),
            8 => Ok(Self::Eight),
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SerialParity {
    None = 1,
    Even = 2,
    Odd = 3,
}

impl SerialParity {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Even => "Even",
            Self::Odd => "Odd",
        }
    }
}

impl TryFrom<u32> for SerialParity {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::None),
            2 => Ok(Self::Even),
            3 => Ok(Self::Odd),
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SerialStopBits {
    One = 1,
    Two = 2,
}

impl SerialStopBits {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::One => "One",
            Self::Two => "Two",
        }
    }
}

impl TryFrom<u32> for SerialStopBits {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::One),
            2 => Ok(Self::Two),
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HostRole {
    Endpoint = 1,
    Transport = 2,
}

impl HostRole {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::Endpoint => "Endpoint",
            Self::Transport => "Transport",
        }
    }
}

impl TryFrom<u32> for HostRole {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Endpoint),
            2 => Ok(Self::Transport),
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IdentityConfigKind {
    Existing = 1,
    GenerateEphemeral = 2,
    LoadOrCreate = 3,
}

impl IdentityConfigKind {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::Existing => "Existing",
            Self::GenerateEphemeral => "GenerateEphemeral",
            Self::LoadOrCreate => "LoadOrCreate",
        }
    }
}

impl TryFrom<u32> for IdentityConfigKind {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Existing),
            2 => Ok(Self::GenerateEphemeral),
            3 => Ok(Self::LoadOrCreate),
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PersistenceConfigKind {
    Ephemeral = 1,
    Directory = 2,
}

impl PersistenceConfigKind {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::Ephemeral => "Ephemeral",
            Self::Directory => "Directory",
        }
    }
}

impl TryFrom<u32> for PersistenceConfigKind {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Ephemeral),
            2 => Ok(Self::Directory),
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DestinationConfigKind {
    Plain = 1,
    Single = 2,
}

impl DestinationConfigKind {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::Plain => "Plain",
            Self::Single => "Single",
        }
    }
}

impl TryFrom<u32> for DestinationConfigKind {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Plain),
            2 => Ok(Self::Single),
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DestinationIdentityConfigKind {
    HostIdentity = 1,
    DedicatedIdentity = 2,
}

impl DestinationIdentityConfigKind {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::HostIdentity => "HostIdentity",
            Self::DedicatedIdentity => "DedicatedIdentity",
        }
    }
}

impl TryFrom<u32> for DestinationIdentityConfigKind {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::HostIdentity),
            2 => Ok(Self::DedicatedIdentity),
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BitrateKind {
    Auto = 1,
    BitsPerSecond = 2,
}

impl BitrateKind {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::BitsPerSecond => "BitsPerSecond",
        }
    }
}

impl TryFrom<u32> for BitrateKind {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Auto),
            2 => Ok(Self::BitsPerSecond),
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ResponseTimeoutKind {
    LinkDefault = 1,
    Exact = 2,
}

impl ResponseTimeoutKind {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::LinkDefault => "LinkDefault",
            Self::Exact => "Exact",
        }
    }
}

impl TryFrom<u32> for ResponseTimeoutKind {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::LinkDefault),
            2 => Ok(Self::Exact),
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ResourceCompressionKind {
    Auto = 1,
    Never = 2,
}

impl ResourceCompressionKind {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Never => "Never",
        }
    }
}

impl TryFrom<u32> for ResourceCompressionKind {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Auto),
            2 => Ok(Self::Never),
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ResourceStrategyKind {
    Refuse = 1,
    Accept = 2,
}

impl ResourceStrategyKind {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::Refuse => "Refuse",
            Self::Accept => "Accept",
        }
    }
}

impl TryFrom<u32> for ResourceStrategyKind {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Refuse),
            2 => Ok(Self::Accept),
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RequestPolicy {
    AllowNone = 1,
    AllowAll = 2,
    AllowList = 3,
}

impl RequestPolicy {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::AllowNone => "AllowNone",
            Self::AllowAll => "AllowAll",
            Self::AllowList => "AllowList",
        }
    }
}

impl TryFrom<u32> for RequestPolicy {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::AllowNone),
            2 => Ok(Self::AllowAll),
            3 => Ok(Self::AllowList),
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CommandOutcomeKind {
    Announced = 1,
    PacketDelivered = 2,
    LinkCloseQueued = 3,
    InterfaceAttached = 4,
    InterfaceDetached = 5,
    LinkEstablished = 6,
    PathDiscovered = 7,
    Identified = 8,
    ResponseReceived = 9,
    ResponseSent = 10,
    ResourceSent = 11,
    ResourceStrategySet = 12,
    RequesterAllowed = 13,
}

impl CommandOutcomeKind {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::Announced => "Announced",
            Self::PacketDelivered => "PacketDelivered",
            Self::LinkCloseQueued => "LinkCloseQueued",
            Self::InterfaceAttached => "InterfaceAttached",
            Self::InterfaceDetached => "InterfaceDetached",
            Self::LinkEstablished => "LinkEstablished",
            Self::PathDiscovered => "PathDiscovered",
            Self::Identified => "Identified",
            Self::ResponseReceived => "ResponseReceived",
            Self::ResponseSent => "ResponseSent",
            Self::ResourceSent => "ResourceSent",
            Self::ResourceStrategySet => "ResourceStrategySet",
            Self::RequesterAllowed => "RequesterAllowed",
        }
    }
}

impl TryFrom<u32> for CommandOutcomeKind {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Announced),
            2 => Ok(Self::PacketDelivered),
            3 => Ok(Self::LinkCloseQueued),
            4 => Ok(Self::InterfaceAttached),
            5 => Ok(Self::InterfaceDetached),
            6 => Ok(Self::LinkEstablished),
            7 => Ok(Self::PathDiscovered),
            8 => Ok(Self::Identified),
            9 => Ok(Self::ResponseReceived),
            10 => Ok(Self::ResponseSent),
            11 => Ok(Self::ResourceSent),
            12 => Ok(Self::ResourceStrategySet),
            13 => Ok(Self::RequesterAllowed),
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CommandFailureKind {
    NodeStopped = 1,
    Busy = 2,
    PayloadTooLarge = 3,
    UnknownDestination = 4,
    NotSingleDestination = 5,
    AnnounceAppDataTooLong = 6,
    UnknownInterface = 7,
    NoRouteToDestination = 8,
    NotDirectlyReachable = 9,
    PacketCulled = 10,
    DeliveryTimedOut = 11,
    InvalidBitrate = 12,
    BindFailed = 13,
    WriteFailed = 14,
    UnsupportedByBackend = 15,
    UnknownLink = 16,
    LinkNotActive = 17,
    EntropyUnavailable = 18,
    NotLinkInitiator = 19,
    IdentityNotHeld = 20,
    UnknownRequestHandler = 21,
    RequestPolicyNotAllowList = 22,
    RequestAllowListFull = 23,
    LinkBusy = 24,
    ResourceTableFull = 25,
    ResourceMetadataTooLarge = 26,
    ResourceRejectedByPeer = 27,
    ResourceSequencingFailed = 28,
    ResourcePredecessorFailed = 29,
    ChannelWindowFull = 30,
    ChannelUntrackable = 31,
    InvalidChannelMessageType = 32,
    InvalidConfiguration = 33,
    ResourceUploadCancelled = 34,
    ResourceEarlyEof = 35,
    ResourceLengthOverrun = 36,
    PermissionDenied = 37,
    DeviceUnavailable = 38,
    ConnectFailed = 39,
    BackendFailed = 40,
    ResponseTooLarge = 41,
}

impl CommandFailureKind {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::NodeStopped => "NodeStopped",
            Self::Busy => "Busy",
            Self::PayloadTooLarge => "PayloadTooLarge",
            Self::UnknownDestination => "UnknownDestination",
            Self::NotSingleDestination => "NotSingleDestination",
            Self::AnnounceAppDataTooLong => "AnnounceAppDataTooLong",
            Self::UnknownInterface => "UnknownInterface",
            Self::NoRouteToDestination => "NoRouteToDestination",
            Self::NotDirectlyReachable => "NotDirectlyReachable",
            Self::PacketCulled => "PacketCulled",
            Self::DeliveryTimedOut => "DeliveryTimedOut",
            Self::InvalidBitrate => "InvalidBitrate",
            Self::BindFailed => "BindFailed",
            Self::WriteFailed => "WriteFailed",
            Self::UnsupportedByBackend => "UnsupportedByBackend",
            Self::UnknownLink => "UnknownLink",
            Self::LinkNotActive => "LinkNotActive",
            Self::EntropyUnavailable => "EntropyUnavailable",
            Self::NotLinkInitiator => "NotLinkInitiator",
            Self::IdentityNotHeld => "IdentityNotHeld",
            Self::UnknownRequestHandler => "UnknownRequestHandler",
            Self::RequestPolicyNotAllowList => "RequestPolicyNotAllowList",
            Self::RequestAllowListFull => "RequestAllowListFull",
            Self::LinkBusy => "LinkBusy",
            Self::ResourceTableFull => "ResourceTableFull",
            Self::ResourceMetadataTooLarge => "ResourceMetadataTooLarge",
            Self::ResourceRejectedByPeer => "ResourceRejectedByPeer",
            Self::ResourceSequencingFailed => "ResourceSequencingFailed",
            Self::ResourcePredecessorFailed => "ResourcePredecessorFailed",
            Self::ChannelWindowFull => "ChannelWindowFull",
            Self::ChannelUntrackable => "ChannelUntrackable",
            Self::InvalidChannelMessageType => "InvalidChannelMessageType",
            Self::InvalidConfiguration => "InvalidConfiguration",
            Self::ResourceUploadCancelled => "ResourceUploadCancelled",
            Self::ResourceEarlyEof => "ResourceEarlyEof",
            Self::ResourceLengthOverrun => "ResourceLengthOverrun",
            Self::PermissionDenied => "PermissionDenied",
            Self::DeviceUnavailable => "DeviceUnavailable",
            Self::ConnectFailed => "ConnectFailed",
            Self::BackendFailed => "BackendFailed",
            Self::ResponseTooLarge => "ResponseTooLarge",
        }
    }
}

impl TryFrom<u32> for CommandFailureKind {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::NodeStopped),
            2 => Ok(Self::Busy),
            3 => Ok(Self::PayloadTooLarge),
            4 => Ok(Self::UnknownDestination),
            5 => Ok(Self::NotSingleDestination),
            6 => Ok(Self::AnnounceAppDataTooLong),
            7 => Ok(Self::UnknownInterface),
            8 => Ok(Self::NoRouteToDestination),
            9 => Ok(Self::NotDirectlyReachable),
            10 => Ok(Self::PacketCulled),
            11 => Ok(Self::DeliveryTimedOut),
            12 => Ok(Self::InvalidBitrate),
            13 => Ok(Self::BindFailed),
            14 => Ok(Self::WriteFailed),
            15 => Ok(Self::UnsupportedByBackend),
            16 => Ok(Self::UnknownLink),
            17 => Ok(Self::LinkNotActive),
            18 => Ok(Self::EntropyUnavailable),
            19 => Ok(Self::NotLinkInitiator),
            20 => Ok(Self::IdentityNotHeld),
            21 => Ok(Self::UnknownRequestHandler),
            22 => Ok(Self::RequestPolicyNotAllowList),
            23 => Ok(Self::RequestAllowListFull),
            24 => Ok(Self::LinkBusy),
            25 => Ok(Self::ResourceTableFull),
            26 => Ok(Self::ResourceMetadataTooLarge),
            27 => Ok(Self::ResourceRejectedByPeer),
            28 => Ok(Self::ResourceSequencingFailed),
            29 => Ok(Self::ResourcePredecessorFailed),
            30 => Ok(Self::ChannelWindowFull),
            31 => Ok(Self::ChannelUntrackable),
            32 => Ok(Self::InvalidChannelMessageType),
            33 => Ok(Self::InvalidConfiguration),
            34 => Ok(Self::ResourceUploadCancelled),
            35 => Ok(Self::ResourceEarlyEof),
            36 => Ok(Self::ResourceLengthOverrun),
            37 => Ok(Self::PermissionDenied),
            38 => Ok(Self::DeviceUnavailable),
            39 => Ok(Self::ConnectFailed),
            40 => Ok(Self::BackendFailed),
            41 => Ok(Self::ResponseTooLarge),
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DeliveryEvidenceKind {
    ExplicitProof = 1,
    ImplicitProof = 2,
    Response = 3,
}

impl DeliveryEvidenceKind {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::ExplicitProof => "ExplicitProof",
            Self::ImplicitProof => "ImplicitProof",
            Self::Response => "Response",
        }
    }
}

impl TryFrom<u32> for DeliveryEvidenceKind {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::ExplicitProof),
            2 => Ok(Self::ImplicitProof),
            3 => Ok(Self::Response),
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LifecyclePhase {
    Starting = 1,
    Running = 2,
    Stopping = 3,
    Stopped = 4,
    Failed = 5,
}

impl LifecyclePhase {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::Starting => "Starting",
            Self::Running => "Running",
            Self::Stopping => "Stopping",
            Self::Stopped => "Stopped",
            Self::Failed => "Failed",
        }
    }
}

impl TryFrom<u32> for LifecyclePhase {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Starting),
            2 => Ok(Self::Running),
            3 => Ok(Self::Stopping),
            4 => Ok(Self::Stopped),
            5 => Ok(Self::Failed),
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StopReason {
    Requested = 1,
    BackendExited = 2,
}

impl StopReason {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::Requested => "Requested",
            Self::BackendExited => "BackendExited",
        }
    }
}

impl TryFrom<u32> for StopReason {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Requested),
            2 => Ok(Self::BackendExited),
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LinkClosedReason {
    Timeout = 1,
    PeerClosed = 2,
    MalformedRtt = 3,
}

impl LinkClosedReason {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::Timeout => "Timeout",
            Self::PeerClosed => "PeerClosed",
            Self::MalformedRtt => "MalformedRtt",
        }
    }
}

impl TryFrom<u32> for LinkClosedReason {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Timeout),
            2 => Ok(Self::PeerClosed),
            3 => Ok(Self::MalformedRtt),
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ApplicationEventKind {
    SingleDelivery = 100,
    Request = 101,
    Response = 102,
    ResponseSegment = 103,
    ResourceAvailable = 104,
    ResourceSegment = 105,
    ResourceNeedsDecompression = 106,
    ChannelMessage = 107,
    LinkDelivery = 108,
}

impl ApplicationEventKind {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::SingleDelivery => "SingleDelivery",
            Self::Request => "Request",
            Self::Response => "Response",
            Self::ResponseSegment => "ResponseSegment",
            Self::ResourceAvailable => "ResourceAvailable",
            Self::ResourceSegment => "ResourceSegment",
            Self::ResourceNeedsDecompression => "ResourceNeedsDecompression",
            Self::ChannelMessage => "ChannelMessage",
            Self::LinkDelivery => "LinkDelivery",
        }
    }
}

impl TryFrom<u32> for ApplicationEventKind {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            100 => Ok(Self::SingleDelivery),
            101 => Ok(Self::Request),
            102 => Ok(Self::Response),
            103 => Ok(Self::ResponseSegment),
            104 => Ok(Self::ResourceAvailable),
            105 => Ok(Self::ResourceSegment),
            106 => Ok(Self::ResourceNeedsDecompression),
            107 => Ok(Self::ChannelMessage),
            108 => Ok(Self::LinkDelivery),
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DiagnosticEventKind {
    AnnounceHeard = 200,
    LinkEstablished = 201,
    PeerIdentified = 202,
    LinkClosed = 203,
    LinkInterfaceMismatch = 204,
    ResourceAssembled = 205,
    ResourceFailed = 206,
    ResourceSendProgress = 207,
    SelfRatchetRotated = 208,
    AnnounceHeldDropped = 209,
    Delivered = 210,
    RouteExpired = 211,
    RouteEvicted = 212,
    RouteInterfaceGone = 213,
    RouteDropped = 214,
    BackendDiagnostic = 215,
    DiagnosticsDropped = 216,
    PersistenceRestored = 217,
    PersistenceFlushed = 218,
    PersistenceFlushFailed = 219,
}

impl DiagnosticEventKind {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::AnnounceHeard => "AnnounceHeard",
            Self::LinkEstablished => "LinkEstablished",
            Self::PeerIdentified => "PeerIdentified",
            Self::LinkClosed => "LinkClosed",
            Self::LinkInterfaceMismatch => "LinkInterfaceMismatch",
            Self::ResourceAssembled => "ResourceAssembled",
            Self::ResourceFailed => "ResourceFailed",
            Self::ResourceSendProgress => "ResourceSendProgress",
            Self::SelfRatchetRotated => "SelfRatchetRotated",
            Self::AnnounceHeldDropped => "AnnounceHeldDropped",
            Self::Delivered => "Delivered",
            Self::RouteExpired => "RouteExpired",
            Self::RouteEvicted => "RouteEvicted",
            Self::RouteInterfaceGone => "RouteInterfaceGone",
            Self::RouteDropped => "RouteDropped",
            Self::BackendDiagnostic => "BackendDiagnostic",
            Self::DiagnosticsDropped => "DiagnosticsDropped",
            Self::PersistenceRestored => "PersistenceRestored",
            Self::PersistenceFlushed => "PersistenceFlushed",
            Self::PersistenceFlushFailed => "PersistenceFlushFailed",
        }
    }
}

impl TryFrom<u32> for DiagnosticEventKind {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            200 => Ok(Self::AnnounceHeard),
            201 => Ok(Self::LinkEstablished),
            202 => Ok(Self::PeerIdentified),
            203 => Ok(Self::LinkClosed),
            204 => Ok(Self::LinkInterfaceMismatch),
            205 => Ok(Self::ResourceAssembled),
            206 => Ok(Self::ResourceFailed),
            207 => Ok(Self::ResourceSendProgress),
            208 => Ok(Self::SelfRatchetRotated),
            209 => Ok(Self::AnnounceHeldDropped),
            210 => Ok(Self::Delivered),
            211 => Ok(Self::RouteExpired),
            212 => Ok(Self::RouteEvicted),
            213 => Ok(Self::RouteInterfaceGone),
            214 => Ok(Self::RouteDropped),
            215 => Ok(Self::BackendDiagnostic),
            216 => Ok(Self::DiagnosticsDropped),
            217 => Ok(Self::PersistenceRestored),
            218 => Ok(Self::PersistenceFlushed),
            219 => Ok(Self::PersistenceFlushFailed),
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PersistenceFlushCause {
    Startup = 1,
    Interval = 2,
    RouteChange = 3,
    RatchetRotation = 4,
    Shutdown = 5,
}

impl PersistenceFlushCause {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::Startup => "Startup",
            Self::Interval => "Interval",
            Self::RouteChange => "RouteChange",
            Self::RatchetRotation => "RatchetRotation",
            Self::Shutdown => "Shutdown",
        }
    }
}

impl TryFrom<u32> for PersistenceFlushCause {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Startup),
            2 => Ok(Self::Interval),
            3 => Ok(Self::RouteChange),
            4 => Ok(Self::RatchetRotation),
            5 => Ok(Self::Shutdown),
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PersistenceFlushTarget {
    RoutingState = 1,
    Ratchets = 2,
}

impl PersistenceFlushTarget {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::RoutingState => "RoutingState",
            Self::Ratchets => "Ratchets",
        }
    }
}

impl TryFrom<u32> for PersistenceFlushTarget {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::RoutingState),
            2 => Ok(Self::Ratchets),
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EventField {
    Destination = 1,
    SourceInterface = 2,
    Plaintext = 3,
    LinkId = 4,
    RequestId = 5,
    Requester = 6,
    PathHash = 7,
    RttMillis = 8,
    Data = 9,
    SegmentIndex = 10,
    TotalSegments = 11,
    Hash = 12,
    OriginalHash = 13,
    Metadata = 14,
    TotalBytes = 15,
    StreamId = 16,
    UncompressedDataBytes = 17,
    MessageType = 18,
    Identity = 19,
    Reason = 20,
    AttachedInterface = 21,
    ArrivedOn = 22,
    TotalSizeBytes = 23,
    Cause = 24,
    TransferredBytes = 25,
    PhysicalTransferredBytes = 26,
    Detail = 27,
    Kind = 28,
    DroppedCount = 29,
    Hops = 30,
    Stream = 31,
    Routes = 32,
    DestinationIdentities = 33,
    Tunnels = 34,
    Ratchets = 35,
    Refused = 36,
    Dropped = 37,
    PersistenceCause = 38,
    PersistenceTarget = 39,
    AppData = 40,
}

impl EventField {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::Destination => "Destination",
            Self::SourceInterface => "SourceInterface",
            Self::Plaintext => "Plaintext",
            Self::LinkId => "LinkId",
            Self::RequestId => "RequestId",
            Self::Requester => "Requester",
            Self::PathHash => "PathHash",
            Self::RttMillis => "RttMillis",
            Self::Data => "Data",
            Self::SegmentIndex => "SegmentIndex",
            Self::TotalSegments => "TotalSegments",
            Self::Hash => "Hash",
            Self::OriginalHash => "OriginalHash",
            Self::Metadata => "Metadata",
            Self::TotalBytes => "TotalBytes",
            Self::StreamId => "StreamId",
            Self::UncompressedDataBytes => "UncompressedDataBytes",
            Self::MessageType => "MessageType",
            Self::Identity => "Identity",
            Self::Reason => "Reason",
            Self::AttachedInterface => "AttachedInterface",
            Self::ArrivedOn => "ArrivedOn",
            Self::TotalSizeBytes => "TotalSizeBytes",
            Self::Cause => "Cause",
            Self::TransferredBytes => "TransferredBytes",
            Self::PhysicalTransferredBytes => "PhysicalTransferredBytes",
            Self::Detail => "Detail",
            Self::Kind => "Kind",
            Self::DroppedCount => "DroppedCount",
            Self::Hops => "Hops",
            Self::Stream => "Stream",
            Self::Routes => "Routes",
            Self::DestinationIdentities => "DestinationIdentities",
            Self::Tunnels => "Tunnels",
            Self::Ratchets => "Ratchets",
            Self::Refused => "Refused",
            Self::Dropped => "Dropped",
            Self::PersistenceCause => "PersistenceCause",
            Self::PersistenceTarget => "PersistenceTarget",
            Self::AppData => "AppData",
        }
    }
}

impl TryFrom<u32> for EventField {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Destination),
            2 => Ok(Self::SourceInterface),
            3 => Ok(Self::Plaintext),
            4 => Ok(Self::LinkId),
            5 => Ok(Self::RequestId),
            6 => Ok(Self::Requester),
            7 => Ok(Self::PathHash),
            8 => Ok(Self::RttMillis),
            9 => Ok(Self::Data),
            10 => Ok(Self::SegmentIndex),
            11 => Ok(Self::TotalSegments),
            12 => Ok(Self::Hash),
            13 => Ok(Self::OriginalHash),
            14 => Ok(Self::Metadata),
            15 => Ok(Self::TotalBytes),
            16 => Ok(Self::StreamId),
            17 => Ok(Self::UncompressedDataBytes),
            18 => Ok(Self::MessageType),
            19 => Ok(Self::Identity),
            20 => Ok(Self::Reason),
            21 => Ok(Self::AttachedInterface),
            22 => Ok(Self::ArrivedOn),
            23 => Ok(Self::TotalSizeBytes),
            24 => Ok(Self::Cause),
            25 => Ok(Self::TransferredBytes),
            26 => Ok(Self::PhysicalTransferredBytes),
            27 => Ok(Self::Detail),
            28 => Ok(Self::Kind),
            29 => Ok(Self::DroppedCount),
            30 => Ok(Self::Hops),
            31 => Ok(Self::Stream),
            32 => Ok(Self::Routes),
            33 => Ok(Self::DestinationIdentities),
            34 => Ok(Self::Tunnels),
            35 => Ok(Self::Ratchets),
            36 => Ok(Self::Refused),
            37 => Ok(Self::Dropped),
            38 => Ok(Self::PersistenceCause),
            39 => Ok(Self::PersistenceTarget),
            40 => Ok(Self::AppData),
            _ => Err(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! assert_contract_enum {
        ($enum:ty, [$(($variant:path, $value:literal, $name:literal)),+ $(,)?]) => {{
            $(
                assert_eq!($variant as u32, $value);
                assert_eq!($variant.contract_name(), $name);
                assert_eq!(<$enum>::try_from($value), Ok($variant));
            )+
            assert_eq!(<$enum>::try_from(u32::MAX), Err(()));
        }};
    }

    #[rustfmt::skip]
    #[test]
    fn status_values_match_the_contract() {
        assert_contract_enum!(Status, [
            (Status::Ok, 0, "Ok"),
            (Status::InvalidArgument, 1, "InvalidArgument"),
            (Status::ContractMismatch, 2, "ContractMismatch"),
            (Status::InvalidHandle, 3, "InvalidHandle"),
            (Status::NotReady, 4, "NotReady"),
            (Status::AlreadyClaimed, 5, "AlreadyClaimed"),
            (Status::WouldBlock, 6, "WouldBlock"),
            (Status::TimedOut, 7, "TimedOut"),
            (Status::QueueFull, 8, "QueueFull"),
            (Status::Stopped, 9, "Stopped"),
            (Status::BackendFailed, 10, "BackendFailed"),
            (Status::Panic, 11, "Panic"),
            (Status::Interrupted, 12, "Interrupted"),
            (Status::Unsupported, 13, "Unsupported"),
            (Status::PermissionDenied, 14, "PermissionDenied"),
            (Status::Unavailable, 15, "Unavailable"),
        ]);
    }

    #[rustfmt::skip]
    #[test]
    fn backend_kind_values_match_the_contract() {
        assert_contract_enum!(BackendKind, [
            (BackendKind::Native, 1, "Native"),
            (BackendKind::Browser, 2, "Browser"),
            (BackendKind::Cooperative, 3, "Cooperative"),
        ]);
    }

    #[rustfmt::skip]
    #[test]
    fn capability_values_match_the_contract() {
        assert_contract_enum!(Capability, [
            (Capability::Loopback, 1, "Loopback"),
            (Capability::TcpClient, 2, "TcpClient"),
            (Capability::TcpServer, 3, "TcpServer"),
            (Capability::Udp, 4, "Udp"),
            (Capability::Serial, 5, "Serial"),
            (Capability::Usb, 6, "Usb"),
            (Capability::Bluetooth, 7, "Bluetooth"),
            (Capability::Wifi, 8, "Wifi"),
            (Capability::WebSocket, 9, "WebSocket"),
            (Capability::BrowserRendezvous, 10, "BrowserRendezvous"),
            (Capability::I2p, 11, "I2p"),
            (Capability::Weave, 12, "Weave"),
            (Capability::SuppliedPipe, 13, "SuppliedPipe"),
        ]);
    }

    #[rustfmt::skip]
    #[test]
    fn interface_kind_values_match_the_contract() {
        assert_contract_enum!(InterfaceKind, [
            (InterfaceKind::AutoLan, 1, "AutoLan"),
            (InterfaceKind::TcpClient, 2, "TcpClient"),
            (InterfaceKind::TcpServer, 3, "TcpServer"),
            (InterfaceKind::Udp, 4, "Udp"),
            (InterfaceKind::Serial, 5, "Serial"),
            (InterfaceKind::Kiss, 6, "Kiss"),
            (InterfaceKind::Ax25Kiss, 7, "Ax25Kiss"),
            (InterfaceKind::RNode, 8, "RNode"),
            (InterfaceKind::MultiRNode, 9, "MultiRNode"),
            (InterfaceKind::Pipe, 10, "Pipe"),
            (InterfaceKind::BackboneClient, 11, "BackboneClient"),
            (InterfaceKind::BackboneServer, 12, "BackboneServer"),
            (InterfaceKind::I2p, 13, "I2p"),
            (InterfaceKind::Weave, 14, "Weave"),
            (InterfaceKind::AutomaticUsb, 15, "AutomaticUsb"),
            (InterfaceKind::AutomaticBluetoothLe, 16, "AutomaticBluetoothLe"),
            (InterfaceKind::WebSocketClient, 17, "WebSocketClient"),
            (InterfaceKind::WebSocketServer, 18, "WebSocketServer"),
            (InterfaceKind::BrowserRendezvous, 19, "BrowserRendezvous"),
        ]);
    }

    #[rustfmt::skip]
    #[test]
    fn interface_mode_values_match_the_contract() {
        assert_contract_enum!(InterfaceMode, [
            (InterfaceMode::Full, 1, "Full"),
            (InterfaceMode::PointToPoint, 2, "PointToPoint"),
            (InterfaceMode::AccessPoint, 3, "AccessPoint"),
            (InterfaceMode::Roaming, 4, "Roaming"),
            (InterfaceMode::Boundary, 5, "Boundary"),
            (InterfaceMode::Gateway, 6, "Gateway"),
            (InterfaceMode::Internal, 7, "Internal"),
        ]);
    }

    #[rustfmt::skip]
    #[test]
    fn web_socket_framing_selection_values_match_the_contract() {
        assert_contract_enum!(WebSocketFramingSelection, [
            (WebSocketFramingSelection::RawPacket, 1, "RawPacket"),
            (WebSocketFramingSelection::Hdlc, 2, "Hdlc"),
            (WebSocketFramingSelection::Kiss, 3, "Kiss"),
            (WebSocketFramingSelection::Auto, 4, "Auto"),
        ]);
    }

    #[rustfmt::skip]
    #[test]
    fn interface_health_values_match_the_contract() {
        assert_contract_enum!(InterfaceHealth, [
            (InterfaceHealth::Initializing, 1, "Initializing"),
            (InterfaceHealth::Connected, 2, "Connected"),
            (InterfaceHealth::Degraded, 3, "Degraded"),
            (InterfaceHealth::Reconnecting, 4, "Reconnecting"),
            (InterfaceHealth::Failed, 5, "Failed"),
            (InterfaceHealth::Disconnected, 6, "Disconnected"),
            (InterfaceHealth::Disabled, 7, "Disabled"),
            (InterfaceHealth::Unknown, 8, "Unknown"),
        ]);
    }

    #[rustfmt::skip]
    #[test]
    fn discovery_scope_values_match_the_contract() {
        assert_contract_enum!(DiscoveryScope, [
            (DiscoveryScope::Link, 1, "Link"),
            (DiscoveryScope::Admin, 2, "Admin"),
            (DiscoveryScope::Site, 3, "Site"),
            (DiscoveryScope::Organization, 4, "Organization"),
            (DiscoveryScope::Global, 5, "Global"),
        ]);
    }

    #[rustfmt::skip]
    #[test]
    fn multicast_address_type_values_match_the_contract() {
        assert_contract_enum!(MulticastAddressType, [
            (MulticastAddressType::Temporary, 1, "Temporary"),
            (MulticastAddressType::Permanent, 2, "Permanent"),
        ]);
    }

    #[rustfmt::skip]
    #[test]
    fn serial_data_bits_values_match_the_contract() {
        assert_contract_enum!(SerialDataBits, [
            (SerialDataBits::Five, 5, "Five"),
            (SerialDataBits::Six, 6, "Six"),
            (SerialDataBits::Seven, 7, "Seven"),
            (SerialDataBits::Eight, 8, "Eight"),
        ]);
    }

    #[rustfmt::skip]
    #[test]
    fn serial_parity_values_match_the_contract() {
        assert_contract_enum!(SerialParity, [
            (SerialParity::None, 1, "None"),
            (SerialParity::Even, 2, "Even"),
            (SerialParity::Odd, 3, "Odd"),
        ]);
    }

    #[rustfmt::skip]
    #[test]
    fn serial_stop_bits_values_match_the_contract() {
        assert_contract_enum!(SerialStopBits, [
            (SerialStopBits::One, 1, "One"),
            (SerialStopBits::Two, 2, "Two"),
        ]);
    }

    #[rustfmt::skip]
    #[test]
    fn host_role_values_match_the_contract() {
        assert_contract_enum!(HostRole, [
            (HostRole::Endpoint, 1, "Endpoint"),
            (HostRole::Transport, 2, "Transport"),
        ]);
    }

    #[rustfmt::skip]
    #[test]
    fn identity_config_kind_values_match_the_contract() {
        assert_contract_enum!(IdentityConfigKind, [
            (IdentityConfigKind::Existing, 1, "Existing"),
            (IdentityConfigKind::GenerateEphemeral, 2, "GenerateEphemeral"),
            (IdentityConfigKind::LoadOrCreate, 3, "LoadOrCreate"),
        ]);
    }

    #[rustfmt::skip]
    #[test]
    fn persistence_config_kind_values_match_the_contract() {
        assert_contract_enum!(PersistenceConfigKind, [
            (PersistenceConfigKind::Ephemeral, 1, "Ephemeral"),
            (PersistenceConfigKind::Directory, 2, "Directory"),
        ]);
    }

    #[rustfmt::skip]
    #[test]
    fn destination_config_kind_values_match_the_contract() {
        assert_contract_enum!(DestinationConfigKind, [
            (DestinationConfigKind::Plain, 1, "Plain"),
            (DestinationConfigKind::Single, 2, "Single"),
        ]);
    }

    #[rustfmt::skip]
    #[test]
    fn destination_identity_config_kind_values_match_the_contract() {
        assert_contract_enum!(DestinationIdentityConfigKind, [
            (DestinationIdentityConfigKind::HostIdentity, 1, "HostIdentity"),
            (DestinationIdentityConfigKind::DedicatedIdentity, 2, "DedicatedIdentity"),
        ]);
    }

    #[rustfmt::skip]
    #[test]
    fn bitrate_kind_values_match_the_contract() {
        assert_contract_enum!(BitrateKind, [
            (BitrateKind::Auto, 1, "Auto"),
            (BitrateKind::BitsPerSecond, 2, "BitsPerSecond"),
        ]);
    }

    #[rustfmt::skip]
    #[test]
    fn response_timeout_kind_values_match_the_contract() {
        assert_contract_enum!(ResponseTimeoutKind, [
            (ResponseTimeoutKind::LinkDefault, 1, "LinkDefault"),
            (ResponseTimeoutKind::Exact, 2, "Exact"),
        ]);
    }

    #[rustfmt::skip]
    #[test]
    fn resource_compression_kind_values_match_the_contract() {
        assert_contract_enum!(ResourceCompressionKind, [
            (ResourceCompressionKind::Auto, 1, "Auto"),
            (ResourceCompressionKind::Never, 2, "Never"),
        ]);
    }

    #[rustfmt::skip]
    #[test]
    fn resource_strategy_kind_values_match_the_contract() {
        assert_contract_enum!(ResourceStrategyKind, [
            (ResourceStrategyKind::Refuse, 1, "Refuse"),
            (ResourceStrategyKind::Accept, 2, "Accept"),
        ]);
    }

    #[rustfmt::skip]
    #[test]
    fn request_policy_values_match_the_contract() {
        assert_contract_enum!(RequestPolicy, [
            (RequestPolicy::AllowNone, 1, "AllowNone"),
            (RequestPolicy::AllowAll, 2, "AllowAll"),
            (RequestPolicy::AllowList, 3, "AllowList"),
        ]);
    }

    #[rustfmt::skip]
    #[test]
    fn command_outcome_kind_values_match_the_contract() {
        assert_contract_enum!(CommandOutcomeKind, [
            (CommandOutcomeKind::Announced, 1, "Announced"),
            (CommandOutcomeKind::PacketDelivered, 2, "PacketDelivered"),
            (CommandOutcomeKind::LinkCloseQueued, 3, "LinkCloseQueued"),
            (CommandOutcomeKind::InterfaceAttached, 4, "InterfaceAttached"),
            (CommandOutcomeKind::InterfaceDetached, 5, "InterfaceDetached"),
            (CommandOutcomeKind::LinkEstablished, 6, "LinkEstablished"),
            (CommandOutcomeKind::PathDiscovered, 7, "PathDiscovered"),
            (CommandOutcomeKind::Identified, 8, "Identified"),
            (CommandOutcomeKind::ResponseReceived, 9, "ResponseReceived"),
            (CommandOutcomeKind::ResponseSent, 10, "ResponseSent"),
            (CommandOutcomeKind::ResourceSent, 11, "ResourceSent"),
            (CommandOutcomeKind::ResourceStrategySet, 12, "ResourceStrategySet"),
            (CommandOutcomeKind::RequesterAllowed, 13, "RequesterAllowed"),
        ]);
    }

    #[rustfmt::skip]
    #[test]
    fn command_failure_kind_values_match_the_contract() {
        assert_contract_enum!(CommandFailureKind, [
            (CommandFailureKind::NodeStopped, 1, "NodeStopped"),
            (CommandFailureKind::Busy, 2, "Busy"),
            (CommandFailureKind::PayloadTooLarge, 3, "PayloadTooLarge"),
            (CommandFailureKind::UnknownDestination, 4, "UnknownDestination"),
            (CommandFailureKind::NotSingleDestination, 5, "NotSingleDestination"),
            (CommandFailureKind::AnnounceAppDataTooLong, 6, "AnnounceAppDataTooLong"),
            (CommandFailureKind::UnknownInterface, 7, "UnknownInterface"),
            (CommandFailureKind::NoRouteToDestination, 8, "NoRouteToDestination"),
            (CommandFailureKind::NotDirectlyReachable, 9, "NotDirectlyReachable"),
            (CommandFailureKind::PacketCulled, 10, "PacketCulled"),
            (CommandFailureKind::DeliveryTimedOut, 11, "DeliveryTimedOut"),
            (CommandFailureKind::InvalidBitrate, 12, "InvalidBitrate"),
            (CommandFailureKind::BindFailed, 13, "BindFailed"),
            (CommandFailureKind::WriteFailed, 14, "WriteFailed"),
            (CommandFailureKind::UnsupportedByBackend, 15, "UnsupportedByBackend"),
            (CommandFailureKind::UnknownLink, 16, "UnknownLink"),
            (CommandFailureKind::LinkNotActive, 17, "LinkNotActive"),
            (CommandFailureKind::EntropyUnavailable, 18, "EntropyUnavailable"),
            (CommandFailureKind::NotLinkInitiator, 19, "NotLinkInitiator"),
            (CommandFailureKind::IdentityNotHeld, 20, "IdentityNotHeld"),
            (CommandFailureKind::UnknownRequestHandler, 21, "UnknownRequestHandler"),
            (CommandFailureKind::RequestPolicyNotAllowList, 22, "RequestPolicyNotAllowList"),
            (CommandFailureKind::RequestAllowListFull, 23, "RequestAllowListFull"),
            (CommandFailureKind::LinkBusy, 24, "LinkBusy"),
            (CommandFailureKind::ResourceTableFull, 25, "ResourceTableFull"),
            (CommandFailureKind::ResourceMetadataTooLarge, 26, "ResourceMetadataTooLarge"),
            (CommandFailureKind::ResourceRejectedByPeer, 27, "ResourceRejectedByPeer"),
            (CommandFailureKind::ResourceSequencingFailed, 28, "ResourceSequencingFailed"),
            (CommandFailureKind::ResourcePredecessorFailed, 29, "ResourcePredecessorFailed"),
            (CommandFailureKind::ChannelWindowFull, 30, "ChannelWindowFull"),
            (CommandFailureKind::ChannelUntrackable, 31, "ChannelUntrackable"),
            (CommandFailureKind::InvalidChannelMessageType, 32, "InvalidChannelMessageType"),
            (CommandFailureKind::InvalidConfiguration, 33, "InvalidConfiguration"),
            (CommandFailureKind::ResourceUploadCancelled, 34, "ResourceUploadCancelled"),
            (CommandFailureKind::ResourceEarlyEof, 35, "ResourceEarlyEof"),
            (CommandFailureKind::ResourceLengthOverrun, 36, "ResourceLengthOverrun"),
            (CommandFailureKind::PermissionDenied, 37, "PermissionDenied"),
            (CommandFailureKind::DeviceUnavailable, 38, "DeviceUnavailable"),
            (CommandFailureKind::ConnectFailed, 39, "ConnectFailed"),
            (CommandFailureKind::BackendFailed, 40, "BackendFailed"),
            (CommandFailureKind::ResponseTooLarge, 41, "ResponseTooLarge"),
        ]);
    }

    #[rustfmt::skip]
    #[test]
    fn delivery_evidence_kind_values_match_the_contract() {
        assert_contract_enum!(DeliveryEvidenceKind, [
            (DeliveryEvidenceKind::ExplicitProof, 1, "ExplicitProof"),
            (DeliveryEvidenceKind::ImplicitProof, 2, "ImplicitProof"),
            (DeliveryEvidenceKind::Response, 3, "Response"),
        ]);
    }

    #[rustfmt::skip]
    #[test]
    fn lifecycle_phase_values_match_the_contract() {
        assert_contract_enum!(LifecyclePhase, [
            (LifecyclePhase::Starting, 1, "Starting"),
            (LifecyclePhase::Running, 2, "Running"),
            (LifecyclePhase::Stopping, 3, "Stopping"),
            (LifecyclePhase::Stopped, 4, "Stopped"),
            (LifecyclePhase::Failed, 5, "Failed"),
        ]);
    }

    #[rustfmt::skip]
    #[test]
    fn stop_reason_values_match_the_contract() {
        assert_contract_enum!(StopReason, [
            (StopReason::Requested, 1, "Requested"),
            (StopReason::BackendExited, 2, "BackendExited"),
        ]);
    }

    #[rustfmt::skip]
    #[test]
    fn link_closed_reason_values_match_the_contract() {
        assert_contract_enum!(LinkClosedReason, [
            (LinkClosedReason::Timeout, 1, "Timeout"),
            (LinkClosedReason::PeerClosed, 2, "PeerClosed"),
            (LinkClosedReason::MalformedRtt, 3, "MalformedRtt"),
        ]);
    }

    #[rustfmt::skip]
    #[test]
    fn application_event_kind_values_match_the_contract() {
        assert_contract_enum!(ApplicationEventKind, [
            (ApplicationEventKind::SingleDelivery, 100, "SingleDelivery"),
            (ApplicationEventKind::Request, 101, "Request"),
            (ApplicationEventKind::Response, 102, "Response"),
            (ApplicationEventKind::ResponseSegment, 103, "ResponseSegment"),
            (ApplicationEventKind::ResourceAvailable, 104, "ResourceAvailable"),
            (ApplicationEventKind::ResourceSegment, 105, "ResourceSegment"),
            (ApplicationEventKind::ResourceNeedsDecompression, 106, "ResourceNeedsDecompression"),
            (ApplicationEventKind::ChannelMessage, 107, "ChannelMessage"),
            (ApplicationEventKind::LinkDelivery, 108, "LinkDelivery"),
        ]);
    }

    #[rustfmt::skip]
    #[test]
    fn diagnostic_event_kind_values_match_the_contract() {
        assert_contract_enum!(DiagnosticEventKind, [
            (DiagnosticEventKind::AnnounceHeard, 200, "AnnounceHeard"),
            (DiagnosticEventKind::LinkEstablished, 201, "LinkEstablished"),
            (DiagnosticEventKind::PeerIdentified, 202, "PeerIdentified"),
            (DiagnosticEventKind::LinkClosed, 203, "LinkClosed"),
            (DiagnosticEventKind::LinkInterfaceMismatch, 204, "LinkInterfaceMismatch"),
            (DiagnosticEventKind::ResourceAssembled, 205, "ResourceAssembled"),
            (DiagnosticEventKind::ResourceFailed, 206, "ResourceFailed"),
            (DiagnosticEventKind::ResourceSendProgress, 207, "ResourceSendProgress"),
            (DiagnosticEventKind::SelfRatchetRotated, 208, "SelfRatchetRotated"),
            (DiagnosticEventKind::AnnounceHeldDropped, 209, "AnnounceHeldDropped"),
            (DiagnosticEventKind::Delivered, 210, "Delivered"),
            (DiagnosticEventKind::RouteExpired, 211, "RouteExpired"),
            (DiagnosticEventKind::RouteEvicted, 212, "RouteEvicted"),
            (DiagnosticEventKind::RouteInterfaceGone, 213, "RouteInterfaceGone"),
            (DiagnosticEventKind::RouteDropped, 214, "RouteDropped"),
            (DiagnosticEventKind::BackendDiagnostic, 215, "BackendDiagnostic"),
            (DiagnosticEventKind::DiagnosticsDropped, 216, "DiagnosticsDropped"),
            (DiagnosticEventKind::PersistenceRestored, 217, "PersistenceRestored"),
            (DiagnosticEventKind::PersistenceFlushed, 218, "PersistenceFlushed"),
            (DiagnosticEventKind::PersistenceFlushFailed, 219, "PersistenceFlushFailed"),
        ]);
    }

    #[rustfmt::skip]
    #[test]
    fn persistence_flush_cause_values_match_the_contract() {
        assert_contract_enum!(PersistenceFlushCause, [
            (PersistenceFlushCause::Startup, 1, "Startup"),
            (PersistenceFlushCause::Interval, 2, "Interval"),
            (PersistenceFlushCause::RouteChange, 3, "RouteChange"),
            (PersistenceFlushCause::RatchetRotation, 4, "RatchetRotation"),
            (PersistenceFlushCause::Shutdown, 5, "Shutdown"),
        ]);
    }

    #[rustfmt::skip]
    #[test]
    fn persistence_flush_target_values_match_the_contract() {
        assert_contract_enum!(PersistenceFlushTarget, [
            (PersistenceFlushTarget::RoutingState, 1, "RoutingState"),
            (PersistenceFlushTarget::Ratchets, 2, "Ratchets"),
        ]);
    }

    #[rustfmt::skip]
    #[test]
    fn event_field_values_match_the_contract() {
        assert_contract_enum!(EventField, [
            (EventField::Destination, 1, "Destination"),
            (EventField::SourceInterface, 2, "SourceInterface"),
            (EventField::Plaintext, 3, "Plaintext"),
            (EventField::LinkId, 4, "LinkId"),
            (EventField::RequestId, 5, "RequestId"),
            (EventField::Requester, 6, "Requester"),
            (EventField::PathHash, 7, "PathHash"),
            (EventField::RttMillis, 8, "RttMillis"),
            (EventField::Data, 9, "Data"),
            (EventField::SegmentIndex, 10, "SegmentIndex"),
            (EventField::TotalSegments, 11, "TotalSegments"),
            (EventField::Hash, 12, "Hash"),
            (EventField::OriginalHash, 13, "OriginalHash"),
            (EventField::Metadata, 14, "Metadata"),
            (EventField::TotalBytes, 15, "TotalBytes"),
            (EventField::StreamId, 16, "StreamId"),
            (EventField::UncompressedDataBytes, 17, "UncompressedDataBytes"),
            (EventField::MessageType, 18, "MessageType"),
            (EventField::Identity, 19, "Identity"),
            (EventField::Reason, 20, "Reason"),
            (EventField::AttachedInterface, 21, "AttachedInterface"),
            (EventField::ArrivedOn, 22, "ArrivedOn"),
            (EventField::TotalSizeBytes, 23, "TotalSizeBytes"),
            (EventField::Cause, 24, "Cause"),
            (EventField::TransferredBytes, 25, "TransferredBytes"),
            (EventField::PhysicalTransferredBytes, 26, "PhysicalTransferredBytes"),
            (EventField::Detail, 27, "Detail"),
            (EventField::Kind, 28, "Kind"),
            (EventField::DroppedCount, 29, "DroppedCount"),
            (EventField::Hops, 30, "Hops"),
            (EventField::Stream, 31, "Stream"),
            (EventField::Routes, 32, "Routes"),
            (EventField::DestinationIdentities, 33, "DestinationIdentities"),
            (EventField::Tunnels, 34, "Tunnels"),
            (EventField::Ratchets, 35, "Ratchets"),
            (EventField::Refused, 36, "Refused"),
            (EventField::Dropped, 37, "Dropped"),
            (EventField::PersistenceCause, 38, "PersistenceCause"),
            (EventField::PersistenceTarget, 39, "PersistenceTarget"),
            (EventField::AppData, 40, "AppData"),
        ]);
    }
}
