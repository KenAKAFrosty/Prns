use crate::engine::commands::{CommandId, LinkEstablished, Settlement};
use crate::engine::InstantMillis;
use crate::identity::IdentityHash;
use crate::interfaces::InterfaceId;
use crate::routing::delivery::Delivery;
use crate::routing::links::request::RequestId;
use crate::routing::links::LinkId;
use crate::routing::request_handlers::RequestPathHash;
use crate::wire::DestinationHash;

pub enum EngineReaction<'a> {
    /// Something that already happened. For the application to observe.
    Journaled(Journaled<'a>),
    /// Something still owed to the outside world. For the driver to carry out.
    Directive(Directive<'a>),
}

/// Past tense: by the time the sink sees a `Journaled`, it is already true of the reactor.
pub enum Journaled<'a> {
    AnnounceHeard {
        destination: DestinationHash,
        hops: u8,
        source_interface: InterfaceId,
    },
    Delivered(Delivery<'a>),
    CommandSettled {
        id: CommandId,
        settlement: Settlement,
    },
    /// A link another node initiated to one of our destinations went ACTIVE on
    /// its LRRTT. Our own initiations settle through `CommandSettled` instead.
    LinkEstablished(LinkEstablished),
    /// The initiator of an active link revealed (and proved) the identity it
    /// holds — RNS 1.3.1's `remote_identified` callback as data.
    PeerIdentified {
        link_id: LinkId,
        identity: IdentityHash,
    },
    /// RNS 1.3.1's request handler callback as data: a sealed request passed
    /// the registry's allow gate, and the app owes the response — answered
    /// with a `Respond` command naming `request_id`. `data` is the request's
    /// raw msgpack value bytes, app-owned.
    RequestReceived {
        link_id: LinkId,
        request_id: RequestId,
        path_hash: RequestPathHash,
        requested_at: InstantMillis,
        data: &'a [u8],
    },
    /// The response that settled a `SendRequest` — the bytes ride here while
    /// the settlement carries the round trip.
    ResponseReceived {
        link_id: LinkId,
        request_id: RequestId,
        data: &'a [u8],
    },
    LinkClosed {
        link_id: LinkId,
        reason: LinkClosedReason,
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
}

pub enum Directive<'a> {
    Send {
        target: InterfaceId,
        bytes: &'a [u8],
    },
    SendAnnounce {
        target: InterfaceId,
        bytes: &'a [u8],
        hops: u8,
    },
    /// A frame owed to one interface, written straight into the wire slot the driver
    /// provides — grant-first emission: sealed once, in place, never staged and copied.
    /// The driver calls `fill` exactly once, with a buffer of at least
    /// `MAX_WIRE_FRAME_LEN` bytes — the granted egress slot when one is free, or its own
    /// discard scratch when the lane is full (the engine's bookkeeping must run either
    /// way; a full lane drops the frame exactly as `Send` always has). `fill` returns the
    /// wire length to commit, or `None` when the engine found nothing to emit after all.
    EmitFrame {
        target: InterfaceId,
        fill: &'a mut dyn FnMut(&mut [u8]) -> Option<usize>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkClosedReason {
    Timeout,
    PeerClosed,
    Protocol,
}
