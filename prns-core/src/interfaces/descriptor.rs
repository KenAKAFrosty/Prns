use crate::interfaces::{BitrateBps, InterfaceCapabilities, InterfaceId, InterfaceMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct FrequencyMilliHertz(u64);

impl FrequencyMilliHertz {
    pub const fn new(milli_hertz: u64) -> Self {
        Self(milli_hertz)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceForwardingPolicy {
    pub recursive_path_requests: bool,
    pub announces_from_internal: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IngressControlPolicy {
    pub enabled: bool,
    pub max_held_announces: usize,
    pub new_interface_ms: u64,
    pub announce_burst_frequency_new: FrequencyMilliHertz,
    pub announce_burst_frequency: FrequencyMilliHertz,
    pub path_request_burst_frequency_new: FrequencyMilliHertz,
    pub path_request_burst_frequency: FrequencyMilliHertz,
    pub burst_hold_ms: u64,
    pub burst_penalty_ms: u64,
    pub held_release_interval_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathRequestEgressControl {
    pub enabled: bool,
    pub frequency: FrequencyMilliHertz,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceCommonPolicy {
    pub forwarding: InterfaceForwardingPolicy,
    pub ingress_control: IngressControlPolicy,
    pub path_request_egress: PathRequestEgressControl,
}

impl InterfaceCommonPolicy {
    pub const RNS_DEFAULT: Self = Self {
        forwarding: InterfaceForwardingPolicy {
            recursive_path_requests: false,
            announces_from_internal: true,
        },
        ingress_control: IngressControlPolicy {
            enabled: true,
            max_held_announces: 256,
            new_interface_ms: 2 * 60 * 60 * 1_000,
            announce_burst_frequency_new: FrequencyMilliHertz::new(3_000),
            announce_burst_frequency: FrequencyMilliHertz::new(10_000),
            path_request_burst_frequency_new: FrequencyMilliHertz::new(3_000),
            path_request_burst_frequency: FrequencyMilliHertz::new(8_000),
            burst_hold_ms: 15_000,
            burst_penalty_ms: 15_000,
            held_release_interval_ms: 5_000,
        },
        path_request_egress: PathRequestEgressControl {
            enabled: false,
            frequency: FrequencyMilliHertz::new(5_000),
        },
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceDescriptor {
    pub id: InterfaceId,
    pub capabilities: InterfaceCapabilities,
    pub mode: InterfaceMode,
    pub bitrate: BitrateBps,
    pub hardware_mtu: Option<usize>,
    pub announce_rate_limit: Option<AnnounceRateLimit>,
    pub announce_bandwidth_cap: AnnounceBandwidthCap,
    pub airtime_duty_cycle: Option<AirtimeDutyCycle>,
    pub common: InterfaceCommonPolicy,
}

/// RNS 1.3.5 `Interface.optimise_mtu`: the hardware MTU an interface declares
/// for its bitrate tier. What a link actually negotiates is this clamped by
/// the engine's `MAX_LINK_MTU`.
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

/// RNS 1.3.5 RNodeInterface `airtime_limit_short`/`airtime_limit_long`, enforced
/// host-side instead of by radio firmware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AirtimeDutyCycle {
    pub limit_short_per_mille: Option<u16>,
    pub limit_long_per_mille: Option<u16>,
    pub max_queued_airtime_ms: u32,
}

impl AirtimeDutyCycle {
    pub fn exceeded_by(&self, utilization: crate::interfaces::AirtimeUtilization) -> bool {
        self.limit_short_per_mille
            .is_some_and(|limit| utilization.short_per_mille >= limit)
            || self
                .limit_long_per_mille
                .is_some_and(|limit| utilization.long_per_mille >= limit)
    }
}

/// Per-interface announce rebroadcast rate policy; RNS 1.3.5's
/// `announce_rate_target`/`announce_rate_grace`/`announce_rate_penalty`
/// (seconds widened to milliseconds here). `None` leaves rate limiting off,
/// which is also the reference default for an interface that never sets a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnnounceRateLimit {
    pub target_ms: u64,
    pub grace: u16,
    pub penalty_ms: u64,
}

/// Per-interface announce egress pacing (RNS 1.3.5's `Interface.announce_cap`)
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

    pub const fn blocks_all(self) -> bool {
        matches!(self, Self::Limited { cap_per_mille: 0 })
    }

    pub fn cooldown_after_send_ms(&self, bitrate: BitrateBps, announce_bytes: usize) -> u64 {
        match *self {
            Self::Unlimited => 0,
            Self::Limited { cap_per_mille } => {
                if cap_per_mille == 0 {
                    return u64::MAX;
                }
                (announce_bytes as u64).saturating_mul(8_000_000)
                    / (bitrate.get().saturating_mul(cap_per_mille as u64))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TYPICAL_ANNOUNCE_BYTES: usize = 167;

    fn bitrate(bps: u64) -> BitrateBps {
        BitrateBps::new(bps).expect("test bitrate above the floor")
    }

    #[test]
    fn unlimited_never_cools_down() {
        assert_eq!(
            AnnounceBandwidthCap::Unlimited
                .cooldown_after_send_ms(bitrate(5_000), TYPICAL_ANNOUNCE_BYTES),
            0
        );
    }

    #[test]
    fn cooldown_matches_the_rns_wait_time_formula() {
        assert_eq!(
            AnnounceBandwidthCap::RNS_DEFAULT.cooldown_after_send_ms(bitrate(5_000), 167),
            13_360
        );
    }

    #[test]
    fn a_zero_cap_blocks_further_announce_bandwidth() {
        assert_eq!(
            AnnounceBandwidthCap::Limited { cap_per_mille: 0 }
                .cooldown_after_send_ms(bitrate(5_000), 167),
            u64::MAX
        );
    }

    #[test]
    fn faster_links_cool_down_less() {
        let cap = AnnounceBandwidthCap::RNS_DEFAULT;
        assert!(
            cap.cooldown_after_send_ms(bitrate(1_000_000), 167)
                < cap.cooldown_after_send_ms(bitrate(5_000), 167)
        );
    }

    #[test]
    fn a_conservative_megabit_throttles_an_order_harder_than_the_rns_lan_guess() {
        let cap = AnnounceBandwidthCap::RNS_DEFAULT;
        assert!(
            cap.cooldown_after_send_ms(bitrate(1_000_000), 167)
                >= cap.cooldown_after_send_ms(bitrate(10_000_000), 167) * 9
        );
    }
}
