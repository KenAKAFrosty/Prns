use crate::engine::commands::{CommandId, LinkEstablished, Settlement};
use crate::engine::InstantMillis;
use crate::identity::IdentityHash;
use crate::interfaces::{InterfaceId, InterfaceKind};
use crate::routing::delivery::Delivery;
use crate::routing::links::channel::MessageType;
use crate::routing::links::request::RequestId;
use crate::routing::links::resources::ResourceHash;
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
    /// the settlement carries the round trip. `command_id` names the
    /// `SendRequest` it answers, so a caller awaiting that request can demux the
    /// data without re-deriving the request id.
    ResponseReceived {
        command_id: CommandId,
        link_id: LinkId,
        request_id: RequestId,
        data: &'a [u8],
    },
    /// A sequenced channel message reassembled into delivery order — RNS 1.3.1
    /// `Channel._receive`'s callback as data. `message_type` is the envelope's
    /// opaque type tag; `data` is the message body, borrowed for this reaction
    /// only (it rides the arriving packet, or the reorder buffer the in-order
    /// run just drained). One arrival can journal several of these in order.
    ChannelMessageReceived {
        link_id: LinkId,
        message_type: MessageType,
        data: &'a [u8],
    },
    LinkClosed {
        link_id: LinkId,
        reason: LinkClosedReason,
    },
    /// RNS 1.3.1 `Link.receive` (Link.py:975): a packet for an active link arrived on an interface
    /// other than the one the link is attached to — dropped unprocessed, as a possible manipulation
    /// attempt, and surfaced here so the foreign-interface signal is observable rather than silent.
    LinkInterfaceMismatch {
        link_id: LinkId,
        attached_interface: InterfaceId,
        arrived_on: InterfaceId,
    },
    /// RNS 1.3.1's  `resource_concluded` callback as data.
    /// The bytes are borrowed from the register and gone after this reaction returns.
    ResourceReceived {
        link_id: LinkId,
        hash: ResourceHash,
        data: &'a [u8],
    },

    ResourceFailed {
        link_id: LinkId,
        hash: ResourceHash,
    },
    /// A complete compressed transfer opened to its bz2 stream — the host
    /// owns the inflate. It answers with `provide_decompressed`, sizing its
    /// output buffer from `uncompressed_data_len` (the advertised size every
    /// honest stream fills exactly — the engine's decompression-bomb guard).
    /// The stream bytes are borrowed from the register and gone after this
    /// reaction returns.
    ResourceNeedsDecompression {
        link_id: LinkId,
        hash: ResourceHash,
        stream: &'a [u8],
        uncompressed_data_len: u64,
    },
    /// One concluded segment of a multi-segment transfer. The bytes are borrowed
    /// from the register and gone after this reaction returns.
    ResourceSegmentReceived {
        link_id: LinkId,
        original_hash: ResourceHash,
        segment_index: u64,
        total_segments: u64,
        data: &'a [u8],
    },
    ResourceAssembled {
        link_id: LinkId,
        original_hash: ResourceHash,
        total_size: u64,
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

/// Which members of a fleet a self-originated frame is fanned across, in the supervisor's own
/// vocabulary. `All` reaches every live member; `Only` exactly one (a directed send); `AllExcept`
/// every member but one — the source-withheld rebroadcast that never echoes a frame back onto the
/// interface it arrived on. The engine hands the supervisor this intent for a whole fleet in one
/// [`Directive::Broadcast`], so a shared lane carries one frame, not one per member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FanTarget {
    All,
    Only(InterfaceId),
    AllExcept(InterfaceId),
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
    /// Fan one self-originated frame across a whole fleet in a single directive: every live member
    /// of `supervisor`'s kind that `fan` selects. The supervisor owns one shared lane, so the
    /// reactor commits one frame carrying `fan`, and the supervisor delivers it to each selected
    /// peer — never a frame per member colliding on a depth-1 lane. Dedicated 1:1 interfaces keep
    /// their per-interface [`Send`](Self::Send).
    Broadcast {
        supervisor: InterfaceKind,
        fan: FanTarget,
        bytes: &'a [u8],
    },
    /// The announce twin of [`Broadcast`](Self::Broadcast): a relayed or own announce fanned across
    /// a fleet, carrying the `hops` count the wire is stamped with on emit.
    BroadcastAnnounce {
        supervisor: InterfaceKind,
        fan: FanTarget,
        bytes: &'a [u8],
        hops: u8,
    },
    /// A frame owed to one interface, written straight into the wire slot the driver
    /// provides — grant-first emission: sealed once, in place, never staged and copied.
    /// `size_hint` is the engine's upper bound on the frame's wire length, so the driver can
    /// size a growable slot to exactly the frame and render in place rather than into a
    /// max-sized buffer. The driver calls `fill` exactly once, with a buffer of at least
    /// `size_hint` bytes — the granted egress slot when one is free, or its own discard
    /// scratch when the lane is full (the engine's bookkeeping must run either way; a full
    /// lane drops the frame exactly as `Send` always has). `fill` returns the wire length to
    /// commit, or `None` when the engine found nothing to emit after all.
    EmitFrame {
        target: InterfaceId,
        size_hint: usize,
        fill: &'a mut dyn FnMut(&mut [u8]) -> Option<usize>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkClosedReason {
    Timeout,
    PeerClosed,
    Protocol,
}
