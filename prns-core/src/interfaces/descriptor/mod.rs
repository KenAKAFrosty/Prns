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
///
/// The tier boundaries are inclusive, matching RNS 1.5.1 and later. RNS 1.5.0 and earlier compared
/// with `>` rather than `>=`, so a bitrate sitting exactly on a boundary resolved one tier lower.
/// That distinction is not academic: 10 Mbps is `TCPInterface.BITRATE_GUESS`, the bitrate both
/// `TCPClientInterface` and `TCPServerInterface` assume when a config names none, so it is the most
/// common value this function ever sees.
pub const fn hardware_mtu_for_bitrate(bitrate_bps: u64) -> Option<usize> {
    match bitrate_bps {
        1_000_000_000.. => Some(524_288),
        750_000_000.. => Some(262_144),
        400_000_000.. => Some(131_072),
        200_000_000.. => Some(65_536),
        100_000_000.. => Some(32_768),
        10_000_000.. => Some(16_384),
        5_000_000.. => Some(8_192),
        2_000_000.. => Some(4_096),
        1_000_000.. => Some(2_048),
        62_500.. => Some(1_024),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::hardware_mtu_for_bitrate;

    /// Pin every tier edge, because the edges are the whole behaviour and nothing else asserted them.
    ///
    /// Each boundary is checked from three sides: one below, exactly on it, and one above. The
    /// "exactly on it" column is what moved between RNS 1.5.0 and 1.5.1, and 10 Mbps is the one users
    /// actually hit, since it is what `TCPInterface` assumes when a config names no bitrate.
    #[test]
    fn tier_boundaries_are_inclusive_like_rns_1_5_1() {
        let cases: &[(u64, Option<usize>)] = &[
            (0, None),
            (62_499, None),
            (62_500, Some(1_024)),
            (62_501, Some(1_024)),
            (999_999, Some(1_024)),
            (1_000_000, Some(2_048)),
            (1_999_999, Some(2_048)),
            (2_000_000, Some(4_096)),
            (4_999_999, Some(4_096)),
            (5_000_000, Some(8_192)),
            (9_999_999, Some(8_192)),
            (10_000_000, Some(16_384)),
            (10_000_001, Some(16_384)),
            (99_999_999, Some(16_384)),
            (100_000_000, Some(32_768)),
            (199_999_999, Some(32_768)),
            (200_000_000, Some(65_536)),
            (399_999_999, Some(65_536)),
            (400_000_000, Some(131_072)),
            (749_999_999, Some(131_072)),
            (750_000_000, Some(262_144)),
            (999_999_999, Some(262_144)),
            (1_000_000_000, Some(524_288)),
            (u64::MAX, Some(524_288)),
        ];
        for &(bitrate, expected) in cases {
            assert_eq!(
                hardware_mtu_for_bitrate(bitrate),
                expected,
                "bitrate {bitrate} resolved the wrong hardware MTU tier"
            );
        }
    }

    /// The default TCP bitrate is the case that diverged, so it gets its own named test.
    #[test]
    fn the_default_tcp_bitrate_resolves_the_tier_rns_now_resolves() {
        assert_eq!(hardware_mtu_for_bitrate(10_000_000), Some(16_384));
    }
}
