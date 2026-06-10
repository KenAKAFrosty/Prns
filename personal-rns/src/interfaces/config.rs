use crate::interfaces::{InterfaceCapabilities, InterfaceId, InterfaceMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceConfig {
    pub id: InterfaceId,
    pub capabilities: InterfaceCapabilities,
    pub mode: InterfaceMode,
    pub bitrate_bps: Option<u32>,
    pub announce_rate_limit: Option<AnnounceRateLimit>,
    pub announce_bandwidth_cap: AnnounceBandwidthCap,
}

/// Per-interface announce rebroadcast rate policy; RNS 1.3.1's
/// `announce_rate_target`/`announce_rate_grace`/`announce_rate_penalty`
/// (seconds widened to milliseconds here). `None` leaves rate limiting off,
/// which is also the reference default for an interface that never sets a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnnounceRateLimit {
    pub target_ms: u64,
    pub grace: u16,
    pub penalty_ms: u64,
}

/// Per-interface announce egress pacing (RNS 1.3.1's `Interface.announce_cap`)
/// against the interface's bitrate. Where [`AnnounceRateLimit`] punishes one
/// chatty destination, this paces all announce egress on a link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceBandwidthCap {
    Unlimited,
    Limited { cap_per_mille: u16 },
}

impl AnnounceBandwidthCap {
    pub const RNS_DEFAULT_CAP_PER_MILLE: u16 = 20;

    pub const RNS_DEFAULT: Self = Self::Limited {
        cap_per_mille: Self::RNS_DEFAULT_CAP_PER_MILLE,
    };

    pub fn cooldown_after_send_ms(&self, bitrate_bps: Option<u32>, announce_bytes: usize) -> u64 {
        match *self {
            Self::Unlimited => 0,
            Self::Limited { cap_per_mille } => {
                let Some(bitrate_bps) = bitrate_bps else {
                    return 0;
                };
                if bitrate_bps == 0 || cap_per_mille == 0 {
                    return 0;
                }
                (announce_bytes as u64).saturating_mul(8_000_000)
                    / ((bitrate_bps as u64).saturating_mul(cap_per_mille as u64))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TYPICAL_ANNOUNCE_BYTES: usize = 167;

    #[test]
    fn unlimited_never_cools_down() {
        assert_eq!(
            AnnounceBandwidthCap::Unlimited
                .cooldown_after_send_ms(Some(5_000), TYPICAL_ANNOUNCE_BYTES),
            0
        );
    }

    #[test]
    fn cooldown_matches_the_rns_wait_time_formula() {
        assert_eq!(
            AnnounceBandwidthCap::RNS_DEFAULT.cooldown_after_send_ms(Some(5_000), 167),
            13_360
        );
    }

    #[test]
    fn an_undeclared_zero_bitrate_or_zero_cap_fails_open() {
        assert_eq!(
            AnnounceBandwidthCap::RNS_DEFAULT.cooldown_after_send_ms(None, 167),
            0
        );
        assert_eq!(
            AnnounceBandwidthCap::RNS_DEFAULT.cooldown_after_send_ms(Some(0), 167),
            0
        );
        assert_eq!(
            AnnounceBandwidthCap::Limited { cap_per_mille: 0 }
                .cooldown_after_send_ms(Some(5_000), 167),
            0
        );
    }

    #[test]
    fn faster_links_cool_down_less() {
        let cap = AnnounceBandwidthCap::RNS_DEFAULT;
        assert!(
            cap.cooldown_after_send_ms(Some(1_000_000), 167)
                < cap.cooldown_after_send_ms(Some(5_000), 167)
        );
    }

    #[test]
    fn a_conservative_megabit_throttles_an_order_harder_than_the_rns_lan_guess() {
        let cap = AnnounceBandwidthCap::RNS_DEFAULT;
        assert!(
            cap.cooldown_after_send_ms(Some(1_000_000), 167)
                >= cap.cooldown_after_send_ms(Some(10_000_000), 167) * 9
        );
    }
}
