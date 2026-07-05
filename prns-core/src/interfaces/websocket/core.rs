use crate::interfaces::ifac::IFAC_MAX_SIZE;
use crate::interfaces::{
    hardware_mtu_for_bitrate, AnnounceBandwidthCap, BitrateBps, EgressCapability,
    IngressCapability, InterfaceCapabilities, InterfaceDescriptor, InterfaceId, InterfaceMode,
    TransportCapability,
};
use crate::routing::links::MAX_LINK_MTU;

/// A conservative default for hosts that do not know their real pipe speed. It mirrors the modern
/// TCP default because WebSocket rides the same routed-IP substrate.
pub const WEBSOCKET_BITRATE_GUESS_BPS: BitrateBps = BitrateBps::guess(1_000_000_000);

/// The largest payload this interface can carry in a single WebSocket binary message.
pub const WEBSOCKET_HW_MTU_CAP: usize = MAX_LINK_MTU;
pub const FRAME_CAP: usize = MAX_LINK_MTU + IFAC_MAX_SIZE;

pub fn descriptor(id: InterfaceId, bitrate: BitrateBps) -> InterfaceDescriptor {
    InterfaceDescriptor {
        id,
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
        },
        mode: InterfaceMode::PointToPoint,
        hardware_mtu: hardware_mtu_for_bitrate(bitrate.get()).map(|tier| tier.min(MAX_LINK_MTU)),
        announce_rate_limit: None,
        bitrate,
        announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
        airtime_duty_cycle: None,
    }
}
