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
use crate::engine::{InstantMillis, Journaled, PersistenceFlushCause, PersistenceFlushTarget};
use crate::identity::IdentityHash;
use crate::interfaces::InterfaceId;
use crate::routing::delivery::Delivery;
use crate::routing::links::channel::MessageType;
use crate::routing::links::request::RequestId;
use crate::routing::links::resources::{ResourceFailureCause, ResourceHash};
use crate::routing::links::LinkId;
use crate::routing::request_handlers::RequestPathHash;
use crate::units::RttMillis;
use crate::wire::DestinationHash;

#[derive(Debug)]
pub enum PrnsEvent<'a> {
    Message(Message<'a>),
    Diagnostic(Diagnostic<'a>),
}

/// The data plane: bytes the app owns.
#[derive(Debug)]
pub enum Message<'a> {
    Delivered(Delivery<'a>),
    Request {
        destination: DestinationHash,
        link_id: LinkId,
        request_id: RequestId,
        requester: Option<IdentityHash>,
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
    /// One in-order segment of a split response; the request's settlement arrives as a [`Diagnostic::CommandSettled`] when the final segment assembles.
    ResponseSegment {
        link_id: LinkId,
        request_id: RequestId,
        segment_index: u64,
        total_segments: u64,
        data: &'a [u8],
    },
    Resource {
        link_id: LinkId,
        hash: ResourceHash,
        /// The transfer's packed metadata, stripped from the stream head, opaque to the engine; `None` when none traveled.
        metadata: Option<&'a [u8]>,
        data: &'a [u8],
    },
    ResourceNeedsDecompression {
        link_id: LinkId,
        hash: ResourceHash,
        stream: &'a [u8],
        uncompressed_data_bytes: u64,
    },
    ResourceSegment {
        link_id: LinkId,
        original_hash: ResourceHash,
        segment_index: u64,
        total_segments: u64,
        /// Rides segment one only, stripped from the stream head like the single-segment delivery.
        metadata: Option<&'a [u8]>,
        data: &'a [u8],
    },
    ChannelMessage {
        link_id: LinkId,
        message_type: MessageType,
        data: &'a [u8],
    },
}

#[derive(Debug)]
pub enum Diagnostic<'a> {
    /// An announce just minted a fresh self-ratchet: flush this destination's record to the
    /// vault now — a secret peers may already encrypt toward must never exist only in memory.
    SelfRatchetRotated {
        destination: DestinationHash,
    },
    AnnounceHeard {
        destination: DestinationHash,
        hops: u8,
        source_interface: InterfaceId,
        app_data: &'a [u8],
    },
    /// The recipe's persistence store was seeded into this boot's engine before the first frame moved.
    PersistenceRestored {
        routes: u32,
        destination_identities: u32,
        tunnels: u32,
        ratchets: u32,
        refused: u32,
        dropped: u32,
    },
    /// The persistence worker landed one independently stored part of a save.
    PersistenceFlushed {
        cause: PersistenceFlushCause,
        target: PersistenceFlushTarget,
    },
    /// The persistence worker could not land one independently stored part of a save.
    ///
    /// Storage-specific error detail is written to the host log; this owned diagnostic
    /// preserves the stable policy-relevant facts for applications.
    PersistenceFlushFailed {
        cause: PersistenceFlushCause,
        target: PersistenceFlushTarget,
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
    /// runs over — dropped unprocessed (RNS 1.4.2 `Link.receive`), surfaced as a possible attempt to
    /// inject into the link from a foreign interface.
    LinkInterfaceMismatch {
        link_id: LinkId,
        attached_interface: InterfaceId,
        arrived_on: InterfaceId,
    },
    ResourceFailed {
        link_id: LinkId,
        hash: ResourceHash,
        cause: ResourceFailureCause,
    },
    ResourceAssembled {
        link_id: LinkId,
        original_hash: ResourceHash,
        total_size_bytes: u64,
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
                destination,
                link_id,
                request_id,
                requester,
                path_hash,
                requested_at,
                rtt,
                data,
            } => PrnsEvent::Message(Message::Request {
                destination,
                link_id,
                request_id,
                requester,
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
            Journaled::ResponseSegmentReceived {
                link_id,
                request_id,
                segment_index,
                total_segments,
                data,
                ..
            } => PrnsEvent::Message(Message::ResponseSegment {
                link_id,
                request_id,
                segment_index,
                total_segments,
                data,
            }),
            Journaled::ResourceReceived {
                link_id,
                hash,
                metadata,
                data,
            } => PrnsEvent::Message(Message::Resource {
                link_id,
                hash,
                metadata,
                data,
            }),
            Journaled::ResourceNeedsDecompression {
                link_id,
                hash,
                stream,
                uncompressed_data_bytes,
            } => PrnsEvent::Message(Message::ResourceNeedsDecompression {
                link_id,
                hash,
                stream,
                uncompressed_data_bytes,
            }),
            Journaled::ResourceSegmentReceived {
                link_id,
                original_hash,
                segment_index,
                total_segments,
                metadata,
                data,
            } => PrnsEvent::Message(Message::ResourceSegment {
                link_id,
                original_hash,
                segment_index,
                total_segments,
                metadata,
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
            Journaled::AnnounceHeard { observation, .. } => {
                PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard {
                    destination: observation.destination,
                    hops: observation.hops.0,
                    source_interface: observation.source_interface,
                    app_data: observation.app_data,
                })
            }
            Journaled::SelfRatchetRotated { destination } => {
                PrnsEvent::Diagnostic(Diagnostic::SelfRatchetRotated { destination })
            }
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
            Journaled::PersistenceFlushed { cause, target } => {
                PrnsEvent::Diagnostic(Diagnostic::PersistenceFlushed { cause, target })
            }
            Journaled::PersistenceFlushFailed { cause, target } => {
                PrnsEvent::Diagnostic(Diagnostic::PersistenceFlushFailed { cause, target })
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
            Journaled::ResourceFailed {
                link_id,
                hash,
                cause,
            } => PrnsEvent::Diagnostic(Diagnostic::ResourceFailed {
                link_id,
                hash,
                cause,
            }),
            Journaled::ResourceAssembled {
                link_id,
                original_hash,
                total_size_bytes,
            } => PrnsEvent::Diagnostic(Diagnostic::ResourceAssembled {
                link_id,
                original_hash,
                total_size_bytes,
            }),
            Journaled::RouteRemoved { destination, cause } => {
                PrnsEvent::Diagnostic(Diagnostic::RouteRemoved { destination, cause })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::announce::{AnnounceObservation, AnnounceRateAccounting};
    use crate::units::HopCount;

    #[test]
    fn announce_event_preserves_application_data() {
        let app_data = b"opaque announce application data";
        let event = PrnsEvent::from(Journaled::AnnounceHeard {
            observation: AnnounceObservation {
                destination: DestinationHash::new([1; 16]),
                announced_identity: IdentityHash::new([2; 16]),
                hops: HopCount(3),
                source_interface: InterfaceId::new([4; 8]),
                arrived_at: InstantMillis(5),
                app_data,
                is_path_response: false,
            },
            rate_accounting: AnnounceRateAccounting::NotApplied,
        });

        assert!(matches!(
            event,
            PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard {
                app_data: observed,
                ..
            }) if observed == app_data
        ));
    }
}
