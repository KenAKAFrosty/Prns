mod attached;
mod bitrate;

#[cfg(feature = "alloc")]
pub use attached::IndexedAttachedInterfaces;
pub use attached::{AttachedInterfaces, Egress};
pub use bitrate::BitrateBps;

use crate::interfaces::{
    AirtimeDutyCycle, AnnounceBandwidthCap, AnnounceRateLimit, InterfaceCapabilities,
    InterfaceCommonPolicy, InterfaceGravity, InterfaceId, InterfaceMode,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceDescriptor {
    pub id: InterfaceId,
    pub capabilities: InterfaceCapabilities,
    pub mode: InterfaceMode,
    pub gravity: InterfaceGravity,
    pub bitrate: BitrateBps,
    pub hardware_mtu: Option<usize>,
    pub announce_rate_limit: Option<AnnounceRateLimit>,
    pub announce_bandwidth_cap: AnnounceBandwidthCap,
    pub airtime_duty_cycle: Option<AirtimeDutyCycle>,
    pub common: InterfaceCommonPolicy,
}

/// RNS `Interface.optimise_mtu`; link negotiation clamps the result to the engine's `MAX_LINK_MTU`.
pub const fn hardware_mtu_for_bitrate(bitrate_bps: u64) -> Option<usize> {
    if bitrate_bps >= 1_000_000_000 {
        Some(524_288)
    } else if bitrate_bps >= 750_000_000 {
        Some(262_144)
    } else if bitrate_bps >= 400_000_000 {
        Some(131_072)
    } else if bitrate_bps >= 200_000_000 {
        Some(65_536)
    } else if bitrate_bps >= 100_000_000 {
        Some(32_768)
    } else if bitrate_bps >= 10_000_000 {
        Some(16_384)
    } else if bitrate_bps >= 5_000_000 {
        Some(8_192)
    } else if bitrate_bps >= 2_000_000 {
        Some(4_096)
    } else if bitrate_bps >= 1_000_000 {
        Some(2_048)
    } else if bitrate_bps >= 62_500 {
        Some(1_024)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::hardware_mtu_for_bitrate;

    #[test]
    fn rns_mtu_tiers_include_every_exact_threshold() {
        assert_eq!(hardware_mtu_for_bitrate(1_000_000_000), Some(524_288));
        assert_eq!(hardware_mtu_for_bitrate(750_000_000), Some(262_144));
        assert_eq!(hardware_mtu_for_bitrate(400_000_000), Some(131_072));
        assert_eq!(hardware_mtu_for_bitrate(200_000_000), Some(65_536));
        assert_eq!(hardware_mtu_for_bitrate(100_000_000), Some(32_768));
        assert_eq!(hardware_mtu_for_bitrate(10_000_000), Some(16_384));
        assert_eq!(hardware_mtu_for_bitrate(5_000_000), Some(8_192));
        assert_eq!(hardware_mtu_for_bitrate(2_000_000), Some(4_096));
        assert_eq!(hardware_mtu_for_bitrate(1_000_000), Some(2_048));
        assert_eq!(hardware_mtu_for_bitrate(62_500), Some(1_024));
        assert_eq!(hardware_mtu_for_bitrate(62_499), None);
    }
}
