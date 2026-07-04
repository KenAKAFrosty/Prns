use crate::interfaces::ifac::IFAC_MAX_SIZE;
use crate::interfaces::{InterfaceConfig, InterfaceKind};

pub const MAX_WIRE_FRAME_LEN: usize = crate::routing::links::MAX_LINK_MTU + IFAC_MAX_SIZE;

/// 1472 (wire 1536): every embedded radio fits under it (ESP-NOW 1406, BLE 500, LoRa ~250, WiFi Auto 1196);
pub const EMBEDDED_MAX_LINK_MTU: usize = 1_472;
pub const EMBEDDED_MAX_WIRE_FRAME_LEN: usize = EMBEDDED_MAX_LINK_MTU + IFAC_MAX_SIZE;

pub const BROADCAST_WIRE_FRAME_LEN: usize = crate::wire::BROADCAST_MTU + IFAC_MAX_SIZE;

pub fn frame_cap_for(descriptor: &InterfaceConfig) -> usize {
    descriptor
        .hardware_mtu
        .unwrap_or(crate::wire::BROADCAST_MTU)
        + IFAC_MAX_SIZE
}

/// By the time `next_outbound` yields, the frame already lives in the queue and no
/// engine buffer is borrowed — the sync egress the borrow invariant demands stays on
/// the reactor's side.
#[allow(async_fn_in_trait)]
pub trait InterfaceSeam {
    async fn next_inbound(&mut self, frame: &[u8]);
    /// Borrowed in place; the borrow releases on the following call.
    async fn next_outbound(&mut self) -> &[u8];

    async fn request_tunnel_synthesis(&mut self) {}
}

#[allow(async_fn_in_trait)]
pub trait Interface {
    const HW_MTU: usize;

    /// The medium this interface speaks / the namespace root of its id
    /// ([`from_channel_tag`](crate::interfaces::InterfaceId::from_channel_tag)).
    const KIND: InterfaceKind;

    fn channel_tag(&self) -> &[u8];

    fn descriptor(&self) -> InterfaceConfig;

    async fn run<S: InterfaceSeam>(self, seam: S);
}
