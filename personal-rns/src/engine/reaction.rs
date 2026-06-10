use crate::engine::commands::{CommandId, Settlement};
use crate::interfaces::InterfaceId;
use crate::routing::delivery::Delivery;
use crate::wire::DestinationHash;

/// One thing the reactor emits while it digests a single input — a packet, a command,
/// or a due deadline. The reactor pushes these to a sink in the order they occur, so a
/// growable output list never has to be stored: it is the ordered effect list, streamed.
///
/// The variants are added as each engine method is re-cut onto this surface.
pub enum EngineReaction<'a> {
    /// Something that already happened — for the application to observe.
    Journaled(Journaled<'a>),
    /// Something still owed to the outside world — for the driver to carry out.
    Directive(Directive<'a>),
}

/// Past tense: by the time the sink sees a `Journaled`, it is already true of the reactor.
pub enum Journaled<'a> {
    /// An announce was accepted and its route learned.
    AnnounceHeard {
        destination: DestinationHash,
        hops: u8,
        source_interface: InterfaceId,
    },
    /// A packet addressed to a local destination was delivered (decrypted if sealed).
    Delivered(Delivery<'a>),
    /// An in-flight command reached a terminal result.
    CommandSettled {
        id: CommandId,
        settlement: Settlement,
    },
}

/// An edict for the I/O edge — still unhandled, ready to be carried out. Bytes are lent
/// from the reactor's own buffers for the duration of the sink call, so carrying out a
/// `Send` is a copy straight into the interface's grant, never an allocation.
pub enum Directive<'a> {
    Send {
        target: InterfaceId,
        bytes: &'a [u8],
    },
    SendAnnounce {
        target: InterfaceId,
        bytes: &'a [u8],
    },
}
