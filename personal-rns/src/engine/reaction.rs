use crate::interfaces::InterfaceId;
use crate::wire::DestinationHash;

/// One thing the reactor emits while it digests a single input — a packet, a command,
/// or a due deadline. The reactor pushes these to a sink in the order they occur, so a
/// growable output list never has to be stored: it is the ordered effect list, streamed.
///
/// The variants are added as each engine method is re-cut onto this surface; today it
/// carries only the announce-into-rebroadcast path.
pub enum EngineReaction<'a> {
    /// Something that already happened — for the application to observe.
    Journaled(Journaled),
    /// Something still owed to the outside world — for the driver to carry out.
    Directive(Directive<'a>),
}

/// Past tense: by the time the sink sees a `Journaled`, it is already true of the reactor.
pub enum Journaled {
    AnnounceHeard {
        destination: DestinationHash,
        hops: u8,
        source_interface: InterfaceId,
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
}
