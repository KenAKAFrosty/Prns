use crate::interfaces::{
    ConnectionState, InterfaceCapabilities, InterfaceId, InterfaceMode, MediumKind,
};

/// The routing-relevant facts about an interface — everything the engine needs
/// to make fanout and routing decisions, and nothing about byte I/O. An interface
/// presents one so the runtime can register and route to it without reaching into how
/// it talks to the world. The engine reads this as needed; the interface owns
/// everything below it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceDescriptor {
    pub id: InterfaceId,
    pub capabilities: InterfaceCapabilities,
    pub mode: InterfaceMode,
    pub medium: MediumKind,
    pub state: ConnectionState,
}
