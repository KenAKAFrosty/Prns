use crate::interfaces::{InterfaceCapabilities, InterfaceId, InterfaceMode};

/// The static configuration of an interface — how it *is*, not how it is doing. Live state
/// (connection, traffic) lives separately, owned by the interface and read by the app through
/// its status handle; this carries only the facts the engine needs to make routing decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceConfig {
    pub id: InterfaceId,
    pub capabilities: InterfaceCapabilities,
    pub mode: InterfaceMode,
    pub announce_rate_limit: Option<AnnounceRateLimit>,
    pub announce_bandwidth_cap: AnnounceBandwidthCap,
}

/// Per-interface announce rebroadcast rate policy — RNS 1.3.1's
/// `announce_rate_target`/`announce_rate_grace`/`announce_rate_penalty`
/// (seconds widened to milliseconds here). `None` leaves rate limiting off,
/// the reference default for an interface that never sets a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnnounceRateLimit {
    pub target_ms: u64,
    pub grace: u16,
    pub penalty_ms: u64,
}

/// Per-interface announce egress pacing — RNS 1.3.1's `Interface.announce_cap`
/// against the interface's bitrate. Where [`AnnounceRateLimit`] punishes one
/// chatty destination, this paces all announce egress on a link: a burst is
/// spread over time, never dropped. `Unlimited` is for links with no real
/// bandwidth to protect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceBandwidthCap {
    Unlimited,
    Limited {
        bitrate_bps: u32,
        cap_per_mille: u16,
    },
}

impl AnnounceBandwidthCap {
    pub const RNS_DEFAULT_CAP_PER_MILLE: u16 = 20;

    /// The interface's declared bitrate at RNS's standard 2% cap — the
    /// self-aware default each interface proffers.
    pub const fn from_bitrate(bitrate_bps: u32) -> Self {
        Self::Limited {
            bitrate_bps,
            cap_per_mille: Self::RNS_DEFAULT_CAP_PER_MILLE,
        }
    }

    /// Minimum spacing after an announce of `announce_bytes` before the next may
    /// leave this interface. `Unlimited`, a zero bitrate, or a zero cap all fail open.
    pub fn spacing_ms(&self, announce_bytes: usize) -> u64 {
        match *self {
            Self::Unlimited => 0,
            Self::Limited {
                bitrate_bps,
                cap_per_mille,
            } => {
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
    fn unlimited_never_spaces() {
        assert_eq!(
            AnnounceBandwidthCap::Unlimited.spacing_ms(TYPICAL_ANNOUNCE_BYTES),
            0
        );
    }

    #[test]
    fn spacing_matches_the_rns_wait_time_formula() {
        let cap = AnnounceBandwidthCap::Limited {
            bitrate_bps: 5_000,
            cap_per_mille: 20,
        };
        assert_eq!(cap.spacing_ms(167), 13_360);
    }

    #[test]
    fn a_zero_bitrate_or_cap_fails_open() {
        assert_eq!(
            AnnounceBandwidthCap::Limited {
                bitrate_bps: 0,
                cap_per_mille: 20,
            }
            .spacing_ms(167),
            0
        );
        assert_eq!(
            AnnounceBandwidthCap::Limited {
                bitrate_bps: 5_000,
                cap_per_mille: 0,
            }
            .spacing_ms(167),
            0
        );
    }

    #[test]
    fn faster_links_space_less() {
        let slow = AnnounceBandwidthCap::Limited {
            bitrate_bps: 5_000,
            cap_per_mille: 20,
        };
        let fast = AnnounceBandwidthCap::Limited {
            bitrate_bps: 1_000_000,
            cap_per_mille: 20,
        };
        assert!(fast.spacing_ms(167) < slow.spacing_ms(167));
    }

    #[test]
    fn from_bitrate_applies_the_standard_cap() {
        assert_eq!(
            AnnounceBandwidthCap::from_bitrate(1_000_000),
            AnnounceBandwidthCap::Limited {
                bitrate_bps: 1_000_000,
                cap_per_mille: AnnounceBandwidthCap::RNS_DEFAULT_CAP_PER_MILLE,
            }
        );
    }

    #[test]
    fn a_conservative_megabit_throttles_an_order_harder_than_the_rns_lan_guess() {
        let rns_guess = AnnounceBandwidthCap::from_bitrate(10_000_000);
        let ours = AnnounceBandwidthCap::from_bitrate(1_000_000);
        assert!(ours.spacing_ms(167) >= rns_guess.spacing_ms(167) * 9);
    }
}
