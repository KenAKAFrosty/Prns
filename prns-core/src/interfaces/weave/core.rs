use crate::interfaces::rns_serial_framing;
use crate::interfaces::{
    AnnounceBandwidthCap, BitrateBps, ConfiguredInterfacePolicy, EffectiveInterfacePolicy,
    EgressCapability, IngressCapability, InterfaceCapabilities, InterfaceDefaults,
    InterfaceDescriptor, InterfaceId, InterfaceMode, MtuPolicy, TransportCapability,
};

pub const WEAVE_SERIAL_BAUD: u32 = 3_000_000;
pub const WEAVE_BITRATE_ESTIMATE: BitrateBps = BitrateBps::guess(250_000);
pub const WEAVE_HW_MTU: usize = 1_024;
pub const WEAVE_MAX_WIRE_PACKET: usize = WEAVE_HW_MTU + crate::interfaces::ifac::IFAC_MAX_SIZE;
pub const PEERING_TIMEOUT_MILLIS: u64 = 20_000;
pub const HANDSHAKE_TIMEOUT_MILLIS: u64 = 2_000;
pub const MULTIPATH_DEDUPLICATION_MILLIS: u64 = 750;
pub const MULTIPATH_DEDUPLICATION_CAPACITY: usize = 48;
pub const READ_BUF_LEN: usize = 1_500;
pub const WDCL_MAX_CHUNK: usize = 32_768;
pub const FRAMED_LEN: usize = rns_serial_framing::max_encoded_len(WDCL_MAX_CHUNK);

pub const DEFAULTS: InterfaceDefaults = InterfaceDefaults {
    capabilities: InterfaceCapabilities {
        ingress: IngressCapability::Enabled,
        egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
    },
    mode: InterfaceMode::Full,
    bitrate: WEAVE_BITRATE_ESTIMATE,
    mtu: MtuPolicy::fixed(WEAVE_HW_MTU),
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
