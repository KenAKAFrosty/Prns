//! The host-agnostic core of the pipe interface: the sizing the read and write loops are built
//! around and the descriptor the engine sees. RNS `PipeInterface` packetizes with the same HDLC
//! octet-stuffing the serial and TCP interfaces use, so the framing lives once in
//! [`rns_serial_framing`] and the serve loop in
//! `prns-interfaces-tokio`'s `framed_stream`; each host's impl supplies only the byte
//! stream — here, the stdin/stdout of a spawned subprocess.

use crate::interfaces::rns_serial_framing::{self, RnsSerialDecoder};
use crate::interfaces::{
    AnnounceBandwidthCap, BitrateBps, ConfiguredInterfacePolicy, EffectiveInterfacePolicy,
    EgressCapability, IngressCapability, InterfaceCapabilities, InterfaceDefaults,
    InterfaceDescriptor, InterfaceId, InterfaceMode, MtuPolicy, TransportCapability,
};

pub const READ_BUF_LEN: usize = 256;
/// RNS `PipeInterface.BITRATE_GUESS` — a local subprocess pipe, generously a megabit for tiering.
pub const PIPE_BITRATE_BPS: BitrateBps = BitrateBps::guess(1_000_000);
/// RNS `PipeInterface.HW_MTU`.
pub const PIPE_HW_MTU: usize = 1_064;
pub const PIPE_FRAME_LEN: usize = PIPE_HW_MTU + crate::interfaces::IFAC_MAX_SIZE;
pub const FRAMED_LEN: usize = rns_serial_framing::max_encoded_len(PIPE_FRAME_LEN);
pub type Decoder = RnsSerialDecoder<PIPE_FRAME_LEN>;

pub const DEFAULTS: InterfaceDefaults = InterfaceDefaults {
    capabilities: InterfaceCapabilities {
        ingress: IngressCapability::Enabled,
        egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
    },
    mode: InterfaceMode::PointToPoint,
    bitrate: PIPE_BITRATE_BPS,
    mtu: MtuPolicy::fixed(PIPE_HW_MTU),
    announce_rate_limit: None,
    announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
    airtime_duty_cycle: None,
};

#[must_use]
pub fn configured_policy(configured: ConfiguredInterfacePolicy) -> EffectiveInterfacePolicy {
    DEFAULTS.configured(configured)
}

pub fn descriptor(id: InterfaceId, policy: EffectiveInterfacePolicy) -> InterfaceDescriptor {
    policy.descriptor(id)
}
