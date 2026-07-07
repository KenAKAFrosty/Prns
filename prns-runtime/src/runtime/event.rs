//! The app-facing event lane, curated from the engine's `Journaled` stream, split so an app
//! can silo its two concerns:
//!
//!   - [`Message`]: payload arrived *for the app* (delivered singles/links, requests to
//!     answer, responses, resources). The data plane.
//!   - [`Diagnostic`]: what the engine did (announces heard, settlements, link lifecycle,
//!     route churn, failures). Observability, not payload.
//!
//! The mapping is total: every `Journaled` lands in exactly one bucket.

use crate::engine::LinkClosedReason;
use crate::engine::{CommandId, HeldDropCause, LinkEstablished, RouteRemovalCause, Settlement};
use crate::engine::{InstantMillis, Journaled};
use crate::identity::IdentityHash;
use crate::interfaces::InterfaceId;
use crate::routing::delivery::Delivery;
use crate::routing::links::channel::MessageType;
use crate::routing::links::request::RequestId;
use crate::routing::links::resources::ResourceHash;
use crate::routing::links::LinkId;
use crate::routing::request_handlers::RequestPathHash;
use crate::units::RttMillis;
use crate::wire::DestinationHash;

#[derive(Debug)]
pub enum PrnsEvent<'a> {
    Message(Message<'a>),
    Diagnostic(Diagnostic),
}

/// The data plane: bytes the app owns.
#[derive(Debug)]
pub enum Message<'a> {
    Delivered(Delivery<'a>),
    Request {
        link_id: LinkId,
        request_id: RequestId,
        path_hash: RequestPathHash,
        requested_at: InstantMillis,
        rtt: RttMillis,
        data: &'a [u8],
    },
    Response {
        link_id: LinkId,
        request_id: RequestId,
        data: &'a [u8],
    },
    Resource {
        link_id: LinkId,
        hash: ResourceHash,
        data: &'a [u8],
    },
    ResourceNeedsDecompression {
        link_id: LinkId,
        hash: ResourceHash,
        stream: &'a [u8],
        uncompressed_data_len: u64,
    },
    ResourceSegment {
        link_id: LinkId,
        original_hash: ResourceHash,
        segment_index: u64,
        total_segments: u64,
        data: &'a [u8],
    },
    ChannelMessage {
        link_id: LinkId,
        message_type: MessageType,
        data: &'a [u8],
    },
}

/// The control/observability plane: what the engine did, not what arrived. Fully owned —
/// no borrow into the inbound frame, so it can outlive the reaction if an app buffers it.
#[derive(Debug)]
pub enum Diagnostic {
    AnnounceHeard {
        destination: DestinationHash,
        hops: u8,
        source_interface: InterfaceId,
    },
    AnnounceHeldDropped {
        destination: DestinationHash,
        source_interface: InterfaceId,
        cause: HeldDropCause,
    },
    CommandSettled {
        id: CommandId,
        settlement: Settlement,
    },
    LinkEstablished(LinkEstablished),
    PeerIdentified {
        link_id: LinkId,
        identity: IdentityHash,
    },
    LinkClosed {
        link_id: LinkId,
        reason: LinkClosedReason,
    },
    /// A packet for this active link arrived on `arrived_on`, not the `attached_interface` the link
    /// runs over — dropped unprocessed (RNS 1.3.5 `Link.receive`), surfaced as a possible attempt to
    /// inject into the link from a foreign interface.
    LinkInterfaceMismatch {
        link_id: LinkId,
        attached_interface: InterfaceId,
        arrived_on: InterfaceId,
    },
    ResourceFailed {
        link_id: LinkId,
        hash: ResourceHash,
    },
    ResourceAssembled {
        link_id: LinkId,
        original_hash: ResourceHash,
        total_size: u64,
    },
    RouteRemoved {
        destination: DestinationHash,
        cause: RouteRemovalCause,
    },
}

impl<'a> From<Journaled<'a>> for PrnsEvent<'a> {
    fn from(journaled: Journaled<'a>) -> Self {
        match journaled {
            Journaled::Delivered(delivery) => PrnsEvent::Message(Message::Delivered(delivery)),
            Journaled::RequestReceived {
                link_id,
                request_id,
                path_hash,
                requested_at,
                rtt,
                data,
            } => PrnsEvent::Message(Message::Request {
                link_id,
                request_id,
                path_hash,
                requested_at,
                rtt,
                data,
            }),
            Journaled::ResponseReceived {
                link_id,
                request_id,
                data,
                ..
            } => PrnsEvent::Message(Message::Response {
                link_id,
                request_id,
                data,
            }),
            Journaled::ResourceReceived {
                link_id,
                hash,
                data,
            } => PrnsEvent::Message(Message::Resource {
                link_id,
                hash,
                data,
            }),
            Journaled::ResourceNeedsDecompression {
                link_id,
                hash,
                stream,
                uncompressed_data_len,
            } => PrnsEvent::Message(Message::ResourceNeedsDecompression {
                link_id,
                hash,
                stream,
                uncompressed_data_len,
            }),
            Journaled::ResourceSegmentReceived {
                link_id,
                original_hash,
                segment_index,
                total_segments,
                data,
            } => PrnsEvent::Message(Message::ResourceSegment {
                link_id,
                original_hash,
                segment_index,
                total_segments,
                data,
            }),
            Journaled::ChannelMessageReceived {
                link_id,
                message_type,
                data,
            } => PrnsEvent::Message(Message::ChannelMessage {
                link_id,
                message_type,
                data,
            }),
            Journaled::AnnounceHeard {
                destination,
                hops,
                source_interface,
            } => PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard {
                destination,
                hops,
                source_interface,
            }),
            Journaled::AnnounceHeldDropped {
                destination,
                source_interface,
                cause,
            } => PrnsEvent::Diagnostic(Diagnostic::AnnounceHeldDropped {
                destination,
                source_interface,
                cause,
            }),
            Journaled::CommandSettled { id, settlement } => {
                PrnsEvent::Diagnostic(Diagnostic::CommandSettled { id, settlement })
            }
            Journaled::LinkEstablished(established) => {
                PrnsEvent::Diagnostic(Diagnostic::LinkEstablished(established))
            }
            Journaled::PeerIdentified { link_id, identity } => {
                PrnsEvent::Diagnostic(Diagnostic::PeerIdentified { link_id, identity })
            }
            Journaled::LinkInterfaceMismatch {
                link_id,
                attached_interface,
                arrived_on,
            } => PrnsEvent::Diagnostic(Diagnostic::LinkInterfaceMismatch {
                link_id,
                attached_interface,
                arrived_on,
            }),
            Journaled::LinkClosed { link_id, reason } => {
                PrnsEvent::Diagnostic(Diagnostic::LinkClosed { link_id, reason })
            }
            Journaled::ResourceFailed { link_id, hash } => {
                PrnsEvent::Diagnostic(Diagnostic::ResourceFailed { link_id, hash })
            }
            Journaled::ResourceAssembled {
                link_id,
                original_hash,
                total_size,
            } => PrnsEvent::Diagnostic(Diagnostic::ResourceAssembled {
                link_id,
                original_hash,
                total_size,
            }),
            Journaled::RouteRemoved { destination, cause } => {
                PrnsEvent::Diagnostic(Diagnostic::RouteRemoved { destination, cause })
            }
        }
    }
}
