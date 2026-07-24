pub const HOST_SCHEMA_VERSION: u32 = 1;
pub const HOST_SCHEMA_ABI: u32 = 1;
pub const HOST_SCHEMA_PRODUCT_VERSION: &str = "0.2.8";
pub const DESTINATION_HASH_LENGTH: usize = 16;
pub const IDENTITY_HASH_LENGTH: usize = 16;
pub const INTERFACE_ID_LENGTH: usize = 8;
pub const LINK_ID_LENGTH: usize = 16;
pub const PACKET_HASH_LENGTH: usize = 32;
pub const REQUEST_ID_LENGTH: usize = 16;
pub const REQUEST_PATH_HASH_LENGTH: usize = 16;
pub const RESOURCE_HASH_LENGTH: usize = 32;
pub const IDENTITY_SECRET_LENGTH: usize = 64;
pub const BALANCED_PENDING_COMMANDS: usize = 256;
pub const BALANCED_APPLICATION_EVENTS: usize = 1024;
pub const BALANCED_RETAINED_EVENT_BYTES: usize = 8388608;
pub const BALANCED_DIAGNOSTICS: usize = 1024;

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AbiStatus {
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
}

impl TryFrom<u32> for AbiStatus {
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
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AbiBackendKind {
    Native = 1,
    Browser = 2,
    Cooperative = 3,
}

impl TryFrom<u32> for AbiBackendKind {
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
pub enum AbiCapability {
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
}

impl TryFrom<u32> for AbiCapability {
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
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AbiHostRole {
    Endpoint = 1,
    Transport = 2,
}

impl TryFrom<u32> for AbiHostRole {
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
pub enum AbiIdentityConfigKind {
    Existing = 1,
    GenerateEphemeral = 2,
    LoadOrCreate = 3,
}

impl TryFrom<u32> for AbiIdentityConfigKind {
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
pub enum AbiDestinationConfigKind {
    Plain = 1,
    Single = 2,
}

impl TryFrom<u32> for AbiDestinationConfigKind {
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
pub enum AbiDestinationIdentityConfigKind {
    HostIdentity = 1,
    DedicatedIdentity = 2,
}

impl TryFrom<u32> for AbiDestinationIdentityConfigKind {
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
pub enum AbiBitrateKind {
    Auto = 1,
    BitsPerSecond = 2,
}

impl TryFrom<u32> for AbiBitrateKind {
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
pub enum AbiCommandOutcomeKind {
    Announced = 1,
    PacketDelivered = 2,
    LinkCloseQueued = 3,
    InterfaceAttached = 4,
    InterfaceDetached = 5,
}

impl TryFrom<u32> for AbiCommandOutcomeKind {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Announced),
            2 => Ok(Self::PacketDelivered),
            3 => Ok(Self::LinkCloseQueued),
            4 => Ok(Self::InterfaceAttached),
            5 => Ok(Self::InterfaceDetached),
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AbiCommandFailureKind {
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
}

impl TryFrom<u32> for AbiCommandFailureKind {
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
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AbiDeliveryEvidenceKind {
    ExplicitProof = 1,
    ImplicitProof = 2,
    Response = 3,
}

impl TryFrom<u32> for AbiDeliveryEvidenceKind {
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
pub enum AbiLifecyclePhase {
    Starting = 1,
    Running = 2,
    Stopping = 3,
    Stopped = 4,
    Failed = 5,
}

impl TryFrom<u32> for AbiLifecyclePhase {
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
pub enum AbiStopReason {
    Requested = 1,
    BackendExited = 2,
}

impl TryFrom<u32> for AbiStopReason {
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
pub enum AbiLinkClosedReason {
    Timeout = 1,
    PeerClosed = 2,
    MalformedRtt = 3,
}

impl TryFrom<u32> for AbiLinkClosedReason {
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
pub enum AbiApplicationEventKind {
    SingleDelivery = 100,
    Request = 101,
    Response = 102,
    ResponseSegment = 103,
    ResourceAvailable = 104,
    ResourceSegment = 105,
    ResourceNeedsDecompression = 106,
    ChannelMessage = 107,
}

impl TryFrom<u32> for AbiApplicationEventKind {
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
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AbiDiagnosticEventKind {
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
}

impl TryFrom<u32> for AbiDiagnosticEventKind {
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
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AbiEventField {
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
}

impl TryFrom<u32> for AbiEventField {
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
            _ => Err(()),
        }
    }
}
