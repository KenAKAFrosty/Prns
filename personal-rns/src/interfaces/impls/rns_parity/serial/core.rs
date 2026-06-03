//! Platform-agnostic part of the serial interface: its routing descriptor and
//! MTU. The wire framing is shared verbatim from
//! [`rns_serial_framing`](crate::interfaces::rns_serial_framing).

use crate::interfaces::{
    ConnectionState, EgressCapability, IngressCapability, InterfaceCapabilities,
    InterfaceDescriptor, InterfaceId, InterfaceMode, MediumKind, TransitCapability,
};
use crate::wire::MTU;

/// The largest serialized Reticulum packet a serial link carries — the engine
/// MTU. The worker's `PACKET_BUFFER_SIZE`, its outbound buffer, and its
/// frame-decoder cap all size to this.
pub const SERIAL_MTU: usize = MTU;

/// The routing facts a serial interface registers: a point-to-point direct peer
/// that receives, transmits, and forwards, with no in-medium repeat (a cable has
/// no broadcast domain to reflect a packet back into). Reported `Connected` once
/// the host has opened the port.
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
