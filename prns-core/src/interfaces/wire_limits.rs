use super::ifac::IFAC_MAX_SIZE;
use super::InterfaceDescriptor;

pub const MAX_WIRE_FRAME_LEN: usize = crate::routing::links::MAX_LINK_MTU + IFAC_MAX_SIZE;

/// 1472 (wire 1536): every embedded radio fits under it (ESP-NOW 1406, BLE 500, LoRa ~250, WiFi Auto 1196);
pub const EMBEDDED_MAX_LINK_MTU: usize = 1_472;
pub const EMBEDDED_MAX_WIRE_FRAME_LEN: usize = EMBEDDED_MAX_LINK_MTU + IFAC_MAX_SIZE;

pub const BROADCAST_WIRE_FRAME_LEN: usize = crate::wire::BROADCAST_MTU + IFAC_MAX_SIZE;

pub fn frame_cap_for(descriptor: &InterfaceDescriptor) -> usize {
    descriptor
        .hardware_mtu
        .unwrap_or(crate::wire::BROADCAST_MTU)
        + IFAC_MAX_SIZE
}
