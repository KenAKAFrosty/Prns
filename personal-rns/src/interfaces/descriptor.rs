use crate::interfaces::{Capabilities, ConnectionState, InterfaceId, InterfaceMode, MediumKind};

/// The routing-relevant facts about an interface — everything the engine needs
/// to make fanout and routing decisions, and nothing about byte I/O. An
/// [`InterfaceWorker`](crate::interfaces::InterfaceWorker) presents one so the
/// runtime can register and route to it without reaching into how it talks to
/// the world. The engine reads this as needed; the worker owns everything below it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceDescriptor {
    pub id: InterfaceId,
    pub capabilities: Capabilities,
    pub mode: InterfaceMode,
    pub medium: MediumKind,
    pub state: ConnectionState,
}
