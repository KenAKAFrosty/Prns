use alloc::string::String;
use alloc::vec::Vec;

use crate::{
    DestinationHash, IdentityHash, InterfaceId, LinkClosedReason, LinkId, PersistenceFlushCause,
    PersistenceFlushTarget, RequestId, RequestPathHash, ResourceAvailable, ResourceHash,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SingleDelivery {
    pub destination: DestinationHash,
    pub source_interface: InterfaceId,
    pub plaintext: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkDelivery {
    pub link_id: LinkId,
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
pub struct ResponseSegmentAvailable {
    pub link_id: LinkId,
    pub request_id: RequestId,
    pub segment_index: u64,
    pub total_segments: u64,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceSegmentAvailable {
    pub link_id: LinkId,
    pub original_hash: ResourceHash,
    pub segment_index: u64,
    pub total_segments: u64,
    pub metadata: Option<Vec<u8>>,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceNeedsDecompression {
    pub link_id: LinkId,
    pub hash: ResourceHash,
    pub stream: Vec<u8>,
    pub uncompressed_data_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelMessage {
    pub link_id: LinkId,
    pub message_type: u16,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplicationEvent {
    SingleDelivery(SingleDelivery),
    LinkDelivery(LinkDelivery),
    Request(RequestAvailable),
    Response(ResponseAvailable),
    ResponseSegment(ResponseSegmentAvailable),
    ResourceAvailable(ResourceAvailable),
    ResourceSegment(ResourceSegmentAvailable),
    ResourceNeedsDecompression(ResourceNeedsDecompression),
    ChannelMessage(ChannelMessage),
}

impl ApplicationEvent {
    #[must_use]
    pub fn retained_bytes(&self) -> usize {
        match self {
            Self::SingleDelivery(event) => event.plaintext.len(),
            Self::LinkDelivery(event) => event.plaintext.len(),
            Self::Request(event) => event.data.len(),
            Self::Response(event) => event.data.len(),
            Self::ResponseSegment(event) => event.data.len(),
            Self::ResourceAvailable(event) => event
                .metadata
                .as_ref()
                .map_or(0, Vec::len)
                .saturating_add(usize::try_from(event.total_bytes).unwrap_or(usize::MAX)),
            Self::ResourceSegment(event) => event
                .data
                .len()
                .saturating_add(event.metadata.as_ref().map_or(0, Vec::len)),
            Self::ResourceNeedsDecompression(event) => event.stream.len(),
            Self::ChannelMessage(event) => event.data.len(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagnosticEvent {
    AnnounceHeard {
        destination: DestinationHash,
        hops: u8,
        source_interface: InterfaceId,
        app_data: Vec<u8>,
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
    LinkInterfaceMismatch {
        link_id: LinkId,
        attached_interface: InterfaceId,
        arrived_on: InterfaceId,
    },
    ResourceAssembled {
        link_id: LinkId,
        original_hash: ResourceHash,
        total_size_bytes: u64,
    },
    ResourceFailed {
        link_id: LinkId,
        hash: ResourceHash,
        cause: String,
    },
    ResourceSendProgress {
        link_id: LinkId,
        transferred_bytes: u64,
        total_bytes: u64,
        physical_transferred_bytes: u64,
        segment_index: u64,
        total_segments: u64,
    },
    SelfRatchetRotated {
        destination: DestinationHash,
    },
    AnnounceHeldDropped {
        destination: DestinationHash,
        source_interface: InterfaceId,
        cause: String,
    },
    Delivered {
        detail: String,
    },
    RouteExpired {
        destination: DestinationHash,
    },
    RouteEvicted {
        destination: DestinationHash,
    },
    RouteInterfaceGone {
        destination: DestinationHash,
    },
    RouteDropped {
        destination: DestinationHash,
    },
    BackendDiagnostic {
        kind: String,
        detail: String,
    },
    PersistenceRestored {
        routes: u64,
        destination_identities: u64,
        tunnels: u64,
        ratchets: u64,
        refused: u64,
        dropped: u64,
    },
    PersistenceFlushed {
        cause: PersistenceFlushCause,
        target: PersistenceFlushTarget,
    },
    PersistenceFlushFailed {
        cause: PersistenceFlushCause,
        target: PersistenceFlushTarget,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticBatch {
    pub events: Vec<DiagnosticEvent>,
    pub dropped_newest: u128,
}
