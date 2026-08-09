use crate::interfaces::BitrateBps;

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
    pub recursive_path_requests: RecursivePathRequestPolicy,
    pub announces_from_internal: bool,
    pub announces_to_internal: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecursivePathRequestPolicy {
    InheritNode,
    Enabled,
    Disabled,
}

impl RecursivePathRequestPolicy {
    pub const fn from_configured(enabled: bool) -> Self {
        if enabled {
            Self::Enabled
        } else {
            Self::Disabled
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IngressControlPolicy {
    pub enabled: bool,
    pub max_held_announces: usize,
    pub new_interface_millis: u64,
    pub announce_burst_frequency_new: FrequencyMilliHertz,
    pub announce_burst_frequency: FrequencyMilliHertz,
    pub path_request_burst_frequency_new: FrequencyMilliHertz,
    pub path_request_burst_frequency: FrequencyMilliHertz,
    pub burst_hold_millis: u64,
    pub burst_penalty_millis: u64,
    pub held_release_interval_millis: u64,
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
            recursive_path_requests: RecursivePathRequestPolicy::InheritNode,
            announces_from_internal: true,
            announces_to_internal: false,
        },
        ingress_control: IngressControlPolicy {
            enabled: true,
            max_held_announces: 256,
            new_interface_millis: 2 * 60 * 60 * 1_000,
            announce_burst_frequency_new: FrequencyMilliHertz::new(3_000),
            announce_burst_frequency: FrequencyMilliHertz::new(10_000),
            path_request_burst_frequency_new: FrequencyMilliHertz::new(3_000),
            path_request_burst_frequency: FrequencyMilliHertz::new(8_000),
            burst_hold_millis: 15_000,
            burst_penalty_millis: 15_000,
            held_release_interval_millis: 5_000,
        },
        path_request_egress: PathRequestEgressControl {
            enabled: false,
            frequency: FrequencyMilliHertz::new(5_000),
        },
    };
}

/// RNS 1.4.2 RNodeInterface `airtime_limit_short` and `airtime_limit_long`, enforced host-side instead of by radio firmware.
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

    /// Whether a projected utilization remains at or below every configured
    /// limit. Callers use this before transmitting so one long frame cannot
    /// overshoot an otherwise clear ledger.
    pub fn permits(&self, projected: crate::interfaces::AirtimeUtilization) -> bool {
        self.limit_short_per_mille
            .is_none_or(|limit| projected.short_per_mille <= limit)
            && self
                .limit_long_per_mille
                .is_none_or(|limit| projected.long_per_mille <= limit)
    }
}

/// RNS 1.4.2 `announce_rate_target`, `announce_rate_grace`, and `announce_rate_penalty`, with seconds widened to milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnnounceRateLimit {
    pub target_ms: u64,
    pub grace: u16,
    pub penalty_ms: u64,
}

/// RNS 1.4.2 `Interface.announce_cap` paces all announce egress on a link; [`AnnounceRateLimit`] applies to one destination.
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
