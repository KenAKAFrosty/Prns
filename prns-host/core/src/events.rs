use alloc::string::String;
use alloc::vec::Vec;

use crate::{
    DestinationHash, IdentityHash, InterfaceId, LinkId, RequestId, RequestPathHash,
    ResourceAvailable,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SingleDelivery {
    pub destination: DestinationHash,
    pub source_interface: InterfaceId,
    pub plaintext: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestAvailable {
    pub destination: DestinationHash,
    pub link_id: LinkId,
    pub request_id: RequestId,
    pub requester: Option<IdentityHash>,
    pub path_hash: RequestPathHash,
    pub rtt_millis: u64,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResponseAvailable {
    pub link_id: LinkId,
    pub request_id: RequestId,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelMessage {
    pub link_id: LinkId,
    pub message_type: String,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplicationEvent {
    SingleDelivery(SingleDelivery),
    Request(RequestAvailable),
    Response(ResponseAvailable),
    ResourceAvailable(ResourceAvailable),
    ChannelMessage(ChannelMessage),
}

impl ApplicationEvent {
    #[must_use]
    pub fn retained_bytes(&self) -> usize {
        match self {
            Self::SingleDelivery(event) => event.plaintext.len(),
            Self::Request(event) => event.data.len(),
            Self::Response(event) => event.data.len(),
            Self::ResourceAvailable(event) => event.metadata.as_ref().map_or(0, Vec::len),
            Self::ChannelMessage(event) => {
                event.message_type.len().saturating_add(event.data.len())
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkClosedReason {
    Timeout,
    PeerClosed,
    MalformedRtt,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagnosticEvent {
    AnnounceHeard {
        destination: DestinationHash,
        hops: u8,
        source_interface: InterfaceId,
    },
    LinkEstablished {
        link_id: LinkId,
        rtt_millis: u64,
    },
    PeerIdentified {
        link_id: LinkId,
        identity: IdentityHash,
    },
    LinkClosed {
        link_id: LinkId,
        reason: LinkClosedReason,
    },
    Backend {
        component: String,
        detail: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticBatch {
    pub events: Vec<DiagnosticEvent>,
    pub dropped_newest: u128,
}
