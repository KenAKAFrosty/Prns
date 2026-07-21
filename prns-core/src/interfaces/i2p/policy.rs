use crate::interfaces::{
    AnnounceBandwidthCap, BitrateBps, ConfiguredInterfacePolicy, EffectiveInterfacePolicy,
    EgressCapability, IngressCapability, InterfaceCapabilities, InterfaceDefaults,
    InterfaceDescriptor, InterfaceId, InterfaceMode, MtuPolicy, TransportCapability,
};

pub const I2P_BITRATE_ESTIMATE: BitrateBps = BitrateBps::guess(256_000);
pub const I2P_HW_MTU: usize = 1_064;

pub const DEFAULTS: InterfaceDefaults = InterfaceDefaults {
    capabilities: InterfaceCapabilities {
        ingress: IngressCapability::Enabled,
        egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
    },
    mode: InterfaceMode::Full,
    bitrate: I2P_BITRATE_ESTIMATE,
    mtu: MtuPolicy::fixed(I2P_HW_MTU),
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
    fn defaults_match_rns_i2p_medium_policy() {
        let policy = configured_policy(ConfiguredInterfacePolicy::default());
        assert_eq!(policy.bitrate, BitrateBps::guess(256_000));
        assert_eq!(policy.mode, InterfaceMode::Full);
        assert_eq!(policy.mtu.resolve(policy.bitrate), Some(1_064));
    }

    #[test]
    fn configured_bitrate_does_not_change_the_fixed_i2p_mtu() {
        let policy = configured_policy(ConfiguredInterfacePolicy {
            bitrate: Some(BitrateBps::guess(1_000_000_000)),
            ..ConfiguredInterfacePolicy::default()
        });
        assert_eq!(policy.bitrate, BitrateBps::guess(1_000_000_000));
        assert_eq!(policy.mtu.resolve(policy.bitrate), Some(1_064));
    }
}
