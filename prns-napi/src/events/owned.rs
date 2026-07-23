use personal_rns::engine::{LinkClosedReason, RouteRemovalCause};
use personal_rns::routing::delivery::Delivery;
use personal_rns::runtime::{Diagnostic, Message, PrnsEvent};

pub enum OwnedEvent {
    Announce {
        destination: [u8; 16],
        hops: u8,
        source_interface: [u8; 8],
    },
    SingleDelivery {
        destination: [u8; 16],
        plaintext: Vec<u8>,
        source_interface: [u8; 8],
    },
    Request {
        destination: [u8; 16],
        link_id: [u8; 16],
        request_id: [u8; 16],
        requester: Option<[u8; 16]>,
        path_hash: [u8; 16],
        rtt_millis: u64,
        data: Vec<u8>,
    },
    Response {
        link_id: [u8; 16],
        request_id: [u8; 16],
        data: Vec<u8>,
    },
    ResponseSegment {
        link_id: [u8; 16],
        request_id: [u8; 16],
        segment_index: u64,
        total_segments: u64,
        data: Vec<u8>,
    },
    Resource {
        link_id: [u8; 16],
        hash: Vec<u8>,
        metadata: Option<Vec<u8>>,
        data: Vec<u8>,
    },
    ResourceSegment {
        link_id: [u8; 16],
        original_hash: Vec<u8>,
        segment_index: u64,
        total_segments: u64,
        metadata: Option<Vec<u8>>,
        data: Vec<u8>,
    },
    ChannelMessage {
        link_id: [u8; 16],
        message_type: String,
        data: Vec<u8>,
    },
    LinkEstablished {
        link_id: [u8; 16],
        rtt_millis: u64,
    },
    PeerIdentified {
        link_id: [u8; 16],
        identity: [u8; 16],
    },
    LinkClosed {
        link_id: [u8; 16],
        reason: &'static str,
    },
    CommandSettled {
        id: u64,
        settlement: String,
    },
    SelfRatchetRotated {
        destination: [u8; 16],
    },
    AnnounceHeldDropped {
        destination: [u8; 16],
        source_interface: [u8; 8],
        cause: String,
    },
    LinkInterfaceMismatch {
        link_id: [u8; 16],
        attached_interface: [u8; 8],
        arrived_on: [u8; 8],
    },
    ResourceAssembled {
        link_id: [u8; 16],
        original_hash: Vec<u8>,
        total_size: u64,
    },
    ResourceFailed {
        link_id: [u8; 16],
        hash: Vec<u8>,
        cause: String,
    },
    RouteRemoved {
        destination: [u8; 16],
        kind: &'static str,
    },
    ResourceSendProgress {
        link_id: [u8; 16],
        transferred: u64,
        total: u64,
        physical_transferred: u64,
        segment_index: u64,
        total_segments: u64,
    },
    Uncategorized {
        kind: &'static str,
        detail: String,
    },
    NodeStopped {
        cause: String,
    },
}

impl OwnedEvent {
    pub fn capture(event: PrnsEvent<'_>) -> Option<Self> {
        match event {
            PrnsEvent::Message(message) => Self::capture_message(message),
            PrnsEvent::Diagnostic(diagnostic) => Some(Self::capture_diagnostic(diagnostic)),
        }
    }

    fn capture_message(message: Message<'_>) -> Option<Self> {
        match message {
            Message::Delivered(Delivery::Single(delivery)) => Some(Self::SingleDelivery {
                destination: *delivery.destination.as_bytes(),
                plaintext: delivery.plaintext.to_vec(),
                source_interface: *delivery.source_interface.as_bytes(),
            }),
            Message::Delivered(other) => Some(Self::Uncategorized {
                kind: "delivered",
                detail: format!("{other:?}"),
            }),
            Message::Request {
                destination,
                link_id,
                request_id,
                requester,
                path_hash,
                requested_at: _,
                rtt,
                data,
            } => Some(Self::Request {
                destination: *destination.as_bytes(),
                link_id: *link_id.as_bytes(),
                request_id: request_id.0,
                requester: requester.map(|identity| *identity.as_bytes()),
                path_hash: *path_hash.as_bytes(),
                rtt_millis: rtt.millis(),
                data: data.to_vec(),
            }),
            Message::Response {
                link_id,
                request_id,
                data,
            } => Some(Self::Response {
                link_id: *link_id.as_bytes(),
                request_id: request_id.0,
                data: data.to_vec(),
            }),
            Message::ResponseSegment {
                link_id,
                request_id,
                segment_index,
                total_segments,
                data,
            } => Some(Self::ResponseSegment {
                link_id: *link_id.as_bytes(),
                request_id: request_id.0,
                segment_index,
                total_segments,
                data: data.to_vec(),
            }),
            Message::Resource {
                link_id,
                hash,
                metadata,
                data,
            } => Some(Self::Resource {
                link_id: *link_id.as_bytes(),
                hash: hash.as_bytes().to_vec(),
                metadata: metadata.map(<[u8]>::to_vec),
                data: data.to_vec(),
            }),
            Message::ResourceNeedsDecompression { .. } => None,
            Message::ResourceSegment {
                link_id,
                original_hash,
                segment_index,
                total_segments,
                metadata,
                data,
            } => Some(Self::ResourceSegment {
                link_id: *link_id.as_bytes(),
                original_hash: original_hash.as_bytes().to_vec(),
                segment_index,
                total_segments,
                metadata: metadata.map(<[u8]>::to_vec),
                data: data.to_vec(),
            }),
            Message::ChannelMessage {
                link_id,
                message_type,
                data,
            } => Some(Self::ChannelMessage {
                link_id: *link_id.as_bytes(),
                message_type: format!("{message_type:?}"),
                data: data.to_vec(),
            }),
        }
    }

    fn capture_diagnostic(diagnostic: Diagnostic) -> Self {
        match diagnostic {
            Diagnostic::AnnounceHeard {
                destination,
                hops,
                source_interface,
            } => Self::Announce {
                destination: *destination.as_bytes(),
                hops,
                source_interface: *source_interface.as_bytes(),
            },
            Diagnostic::LinkEstablished(established) => Self::LinkEstablished {
                link_id: *established.link_id.as_bytes(),
                rtt_millis: established.rtt_ms,
            },
            Diagnostic::PeerIdentified { link_id, identity } => Self::PeerIdentified {
                link_id: *link_id.as_bytes(),
                identity: *identity.as_bytes(),
            },
            Diagnostic::LinkClosed { link_id, reason } => Self::LinkClosed {
                link_id: *link_id.as_bytes(),
                reason: match reason {
                    LinkClosedReason::Timeout => "timeout",
                    LinkClosedReason::PeerClosed => "peerClosed",
                    LinkClosedReason::MalformedRtt => "malformedRtt",
                },
            },
            Diagnostic::CommandSettled { id, settlement } => Self::CommandSettled {
                id: id.0,
                settlement: format!("{settlement:?}"),
            },
            Diagnostic::SelfRatchetRotated { destination } => Self::SelfRatchetRotated {
                destination: *destination.as_bytes(),
            },
            Diagnostic::AnnounceHeldDropped {
                destination,
                source_interface,
                cause,
            } => Self::AnnounceHeldDropped {
                destination: *destination.as_bytes(),
                source_interface: *source_interface.as_bytes(),
                cause: format!("{cause:?}"),
            },
            Diagnostic::LinkInterfaceMismatch {
                link_id,
                attached_interface,
                arrived_on,
            } => Self::LinkInterfaceMismatch {
                link_id: *link_id.as_bytes(),
                attached_interface: *attached_interface.as_bytes(),
                arrived_on: *arrived_on.as_bytes(),
            },
            Diagnostic::ResourceAssembled {
                link_id,
                original_hash,
                total_size,
            } => Self::ResourceAssembled {
                link_id: *link_id.as_bytes(),
                original_hash: original_hash.as_bytes().to_vec(),
                total_size,
            },
            Diagnostic::ResourceFailed {
                link_id,
                hash,
                cause,
            } => Self::ResourceFailed {
                link_id: *link_id.as_bytes(),
                hash: hash.as_bytes().to_vec(),
                cause: format!("{cause:?}"),
            },
            Diagnostic::RouteRemoved { destination, cause } => Self::RouteRemoved {
                destination: *destination.as_bytes(),
                kind: match cause {
                    RouteRemovalCause::Expired => "routeExpired",
                    RouteRemovalCause::Evicted => "routeEvicted",
                    RouteRemovalCause::InterfaceGone => "routeInterfaceGone",
                    RouteRemovalCause::Dropped => "routeDropped",
                },
            },
        }
    }
}
