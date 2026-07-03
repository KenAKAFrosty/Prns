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
use crate::units::Rtt;
use crate::wire::DestinationHash;

#[repr(C)]
pub enum EngineReaction<'a> {
    Journaled(Journaled<'a>),
    Directive(Directive<'a>),
}

#[repr(C)]
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
    LinkEstablished(LinkEstablished),
    /// The initiator of an active link revealed (and proved) the identity it
    /// holds — RNS 1.3.5's `remote_identified` callback as data.
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
        rtt: Rtt,
        data: &'a [u8],
    },
    ResponseReceived {
        command_id: CommandId,
        link_id: LinkId,
        request_id: RequestId,
        data: &'a [u8],
    },
    /// RNS 1.3.5 `Channel._receive`'s callback as data.
    ChannelMessageReceived {
        link_id: LinkId,
        message_type: MessageType,
        data: &'a [u8],
    },
    LinkClosed {
        link_id: LinkId,
        reason: LinkClosedReason,
    },
    /// RNS 1.3.5 `Link.receive` (Link.py:975): a packet for an active link arrived on an interface
    /// other than the one the link is attached to — dropped unprocessed, as a possible manipulation
    /// attempt, and surfaced here so the foreign-interface signal is observable rather than silent.
    LinkInterfaceMismatch {
        link_id: LinkId,
        attached_interface: InterfaceId,
        arrived_on: InterfaceId,
    },
    /// RNS 1.3.5's `resource_concluded` callback as data.
    ResourceReceived {
        link_id: LinkId,
        hash: ResourceHash,
        data: &'a [u8],
    },

    ResourceFailed {
        link_id: LinkId,
        hash: ResourceHash,
    },
    /// The host sizes its inflate output by `uncompressed_data_len` — the decompression-bomb guard.
    ResourceNeedsDecompression {
        link_id: LinkId,
        hash: ResourceHash,
        stream: &'a [u8],
        uncompressed_data_len: u64,
    },
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
    Broadcast {
        supervisor: InterfaceKind,
        fan: FanTarget,
        bytes: &'a [u8],
    },
    BroadcastAnnounce {
        supervisor: InterfaceKind,
        fan: FanTarget,
        bytes: &'a [u8],
        hops: u8,
    },
    /// The driver calls `fill` exactly once, with at least `size_hint` bytes — even on a full
    /// lane (its own scratch): the engine's bookkeeping runs inside `fill`.
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
