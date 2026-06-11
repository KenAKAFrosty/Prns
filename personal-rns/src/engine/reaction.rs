use crate::engine::commands::{CommandId, LinkEstablished, Settlement};
use crate::identity::IdentityHash;
use crate::interfaces::InterfaceId;
use crate::routing::delivery::Delivery;
use crate::routing::links::LinkId;
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkClosedReason {
    Timeout,
    PeerClosed,
    Protocol,
}
