use crate::interfaces::{
    AnnounceBandwidthCap, BitrateBps, EffectiveInterfacePolicy, EgressCapability,
    IngressCapability, InterfaceCapabilities, InterfaceDefaults, InterfaceGravityDefault,
    InterfaceMode, MtuPolicy, TransportCapability, IFAC_MAX_SIZE,
};

pub const HARDWARE_MTU: usize = super::FRAME_MTU - IFAC_MAX_SIZE;

/// The caller supplies its radio profile's payload-rate estimate, separately for
/// shared broadcasts and peer unicasts. This does not program the PHY.
#[must_use]
pub fn policy_for_bitrate(bitrate: BitrateBps) -> EffectiveInterfacePolicy {
    InterfaceDefaults {
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
        },
        mode: InterfaceMode::Full,
        gravity: InterfaceGravityDefault::FromBitrate,
        bitrate,
        mtu: MtuPolicy::fixed(HARDWARE_MTU),
        announce_rate_limit: None,
        announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
        airtime_duty_cycle: None,
    }
    .configured(Default::default())
}
