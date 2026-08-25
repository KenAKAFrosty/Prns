#[cfg(feature = "runtime-metrics")]
use super::metrics::AnnounceOrigin;
use crate::engine::InstantMillis;
use crate::engine::{CommandId, LinkEstablished, Settlement};
use crate::identity::IdentityHash;
use crate::interfaces::{InterfaceId, InterfaceKind};
use crate::routing::announce::held::HeldDropCause;
use crate::routing::announce::{AnnounceObservation, AnnounceRateAccounting};
use crate::routing::delivery::Delivery;
use crate::routing::links::channel::MessageType;
use crate::routing::links::request::RequestId;
use crate::routing::links::resources::{ResourceFailureCause, ResourceHash};
use crate::routing::links::LinkId;
use crate::routing::request_handlers::RequestPathHash;
use crate::routing::RouteRemovalCause;
use crate::units::RttMillis;
use crate::wire::DestinationHash;

// repr(C) on this enum, Journaled, and Directive: they cross the dual-core channel; see the layout note on [`PrnsCommand`].
#[repr(C)]
pub enum EngineReaction<'a> {
    /// A notice that something has just happened within the engine.
    Journaled(Journaled<'a>),

    /// An order for something that must now happen outside the engine.
    Directive(Directive<'a>),
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistenceFlushCause {
    Startup,
    Interval,
    RouteChange,
    RatchetRotation,
    Shutdown,
}

impl PersistenceFlushCause {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::Interval => "interval",
            Self::RouteChange => "route_change",
            Self::RatchetRotation => "ratchet_rotation",
            Self::Shutdown => "shutdown",
        }
    }
}

/// The independently stored half of a persistence flush.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistenceFlushTarget {
    RoutingState,
    Ratchets,
}

impl PersistenceFlushTarget {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::RoutingState => "routing_state",
            Self::Ratchets => "ratchets",
        }
    }
}

pub enum Journaled<'a> {
    /// RNS 1.4.2's announce-handler `received_announce` callback as data.
    AnnounceHeard {
        observation: AnnounceObservation<'a>,
        rate_accounting: AnnounceRateAccounting,
    },

    SelfRatchetRotated {
        destination: DestinationHash,
    },

    /// A host persistence worker injected an ordered save notice into the engine journal.
    /// The engine itself performs no storage IO.
    PersistenceFlushed {
        cause: PersistenceFlushCause,
        target: PersistenceFlushTarget,
    },

    /// A host persistence worker injected an ordered save-failure notice into the engine
    /// journal. Storage-specific error detail stays in the host log.
    PersistenceFlushFailed {
        cause: PersistenceFlushCause,
        target: PersistenceFlushTarget,
    },
    AnnounceHeldDropped {
        destination: DestinationHash,
        source_interface: InterfaceId,
        cause: HeldDropCause,
    },
    /// RNS 1.4.2's destination `set_packet_callback` delivery as data.
    ///
    /// Emitted synchronously before a corresponding [`ProofStrategy::ProveIf`](crate::routing::ProofStrategy::ProveIf)
    /// decision is requested and before any proof directive reaches egress. A host that
    /// durably records this callback in-stack therefore lands the inbound delivery before
    /// acknowledging it.
    Delivered(Delivery<'a>),

    CommandSettled {
        id: CommandId,
        settlement: Settlement,
    },

    /// RNS 1.4.2's `set_link_established_callback` as data.
    LinkEstablished(LinkEstablished),

    /// RNS 1.4.2's `remote_identified` callback as data.
    PeerIdentified {
        link_id: LinkId,
        identity: IdentityHash,
    },

    /// RNS 1.4.2's request handler callback as data.
    RequestReceived {
        destination: DestinationHash,
        link_id: LinkId,
        request_id: RequestId,
        requester: Option<IdentityHash>,
        path_hash: RequestPathHash,
        requested_at: InstantMillis,
        rtt: RttMillis,
        data: &'a [u8],
    },

    /// RNS 1.4.2's request `response_callback` as data.
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

    /// RNS 1.4.2 `Channel._receive`'s callback as data.
    ChannelMessageReceived {
        link_id: LinkId,
        message_type: MessageType,
        data: &'a [u8],
    },

    /// RNS 1.4.2's `set_link_closed_callback` as data.
    LinkClosed {
        link_id: LinkId,
        reason: LinkClosedReason,
    },

    /// RNS 1.4.2 `Link.receive`: a packet for an active link arrived on an interface other than the link's own, dropped unprocessed as a possible manipulation attempt.
    LinkInterfaceMismatch {
        link_id: LinkId,
        attached_interface: InterfaceId,
        arrived_on: InterfaceId,
    },

    /// RNS 1.4.2's `resource_concluded` callback as data.
    /// `metadata` is the transfer's packed metadata, stripped from the stream head, opaque to the engine.
    ResourceReceived {
        link_id: LinkId,
        hash: ResourceHash,
        metadata: Option<&'a [u8]>,
        data: &'a [u8],
    },

    /// The failure half of RNS 1.4.2's `resource_concluded` callback, with the cause the reference never names.
    ResourceFailed {
        link_id: LinkId,
        hash: ResourceHash,
        cause: ResourceFailureCause,
    },

    ResourceNeedsDecompression {
        link_id: LinkId,
        hash: ResourceHash,
        stream: &'a [u8],
        uncompressed_data_bytes: u64,
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
        total_size_bytes: u64,
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
    MalformedRtt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FanTarget {
    All,
    Only(InterfaceId),
    AllExcept(InterfaceId),
}

/// An order for something that must now happen outside the engine.
#[repr(C)]
pub enum Directive<'a> {
    Send {
        target: InterfaceId,
        bytes: &'a [u8],
    },
    SendIfOnline {
        target: InterfaceId,
        bytes: &'a [u8],
        on_send: &'a mut dyn FnMut(),
    },

    SendAnnounce {
        target: InterfaceId,
        bytes: &'a [u8],
        hops: u8,
        #[cfg(feature = "runtime-metrics")]
        origin: AnnounceOrigin,
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
        #[cfg(feature = "runtime-metrics")]
        origin: AnnounceOrigin,
    },
    /// The driver calls `fill` exactly once, with at least `size_hint` bytes, even on a full lane (its own scratch). The engine's bookkeeping runs inside `fill`.
    EmitFrame {
        target: InterfaceId,
        size_hint: usize,
        fill: &'a mut dyn FnMut(&mut [u8]) -> Option<usize>,
    },

    #[cfg(feature = "runtime-metrics")]
    SendMeasuredLocalAnnounce {
        target: InterfaceId,
        bytes: &'a [u8],
    },

    #[cfg(feature = "runtime-metrics")]
    SendMeasuredLocalAnnounceToFleet {
        supervisor: InterfaceKind,
        fan: FanTarget,
        bytes: &'a [u8],
    },
}
