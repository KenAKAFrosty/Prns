use crate::interfaces::{
    AnnounceBandwidthCap, BitrateBps, ConfiguredInterfacePolicy, EffectiveInterfacePolicy,
    EgressCapability, IngressCapability, InterfaceCapabilities, InterfaceDefaults,
    InterfaceDescriptor, InterfaceId, InterfaceMode, MtuPolicy, TransportCapability,
};

pub const RNODE_HW_MTU: usize = 508;

#[must_use]
pub const fn nominal_bitrate_bps(spreading_factor: u8, coding_rate: u8, bandwidth_hz: u32) -> u32 {
    crate::interfaces::lora::nominal_lora_bitrate_bps(spreading_factor, coding_rate, bandwidth_hz)
}

#[must_use]
pub fn defaults_for_bitrate(bitrate: BitrateBps) -> InterfaceDefaults {
    InterfaceDefaults {
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransportCapability::SameInterfaceRepeat),
        },
        mode: InterfaceMode::Full,
        gravity: crate::interfaces::InterfaceGravity::ZERO,
        bitrate,
        mtu: MtuPolicy::fixed(RNODE_HW_MTU),
        announce_rate_limit: None,
        announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
        airtime_duty_cycle: None,
    }
}

#[must_use]
pub fn policy_for_bitrate(bitrate: BitrateBps) -> EffectiveInterfacePolicy {
    defaults_for_bitrate(bitrate).configured(ConfiguredInterfacePolicy::default())
}

pub fn descriptor(id: InterfaceId, policy: EffectiveInterfacePolicy) -> InterfaceDescriptor {
    policy.descriptor(id)
}
