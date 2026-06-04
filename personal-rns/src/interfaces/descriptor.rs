use crate::interfaces::{
    ConnectionState, InterfaceCapabilities, InterfaceId, InterfaceMode, MediumKind,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceDescriptor {
    pub id: InterfaceId,
    pub capabilities: InterfaceCapabilities,
    pub mode: InterfaceMode,
    pub medium: MediumKind,
    pub state: ConnectionState,
}
