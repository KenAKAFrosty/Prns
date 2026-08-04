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

/// RNS 1.4.2 `Interface.optimise_mtu`; link negotiation clamps the result to the engine's `MAX_LINK_MTU`.
pub const fn hardware_mtu_for_bitrate(bitrate_bps: u64) -> Option<usize> {
    match bitrate_bps {
        1_000_000_000.. => Some(524_288),
        750_000_001.. => Some(262_144),
        400_000_001.. => Some(131_072),
        200_000_001.. => Some(65_536),
        100_000_001.. => Some(32_768),
        10_000_001.. => Some(16_384),
        5_000_001.. => Some(8_192),
        2_000_001.. => Some(4_096),
        1_000_001.. => Some(2_048),
        62_501.. => Some(1_024),
        _ => None,
    }
}
