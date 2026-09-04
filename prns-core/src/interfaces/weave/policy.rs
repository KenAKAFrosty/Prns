use crate::interfaces::{
    AnnounceBandwidthCap, BitrateBps, ConfiguredInterfacePolicy, EffectiveInterfacePolicy,
    EgressCapability, IngressCapability, InterfaceCapabilities, InterfaceDefaults,
    InterfaceDescriptor, InterfaceId, InterfaceMode, MtuPolicy, TransportCapability,
};

pub const WEAVE_SERIAL_BAUD: u32 = 3_000_000;
pub const WEAVE_BITRATE_ESTIMATE: BitrateBps = BitrateBps::guess(250_000);
pub const WEAVE_HW_MTU: usize = 1_024;
pub const WEAVE_MAX_WIRE_PACKET: usize = WEAVE_HW_MTU + crate::interfaces::IFAC_MAX_SIZE;

pub const DEFAULTS: InterfaceDefaults = InterfaceDefaults {
    capabilities: InterfaceCapabilities {
        ingress: IngressCapability::Enabled,
        egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
    },
    mode: InterfaceMode::Full,
    gravity: crate::interfaces::InterfaceGravityDefault::FromBitrate,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_reference_weave_medium() {
        let policy = configured_policy(ConfiguredInterfacePolicy::default());
        assert_eq!(policy.bitrate, WEAVE_BITRATE_ESTIMATE);
        assert_eq!(policy.mode, InterfaceMode::Full);
        assert_eq!(policy.mtu.resolve(policy.bitrate), Some(WEAVE_HW_MTU));
    }
}
