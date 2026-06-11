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

pub const READ_BUF_LEN: usize = 256;
/// CDC-ACM behind a USB bridge: nominal line rate, conservative for tiering.
pub const SERIAL_BITRATE_BPS: u32 = 1_000_000;
pub const SERIAL_HW_MTU: usize = 1_024;
pub const SERIAL_FRAME_LEN: usize = SERIAL_HW_MTU + crate::interfaces::ifac::IFAC_MAX_SIZE;
pub const FRAMED_LEN: usize = rns_serial_framing::max_encoded_len(SERIAL_FRAME_LEN);
pub type Decoder = RnsSerialDecoder<SERIAL_FRAME_LEN>;

pub fn descriptor(id: InterfaceId) -> InterfaceConfig {
    InterfaceConfig {
        id,
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
        },
        mode: InterfaceMode::PointToPoint,
        hardware_mtu: Some(SERIAL_HW_MTU),
        announce_rate_limit: None,
        bitrate_bps: Some(SERIAL_BITRATE_BPS),
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
