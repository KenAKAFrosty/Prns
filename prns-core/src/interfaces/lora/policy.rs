use crate::interfaces::{
    AirtimeDutyCycle, AnnounceBandwidthCap, BitrateBps, ConfiguredInterfacePolicy,
    EgressCapability, IngressCapability, InterfaceCapabilities, InterfaceDefaults,
    InterfaceDescriptor, InterfaceId, InterfaceMode, MtuPolicy, TransportCapability,
};

use super::framing::LORA_MAX_PAYLOAD;
use super::profile::RadioProfile;

pub fn descriptor(
    id: InterfaceId,
    profile: &RadioProfile,
    airtime_duty_cycle: Option<AirtimeDutyCycle>,
) -> InterfaceDescriptor {
    defaults(profile, airtime_duty_cycle)
        .configured(ConfiguredInterfacePolicy::default())
        .descriptor(id)
}

pub fn defaults(
    profile: &RadioProfile,
    airtime_duty_cycle: Option<AirtimeDutyCycle>,
) -> InterfaceDefaults {
    InterfaceDefaults {
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransportCapability::SameInterfaceRepeat),
        },
        mode: InterfaceMode::Full,
        bitrate: BitrateBps::guess(u64::from(profile.nominal_bitrate_bps())),
        mtu: MtuPolicy::fixed(LORA_MAX_PAYLOAD),
        announce_rate_limit: None,
        announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
        airtime_duty_cycle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::lora::{Region, DEFAULT_915_PROFILE};
    use crate::interfaces::INTERFACE_ID_LEN;

    #[test]
    fn descriptor_is_a_repeating_shared_half_duplex_interface() {
        let d = descriptor(
            InterfaceId::new([0x5C; INTERFACE_ID_LEN]),
            &DEFAULT_915_PROFILE,
            None,
        );
        assert!(matches!(d.mode, InterfaceMode::Full));
        assert_eq!(d.capabilities.ingress, IngressCapability::Enabled);
        assert_eq!(
            d.capabilities.egress,
            EgressCapability::Enabled(TransportCapability::SameInterfaceRepeat)
        );
        assert_eq!(d.hardware_mtu, Some(LORA_MAX_PAYLOAD));
        assert_eq!(
            d.bitrate,
            BitrateBps::new(u64::from(DEFAULT_915_PROFILE.nominal_bitrate_bps())).unwrap()
        );
        assert_eq!(d.announce_bandwidth_cap, AnnounceBandwidthCap::RNS_DEFAULT);
    }

    #[test]
    fn descriptor_uses_the_supplied_duty_cycle() {
        let id = InterfaceId::new([0x5C; INTERFACE_ID_LEN]);
        let eu_preset = Region::Eu868.regulatory_duty_cycle();
        let d = descriptor(id, &DEFAULT_915_PROFILE, eu_preset);
        assert_eq!(d.airtime_duty_cycle, eu_preset);
        let none = descriptor(id, &DEFAULT_915_PROFILE, None);
        assert_eq!(none.airtime_duty_cycle, None);
    }
}
