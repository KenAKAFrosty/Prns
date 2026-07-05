use crate::interfaces::ifac::IFAC_MAX_SIZE;
use crate::interfaces::{InterfaceDescriptor, InterfaceKind};

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

/// One interface's side of the reactor boundary: [`next_inbound`](Self::next_inbound) hands the reactor a frame heard on the medium, and [`next_outbound`](Self::next_outbound) parks until the reactor has a frame for this interface to transmit.
/// An outbound frame arrives already committed: the engine wrote it into the lane's slot and let go in its own synchronous step before `next_outbound` resolves, so the returned borrow points into the lane, never into the engine, and holding it across the transmit await pins nothing.
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

    /// The medium this interface speaks, which is also the namespace root of its id
    /// ([`from_channel_tag`](crate::interfaces::InterfaceId::from_channel_tag)).
    const KIND: InterfaceKind;

    /// IMPORTANT! There is one hard contract you alone must honor with a channel_tag(): distinct bytes across distinct concurrent communication channels, and the same bytes across every reconnect and reboot for that effective channel.
    ///
    /// The tag names *which* channel of this medium the interface is: e.g., a TCP `host:port`, a BLE peer MAC, a LoRa frequency + modulation profile.
    /// [`InterfaceId::from_channel_tag`](crate::interfaces::InterfaceId::from_channel_tag) hashes it into this interface's id (`[KIND] ++ sha256(tag)[..7]`), the engine's entire notion of the interface: routes, links, and the departure grace all key on it.
    /// Same tag means the re-attached interface IS the old one and its routes survive; a shared tag would fuse two distinct live channels into the same id.
    fn channel_tag(&self) -> &[u8];

    fn descriptor(&self) -> InterfaceDescriptor;

    async fn run<S: InterfaceSeam>(self, seam: S);
}
