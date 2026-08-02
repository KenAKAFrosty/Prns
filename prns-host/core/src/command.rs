use alloc::string::String;
use alloc::vec::Vec;

use crate::{
    DestinationHash, IdentityHash, InterfaceConfig, InterfaceId, LinkId, PacketHash, RequestId,
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
}
