//! The host-agnostic core of the serial interface: the RNS HDLC-style framing the read and
//! write loops share. The per-host impls own only the async IO and the select primitive; the
//! deframe-and-hand-up / frame-for-the-wire logic lives here, identical across std and no_std
//! (it touches no executor, only the [`InterfaceSeam`] and the shared codec).

use crate::interfaces::rns_serial_framing::{self, RnsSerialDecoder};
use crate::interfaces::{
    AnnounceBandwidthCap, EgressCapability, IngressCapability, InterfaceCapabilities,
    InterfaceConfig, InterfaceId, InterfaceMode, TransportCapability,
};
use crate::reactor::interface_seam::InterfaceSeam;
use crate::wire::MTU;

pub const READ_BUF_LEN: usize = 256;
pub const FRAMED_LEN: usize = rns_serial_framing::max_encoded_len(MTU);
pub type Decoder = RnsSerialDecoder<MTU>;

pub fn descriptor(id: InterfaceId) -> InterfaceConfig {
    InterfaceConfig {
        id,
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
        },
        mode: InterfaceMode::PointToPoint,
        announce_rate_limit: None,
        bitrate_bps: Some(1_000_000),
        announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
        airtime_duty_cycle: None,
    }
}

pub async fn deframe_to_seam<Seam: InterfaceSeam>(
    decoder: &mut Decoder,
    chunk: &[u8],
    seam: &mut Seam,
) {
    for &byte in chunk {
        if let Ok(Some(frame)) = decoder.feed(byte) {
            if !frame.is_empty() {
                seam.next_inbound(frame).await;
            }
        }
    }
}

#[must_use]
pub fn frame_for_wire(packet: &[u8], buf: &mut [u8; FRAMED_LEN]) -> Option<usize> {
    rns_serial_framing::encode(packet, buf).ok()
}
