use crate::interfaces::{
    FrameSink, InterfaceDescriptor, InterfaceKind, InterfaceOriginKind, PacketPhyStats,
};

pub use prns_core::interfaces::wire_limits::{
    frame_cap_for, BROADCAST_WIRE_FRAME_LEN, EMBEDDED_MAX_LINK_MTU, EMBEDDED_MAX_WIRE_FRAME_LEN,
    MAX_WIRE_FRAME_LEN,
};

/// One interface's side of the reactor boundary: inbound frames accumulate in [`inbound_sink`](Self::inbound_sink) and cross on [`commit_inbound`](Self::commit_inbound), and [`next_outbound`](Self::next_outbound) parks until the reactor has a frame for this interface to transmit. An outbound frame arrives already committed: the engine wrote it into the lane's slot and let go in its own synchronous step before `next_outbound` resolves, so the returned borrow points into the lane, never into the engine, and holding it across the transmit await pins nothing.
#[allow(async_fn_in_trait)]
pub trait InterfaceSeam {
    fn interface_origin(&self) -> InterfaceOriginKind {
        InterfaceOriginKind::Configured
    }

    /// The storage the frame being received accumulates in — the seam's granted inbound slot, so a streaming deframer's writes land once, already across the seam. Parks until a slot is free (backpressure: an interface that cannot grant stops reading its medium). Repeated calls before [`commit_inbound`](Self::commit_inbound) return the same storage with its accumulation intact, so one frame may arrive across many reads.
    async fn inbound_sink(&mut self) -> &mut dyn FrameSink;

    /// Hand the reactor the frame accumulated in [`inbound_sink`](Self::inbound_sink) and release the storage. An empty sink commits nothing — delimiter-only keepalives die here, in one place, for every interface.
    async fn commit_inbound(&mut self);

    /// Hand the reactor one whole frame heard on the medium — the datagram path, derived from the sink pair. A frame past the sink's capacity is dropped whole.
    async fn next_inbound(&mut self, frame: &[u8]) {
        let sink = self.inbound_sink().await;
        sink.clear();
        if sink.extend_from_slice(frame).is_err() {
            return;
        }
        self.commit_inbound().await;
    }

    async fn next_inbound_with_phy(&mut self, frame: &[u8], _phy: PacketPhyStats) {
        self.next_inbound(frame).await;
    }

    /// Borrowed in place; the borrow releases on the following call.
    async fn next_outbound(&mut self) -> &[u8];

    /// A further frame already committed for this interface, if one is waiting. Never parks; the borrow contract matches [`next_outbound`](Self::next_outbound). Serve loops use it to coalesce a burst that queued behind the frame being written into one wire write; the default never offers one, so a seam without it simply never batches.
    fn try_next_outbound(&mut self) -> Option<&[u8]> {
        None
    }

    async fn request_tunnel_synthesis(&mut self) {}
}

#[allow(async_fn_in_trait)]
pub trait Interface {
    const HW_MTU: usize;

    /// The medium this interface speaks, which is also the namespace root of its id ([`from_channel_tag`](crate::interfaces::InterfaceId::from_channel_tag)).
    const KIND: InterfaceKind;

    /// There is one hard contract for `channel_tag()`: distinct bytes across distinct concurrent communication channels, and the same bytes across every reconnect and reboot for that effective channel.
    ///
    /// The tag names *which* channel of this medium the interface is: e.g., a TCP `host:port`, a BLE peer MAC, a LoRa frequency + modulation profile. [`InterfaceId::from_channel_tag`](crate::interfaces::InterfaceId::from_channel_tag) hashes it into this interface's id (`[KIND] ++ sha256(tag)[..7]`), the engine's entire notion of the interface: routes, links, and the departure grace all key on it. Same tag means the re-attached interface is the old one and its routes survive; a shared tag would fuse two distinct live channels into the same id.
    fn channel_tag(&self) -> &[u8];

    fn descriptor(&self) -> InterfaceDescriptor;

    async fn run<S: InterfaceSeam>(self, seam: S);
}
