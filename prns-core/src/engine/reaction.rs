use crate::engine::InstantMillis;
use crate::engine::{CommandId, LinkEstablished, Settlement};
use crate::identity::IdentityHash;
use crate::interfaces::{InterfaceId, InterfaceKind};
use crate::routing::announce::held::HeldDropCause;
use crate::routing::delivery::Delivery;
use crate::routing::links::channel::MessageType;
use crate::routing::links::request::RequestId;
use crate::routing::links::resources::{ResourceFailureCause, ResourceHash};
use crate::routing::links::LinkId;
use crate::routing::request_handlers::RequestPathHash;
use crate::routing::RouteRemovalCause;
use crate::units::RttMillis;
use crate::wire::DestinationHash;

// repr(C) on this enum, Journaled, and Directive: they cross the dual-core channel; see the layout note on [`EngineCommand`].
#[repr(C)]
pub enum EngineReaction<'a> {
    /// A notice that something has already happened within the engine.
    Journaled(Journaled<'a>),

    /// An order for something that must now happen outside it.
    Directive(Directive<'a>),
}

#[repr(C)]
pub enum Journaled<'a> {
    /// RNS 1.3.5's announce-handler `received_announce` callback as data.
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
    /// RNS 1.3.5's destination `set_packet_callback` delivery as data.
    Delivered(Delivery<'a>),

    CommandSettled {
        id: CommandId,
        settlement: Settlement,
    },

    /// RNS 1.3.5's `set_link_established_callback` as data.
    LinkEstablished(LinkEstablished),

    /// The link initiator revealed and proved the identity it holds: RNS 1.3.5's `remote_identified` callback as data.
    PeerIdentified {
        link_id: LinkId,
        identity: IdentityHash,
    },

    /// RNS 1.3.5's request handler callback as data.
    RequestReceived {
        link_id: LinkId,
        request_id: RequestId,
        path_hash: RequestPathHash,
        requested_at: InstantMillis,
        rtt: RttMillis,
        data: &'a [u8],
    },

    /// RNS 1.3.5's request `response_callback` as data.
    ResponseReceived {
        command_id: CommandId,
        link_id: LinkId,
        request_id: RequestId,
        data: &'a [u8],
    },

    /// One verified segment of a split response resource; the receive gate refuses out-of-order chains, so these concatenate in arrival order.
    /// The request settles as `Settlement::SendRequest` when the final segment assembles, not through a [`Journaled::ResponseReceived`].
    ResponseSegmentReceived {
        command_id: CommandId,
        link_id: LinkId,
        request_id: RequestId,
        segment_index: u64,
        total_segments: u64,
        data: &'a [u8],
    },

    /// RNS 1.3.5 `Channel._receive`'s callback as data.
    ChannelMessageReceived {
        link_id: LinkId,
        message_type: MessageType,
        data: &'a [u8],
    },

    /// RNS 1.3.5's `set_link_closed_callback` as data.
    LinkClosed {
        link_id: LinkId,
        reason: LinkClosedReason,
    },

    /// RNS 1.3.5 `Link.receive`: a packet for an active link arrived on an interface other than the link's own, dropped unprocessed as a possible manipulation attempt and surfaced so the signal is observable rather than silent.
    LinkInterfaceMismatch {
        link_id: LinkId,
        attached_interface: InterfaceId,
        arrived_on: InterfaceId,
    },

    /// RNS 1.3.5's `resource_concluded` callback as data.
    /// `metadata` is the transfer's packed metadata, stripped from the stream head, opaque to the engine; `None` when none traveled.
    ResourceReceived {
        link_id: LinkId,
        hash: ResourceHash,
        metadata: Option<&'a [u8]>,
        data: &'a [u8],
    },

    /// The failure half of RNS 1.3.5's `resource_concluded` callback, with the cause the reference never names.
    ResourceFailed {
        link_id: LinkId,
        hash: ResourceHash,
        cause: ResourceFailureCause,
    },

    ResourceNeedsDecompression {
        link_id: LinkId,
        hash: ResourceHash,
        stream: &'a [u8],
        uncompressed_data_len: u64,
    },

    /// One segment of a split resource landed / progress toward [`Journaled::ResourceAssembled`].
    /// `metadata` rides segment one only, stripped from the stream head like the single-segment delivery.
    ResourceSegmentReceived {
        link_id: LinkId,
        original_hash: ResourceHash,
        segment_index: u64,
        total_segments: u64,
        metadata: Option<&'a [u8]>,
        data: &'a [u8],
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkClosedReason {
    Timeout,
    PeerClosed,
    /// The peer's link-RTT message failed to parse during establishment.
    MalformedRtt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FanTarget {
    All,
    Only(InterfaceId),
    AllExcept(InterfaceId),
}

#[repr(C)]
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
    SendToFleet {
        supervisor: InterfaceKind,
        fan: FanTarget,
        bytes: &'a [u8],
    },
    SendAnnounceToFleet {
        supervisor: InterfaceKind,
        fan: FanTarget,
        bytes: &'a [u8],
        hops: u8,
    },
    /// The driver calls `fill` exactly once, with at least `size_hint` bytes, even on a full lane (its own scratch). The engine's bookkeeping runs inside `fill`.
    EmitFrame {
        target: InterfaceId,
        size_hint: usize,
        fill: &'a mut dyn FnMut(&mut [u8]) -> Option<usize>,
    },
}
