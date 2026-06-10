use crate::engine::commands::{CommandId, Settlement};
use crate::interfaces::InterfaceId;
use crate::routing::delivery::Delivery;
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
