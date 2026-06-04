use crate::interfaces::{
    ConnectionState, EgressCapability, IngressCapability, InterfaceCapabilities,
    InterfaceDescriptor, InterfaceId, InterfaceMode, MediumKind, TransitCapability,
};
use crate::wire::MTU;

pub const SERIAL_MTU: usize = MTU;

pub fn descriptor(id: InterfaceId) -> InterfaceDescriptor {
    InterfaceDescriptor {
        id,
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransitCapability::CrossInterfaceOnly),
        },
        mode: InterfaceMode::PointToPoint,
        medium: MediumKind::DirectPeer,
        state: ConnectionState::Connected,
    }
}
