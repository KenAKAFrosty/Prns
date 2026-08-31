use alloc::string::String;
use alloc::vec::Vec;

use crate::{
    CommandFailureKind, CommandOutcomeKind, DeliveryEvidenceKind, DestinationHash, IdentityHash,
    InterfaceConfig, InterfaceId, InterfaceRoutingPolicy, LinkId, PacketHash, RequestId,
    RequestPathHash,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bitrate {
    Auto,
    BitsPerSecond(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResponseTimeout {
    LinkDefault,
    Exact { millis: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceCompression {
    Auto,
    Never,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceStrategy {
    Refuse,
    Accept {
        maximum_uncompressed_bytes: u64,
        accept_compressed: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostCommand {
    Announce {
        destination: DestinationHash,
        interface: Option<InterfaceId>,
    },
    SendSinglePacket {
        destination: DestinationHash,
        payload: Vec<u8>,
    },
    CloseLink {
        link_id: LinkId,
    },
    AttachTcpServer {
        bind: String,
        bitrate: Bitrate,
    },
    AttachTcpClient {
        target: String,
        bitrate: Bitrate,
    },
    AttachUdp {
        local: String,
        peer: String,
        bitrate: Bitrate,
    },
    AttachInterface {
        config: InterfaceConfig,
        routing: Option<InterfaceRoutingPolicy>,
    },
    DetachInterface {
        interface: InterfaceId,
    },
    EstablishLink {
        destination: DestinationHash,
    },
    RequestPath {
        destination: DestinationHash,
    },
    Identify {
        link_id: LinkId,
        identity: IdentityHash,
    },
    SendLinkPacket {
        link_id: LinkId,
        payload: Vec<u8>,
    },
    Request {
        link_id: LinkId,
        path_hash: RequestPathHash,
        payload: Vec<u8>,
        timeout: ResponseTimeout,
        maximum_response_bytes: Option<u64>,
    },
    Respond {
        link_id: LinkId,
        request_id: RequestId,
        request_rtt_millis: u64,
        payload: Vec<u8>,
    },
    SendResource {
        link_id: LinkId,
        payload: Vec<u8>,
        packed_metadata: Option<Vec<u8>>,
        compression: ResourceCompression,
    },
    SetLinkResourceStrategy {
        link_id: LinkId,
        strategy: ResourceStrategy,
    },
    SetDestinationResourceStrategy {
        destination: DestinationHash,
        strategy: ResourceStrategy,
    },
    SendChannelMessage {
        link_id: LinkId,
        message_type: u16,
        payload: Vec<u8>,
    },
    AllowRequester {
        destination: DestinationHash,
        path_hash: RequestPathHash,
        identity: IdentityHash,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryEvidence {
    ExplicitProof(PacketHash),
    ImplicitProof(PacketHash),
    Response,
}

impl DeliveryEvidence {
    #[must_use]
    pub const fn kind(self) -> DeliveryEvidenceKind {
        match self {
            Self::ExplicitProof(_) => DeliveryEvidenceKind::ExplicitProof,
            Self::ImplicitProof(_) => DeliveryEvidenceKind::ImplicitProof,
            Self::Response => DeliveryEvidenceKind::Response,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandOutcome {
    Announced,
    PacketDelivered {
        rtt_millis: u64,
        evidence: DeliveryEvidence,
    },
    LinkCloseQueued,
    InterfaceAttached {
        interface: InterfaceId,
    },
    InterfaceDetached {
        interface: InterfaceId,
    },
    LinkEstablished {
        link_id: LinkId,
        rtt_millis: u64,
    },
    PathDiscovered {
        hops: u8,
    },
    Identified,
    ResponseReceived {
        data: Vec<u8>,
        rtt_millis: u64,
    },
    ResponseSent {
        rtt_millis: u64,
    },
    ResourceSent,
    ResourceStrategySet,
    RequesterAllowed,
}

impl CommandOutcome {
    #[must_use]
    pub const fn kind(&self) -> CommandOutcomeKind {
        match self {
            Self::Announced => CommandOutcomeKind::Announced,
            Self::PacketDelivered { .. } => CommandOutcomeKind::PacketDelivered,
            Self::LinkCloseQueued => CommandOutcomeKind::LinkCloseQueued,
            Self::InterfaceAttached { .. } => CommandOutcomeKind::InterfaceAttached,
            Self::InterfaceDetached { .. } => CommandOutcomeKind::InterfaceDetached,
            Self::LinkEstablished { .. } => CommandOutcomeKind::LinkEstablished,
            Self::PathDiscovered { .. } => CommandOutcomeKind::PathDiscovered,
            Self::Identified => CommandOutcomeKind::Identified,
            Self::ResponseReceived { .. } => CommandOutcomeKind::ResponseReceived,
            Self::ResponseSent { .. } => CommandOutcomeKind::ResponseSent,
            Self::ResourceSent => CommandOutcomeKind::ResourceSent,
            Self::ResourceStrategySet => CommandOutcomeKind::ResourceStrategySet,
            Self::RequesterAllowed => CommandOutcomeKind::RequesterAllowed,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandFailure {
    NodeStopped,
    Busy,
    PayloadTooLarge,
    UnknownDestination,
    NotSingleDestination,
    AnnounceAppDataTooLong,
    UnknownInterface,
    NoRouteToDestination,
    NotDirectlyReachable,
    PacketCulled,
    DeliveryTimedOut,
    InvalidBitrate,
    BindFailed { detail: String },
    WriteFailed { detail: String },
    UnsupportedByBackend,
    UnknownLink,
    LinkNotActive,
    EntropyUnavailable,
    NotLinkInitiator,
    IdentityNotHeld,
    UnknownRequestHandler,
    RequestPolicyNotAllowList,
    RequestAllowListFull,
    LinkBusy,
    ResourceTableFull,
    ResourceMetadataTooLarge,
    ResourceRejectedByPeer,
    ResourceSequencingFailed,
    ResourcePredecessorFailed,
    ChannelWindowFull,
    ChannelUntrackable,
    InvalidChannelMessageType,
    InvalidConfiguration { detail: String },
    ResourceUploadCancelled,
    ResourceEarlyEof,
    ResourceLengthOverrun,
    PermissionDenied { detail: String },
    DeviceUnavailable { detail: String },
    ConnectFailed { detail: String },
    BackendFailed { detail: String },
    ResponseTooLarge,
    LinkClosed,
    ResponseCancelledBySender,
    ResponseHashmapBeyondPartCount,
    ResponseHashmapSkipsAhead,
    ResponseHashmapTooLong,
    ResponseHashmapRagged,
    ResponseRetriesExhausted,
    ResponseLinkVanished,
    ResponseTransferUnopenable,
    ResponseTransferCorrupt,
    ResponseProofUnsendable,
    ResponseDecompressionFailed,
    ResponseDecompressionTimedOut,
    ResponseOpenTimedOut,
    ResponseMetadataOverrun,
}

impl CommandFailure {
    #[must_use]
    pub const fn kind(&self) -> CommandFailureKind {
        match self {
            Self::NodeStopped => CommandFailureKind::NodeStopped,
            Self::Busy => CommandFailureKind::Busy,
            Self::PayloadTooLarge => CommandFailureKind::PayloadTooLarge,
            Self::UnknownDestination => CommandFailureKind::UnknownDestination,
            Self::NotSingleDestination => CommandFailureKind::NotSingleDestination,
            Self::AnnounceAppDataTooLong => CommandFailureKind::AnnounceAppDataTooLong,
            Self::UnknownInterface => CommandFailureKind::UnknownInterface,
            Self::NoRouteToDestination => CommandFailureKind::NoRouteToDestination,
            Self::NotDirectlyReachable => CommandFailureKind::NotDirectlyReachable,
            Self::PacketCulled => CommandFailureKind::PacketCulled,
            Self::DeliveryTimedOut => CommandFailureKind::DeliveryTimedOut,
            Self::InvalidBitrate => CommandFailureKind::InvalidBitrate,
            Self::BindFailed { .. } => CommandFailureKind::BindFailed,
            Self::WriteFailed { .. } => CommandFailureKind::WriteFailed,
            Self::UnsupportedByBackend => CommandFailureKind::UnsupportedByBackend,
            Self::UnknownLink => CommandFailureKind::UnknownLink,
            Self::LinkNotActive => CommandFailureKind::LinkNotActive,
            Self::EntropyUnavailable => CommandFailureKind::EntropyUnavailable,
            Self::NotLinkInitiator => CommandFailureKind::NotLinkInitiator,
            Self::IdentityNotHeld => CommandFailureKind::IdentityNotHeld,
            Self::UnknownRequestHandler => CommandFailureKind::UnknownRequestHandler,
            Self::RequestPolicyNotAllowList => CommandFailureKind::RequestPolicyNotAllowList,
            Self::RequestAllowListFull => CommandFailureKind::RequestAllowListFull,
            Self::LinkBusy => CommandFailureKind::LinkBusy,
            Self::ResourceTableFull => CommandFailureKind::ResourceTableFull,
            Self::ResourceMetadataTooLarge => CommandFailureKind::ResourceMetadataTooLarge,
            Self::ResourceRejectedByPeer => CommandFailureKind::ResourceRejectedByPeer,
            Self::ResourceSequencingFailed => CommandFailureKind::ResourceSequencingFailed,
            Self::ResourcePredecessorFailed => CommandFailureKind::ResourcePredecessorFailed,
            Self::ChannelWindowFull => CommandFailureKind::ChannelWindowFull,
            Self::ChannelUntrackable => CommandFailureKind::ChannelUntrackable,
            Self::InvalidChannelMessageType => CommandFailureKind::InvalidChannelMessageType,
            Self::InvalidConfiguration { .. } => CommandFailureKind::InvalidConfiguration,
            Self::ResourceUploadCancelled => CommandFailureKind::ResourceUploadCancelled,
            Self::ResourceEarlyEof => CommandFailureKind::ResourceEarlyEof,
            Self::ResourceLengthOverrun => CommandFailureKind::ResourceLengthOverrun,
            Self::PermissionDenied { .. } => CommandFailureKind::PermissionDenied,
            Self::DeviceUnavailable { .. } => CommandFailureKind::DeviceUnavailable,
            Self::ConnectFailed { .. } => CommandFailureKind::ConnectFailed,
            Self::BackendFailed { .. } => CommandFailureKind::BackendFailed,
            Self::ResponseTooLarge => CommandFailureKind::ResponseTooLarge,
            Self::LinkClosed => CommandFailureKind::LinkClosed,
            Self::ResponseCancelledBySender => CommandFailureKind::ResponseCancelledBySender,
            Self::ResponseHashmapBeyondPartCount => {
                CommandFailureKind::ResponseHashmapBeyondPartCount
            }
            Self::ResponseHashmapSkipsAhead => CommandFailureKind::ResponseHashmapSkipsAhead,
            Self::ResponseHashmapTooLong => CommandFailureKind::ResponseHashmapTooLong,
            Self::ResponseHashmapRagged => CommandFailureKind::ResponseHashmapRagged,
            Self::ResponseRetriesExhausted => CommandFailureKind::ResponseRetriesExhausted,
            Self::ResponseLinkVanished => CommandFailureKind::ResponseLinkVanished,
            Self::ResponseTransferUnopenable => CommandFailureKind::ResponseTransferUnopenable,
            Self::ResponseTransferCorrupt => CommandFailureKind::ResponseTransferCorrupt,
            Self::ResponseProofUnsendable => CommandFailureKind::ResponseProofUnsendable,
            Self::ResponseDecompressionFailed => CommandFailureKind::ResponseDecompressionFailed,
            Self::ResponseDecompressionTimedOut => {
                CommandFailureKind::ResponseDecompressionTimedOut
            }
            Self::ResponseOpenTimedOut => CommandFailureKind::ResponseOpenTimedOut,
            Self::ResponseMetadataOverrun => CommandFailureKind::ResponseMetadataOverrun,
        }
    }

    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::BindFailed { detail }
            | Self::WriteFailed { detail }
            | Self::InvalidConfiguration { detail }
            | Self::PermissionDenied { detail }
            | Self::DeviceUnavailable { detail }
            | Self::ConnectFailed { detail }
            | Self::BackendFailed { detail } => Some(detail),
            Self::NodeStopped
            | Self::Busy
            | Self::PayloadTooLarge
            | Self::UnknownDestination
            | Self::NotSingleDestination
            | Self::AnnounceAppDataTooLong
            | Self::UnknownInterface
            | Self::NoRouteToDestination
            | Self::NotDirectlyReachable
            | Self::PacketCulled
            | Self::DeliveryTimedOut
            | Self::InvalidBitrate
            | Self::UnsupportedByBackend
            | Self::UnknownLink
            | Self::LinkNotActive
            | Self::EntropyUnavailable
            | Self::NotLinkInitiator
            | Self::IdentityNotHeld
            | Self::UnknownRequestHandler
            | Self::RequestPolicyNotAllowList
            | Self::RequestAllowListFull
            | Self::LinkBusy
            | Self::ResourceTableFull
            | Self::ResourceMetadataTooLarge
            | Self::ResourceRejectedByPeer
            | Self::ResourceSequencingFailed
            | Self::ResourcePredecessorFailed
            | Self::ChannelWindowFull
            | Self::ChannelUntrackable
            | Self::InvalidChannelMessageType
            | Self::ResourceUploadCancelled
            | Self::ResourceEarlyEof
            | Self::ResourceLengthOverrun
            | Self::ResponseTooLarge
            | Self::LinkClosed
            | Self::ResponseCancelledBySender
            | Self::ResponseHashmapBeyondPartCount
            | Self::ResponseHashmapSkipsAhead
            | Self::ResponseHashmapTooLong
            | Self::ResponseHashmapRagged
            | Self::ResponseRetriesExhausted
            | Self::ResponseLinkVanished
            | Self::ResponseTransferUnopenable
            | Self::ResponseTransferCorrupt
            | Self::ResponseProofUnsendable
            | Self::ResponseDecompressionFailed
            | Self::ResponseDecompressionTimedOut
            | Self::ResponseOpenTimedOut
            | Self::ResponseMetadataOverrun => None,
        }
    }
}
