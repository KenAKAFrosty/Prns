use alloc::string::String;
use alloc::vec::Vec;

use crate::{DestinationHash, InterfaceId, LinkId, PacketHash};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bitrate {
    Auto,
    BitsPerSecond(u64),
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
    DetachInterface {
        interface: InterfaceId,
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
}
