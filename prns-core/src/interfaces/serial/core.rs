//! The host-agnostic core of the serial interface: the sizing the read and write loops are
//! built around, and the descriptor the engine sees. The framing brain lives once in the
//! shared serve loop (`interfaces::framed_stream`); each host's impl supplies only
//! the async byte stream.

use crate::interfaces::rns_serial_framing::{self, RnsSerialDecoder};
use crate::interfaces::{
    AnnounceBandwidthCap, BitrateBps, EgressCapability, IngressCapability, InterfaceCapabilities,
    InterfaceDescriptor, InterfaceId, InterfaceMode, TransportCapability,
};

pub const READ_BUF_LEN: usize = 256;
/// CDC-ACM behind a USB bridge: nominal line rate, conservative for tiering.
pub const SERIAL_BITRATE_BPS: BitrateBps = BitrateBps::guess(1_000_000);
pub const SERIAL_HW_MTU: usize = 1_024;
pub const SERIAL_FRAME_LEN: usize = SERIAL_HW_MTU + crate::interfaces::ifac::IFAC_MAX_SIZE;
pub const FRAMED_LEN: usize = rns_serial_framing::max_encoded_len(SERIAL_FRAME_LEN);
pub type Decoder = RnsSerialDecoder<SERIAL_FRAME_LEN>;

pub fn descriptor(id: InterfaceId) -> InterfaceDescriptor {
    InterfaceDescriptor {
        id,
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
        },
        mode: InterfaceMode::PointToPoint,
        hardware_mtu: Some(SERIAL_HW_MTU),
        announce_rate_limit: None,
        bitrate: SERIAL_BITRATE_BPS,
        announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
        airtime_duty_cycle: None,
    }
}
