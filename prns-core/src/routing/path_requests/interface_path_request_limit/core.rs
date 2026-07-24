use crate::engine::InstantMillis;
use crate::interfaces::{
    IngressControlPolicy, InterfaceCommonPolicy, InterfaceId, PathRequestEgressControl,
};

/// RNS 1.4.0 `Interface.IC_PR_BURST_FREQ_NEW` (3 Hz, an interface younger than [`NEW_INTERFACE_AGE_MS`])
pub const STRICT_RATE_LIMIT_HZ: u64 = 3;
/// RNS 1.4.0 `Interface.IC_PR_BURST_FREQ` (8 Hz, an established interface)
pub const RELAXED_RATE_LIMIT_HZ: u64 = 8;
/// RNS 1.4.0 `Interface.IC_NEW_TIME` (2 hours)
pub const NEW_INTERFACE_AGE_MS: u64 = 2 * 60 * 60 * 1_000;
/// RNS 1.4.0 `Interface.IC_BURST_HOLD` (15 seconds): the minimum a burst stays latched
pub const BURST_HOLD_MS: u64 = 15 * 1_000;
/// RNS 1.4.0 `Interface.PR_FREQ_DECAY` (10 seconds): the rate-measurement window
pub const FREQUENCY_WINDOW_MS: u64 = 10 * 1_000;
/// RNS 1.4.0 `Interface.IC_DEQUE_MIN_SAMPLE` + 1: samples needed before a rate is judged
pub const MIN_SAMPLES_TO_JUDGE: u16 = 3;
const MIN_EGRESS_SAMPLES_TO_JUDGE: u16 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BurstState {
    Calm,
    Bursting { since: InstantMillis },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfacePathRequestLimit {
    pub interface: InterfaceId,
    pub created_at: InstantMillis,
    pub window_start: InstantMillis,
    pub window_count: u16,
    pub burst: BurstState,
}

pub trait InterfacePathRequestLimitTable {
    fn capacity(&self) -> usize;
    fn rows(&self) -> &[InterfacePathRequestLimit];
    fn rows_mut(&mut self) -> &mut [InterfacePathRequestLimit];
    fn push(&mut self, row: InterfacePathRequestLimit);
    fn swap_remove(&mut self, index: usize);
}

#[derive(Debug, Default)]
pub struct InterfacePathRequestLimits<C: InterfacePathRequestLimitTable> {
    table: C,
}

impl<C: InterfacePathRequestLimitTable> InterfacePathRequestLimits<C> {
    pub fn should_egress_limit(
        &self,
        interface: InterfaceId,
        now: InstantMillis,
        policy: PathRequestEgressControl,
    ) -> bool {
        if !policy.enabled {
            return false;
        }
        self.table
            .rows()
            .iter()
            .find(|row| row.interface == interface)
            .is_some_and(|row| {
                let elapsed_ms = now.0.saturating_sub(row.window_start.0);
                elapsed_ms != 0
                    && row.window_count >= MIN_EGRESS_SAMPLES_TO_JUDGE
                    && u128::from(row.window_count) * 1_000_000
                        > u128::from(policy.frequency.get()) * u128::from(elapsed_ms)
            })
    }

    pub fn record_egress(&mut self, interface: InterfaceId, now: InstantMillis) {
        let index = self.index_or_insert(interface, now);
        let row = &mut self.table.rows_mut()[index];
        if now.0.saturating_sub(row.window_start.0) >= FREQUENCY_WINDOW_MS {
            row.window_start = now;
            row.window_count = 1;
        } else {
            row.window_count = row.window_count.saturating_add(1);
        }
    }

    /// Record a path request on `interface` and report whether to drop its recursive discovery forward. This implements RNS 1.4.0 `Interface.should_ingress_limit_pr`, replacing the 48-sample sliding deque with an integer fixed window of the same span.
    pub fn record_and_should_limit(&mut self, interface: InterfaceId, now: InstantMillis) -> bool {
        self.record_and_should_limit_with_policy(
            interface,
            now,
            InterfaceCommonPolicy::RNS_DEFAULT.ingress_control,
        )
    }

    pub fn record_and_should_limit_with_policy(
        &mut self,
        interface: InterfaceId,
        now: InstantMillis,
        policy: IngressControlPolicy,
    ) -> bool {
        if !policy.enabled {
            return false;
        }
        let index = self.index_or_insert(interface, now);
        let row = &mut self.table.rows_mut()[index];

        if now.0.saturating_sub(row.window_start.0) >= FREQUENCY_WINDOW_MS {
            row.window_start = now;
            row.window_count = 1;
        } else {
            row.window_count = row.window_count.saturating_add(1);
        }

        let threshold = if now.0.saturating_sub(row.created_at.0) < policy.new_interface_millis {
            policy.path_request_burst_frequency_new.get()
        } else {
            policy.path_request_burst_frequency.get()
        };
        let elapsed_ms = now.0.saturating_sub(row.window_start.0);
        let over_threshold = elapsed_ms != 0
            && row.window_count >= MIN_SAMPLES_TO_JUDGE
            && u128::from(row.window_count) * 1_000_000
                > u128::from(threshold) * u128::from(elapsed_ms);

        match row.burst {
            BurstState::Bursting { since } => {
                if !over_threshold && now.0 >= since.0.saturating_add(policy.burst_hold_millis) {
                    row.burst = BurstState::Calm;
                }
                true
            }
            BurstState::Calm => {
                if over_threshold {
                    row.burst = BurstState::Bursting { since: now };
                    true
                } else {
                    false
                }
            }
        }
    }

    pub fn len(&self) -> usize {
        self.table.rows().len()
    }

    pub fn is_empty(&self) -> bool {
        self.table.rows().is_empty()
    }

    fn index_or_insert(&mut self, interface: InterfaceId, now: InstantMillis) -> usize {
        if let Some(index) = self
            .table
            .rows()
            .iter()
            .position(|row| row.interface == interface)
        {
            return index;
        }
        if self.table.rows().len() >= self.table.capacity() {
            self.evict_least_recent();
        }
        self.table.push(InterfacePathRequestLimit {
            interface,
            created_at: now,
            window_start: now,
            window_count: 0,
            burst: BurstState::Calm,
        });
        self.table.rows().len() - 1
    }

    fn evict_least_recent(&mut self) {
        if let Some(index) = self
            .table
            .rows()
            .iter()
            .enumerate()
            .min_by_key(|(_, row)| row.window_start.0)
            .map(|(index, _)| index)
        {
            self.table.swap_remove(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;

    fn iface(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 8])
    }

    fn limits() -> InterfacePathRequestLimits<FixedInterfacePathRequestLimitTable<4>> {
        InterfacePathRequestLimits::default()
    }

    #[test]
    fn a_calm_trickle_is_never_limited() {
        let mut limits = limits();
        for second in 0..20u64 {
            assert!(
                !limits.record_and_should_limit(iface(1), InstantMillis(second * 1_000)),
                "one request per second stays under even the strict 3 Hz limit",
            );
        }
    }

    #[test]
    fn a_flood_latches_a_burst_that_outlasts_the_traffic() {
        let mut limits = limits();
        let mut limited_any = false;
        for i in 0..12u64 {
            limited_any |= limits.record_and_should_limit(iface(1), InstantMillis(i * 8));
        }
        assert!(
            limited_any,
            "twelve requests in 100 ms is 120 Hz — the flood trips the limiter"
        );
        assert!(
            limits.record_and_should_limit(iface(1), InstantMillis(200)),
            "still latched a moment later though no new traffic cleared it",
        );
    }

    #[test]
    fn the_first_two_samples_are_never_judged() {
        let mut limits = limits();
        assert!(
            !limits.record_and_should_limit(iface(1), InstantMillis(0)),
            "two simultaneous requests are below the minimum sample count and cannot be rated",
        );
        assert!(!limits.record_and_should_limit(iface(1), InstantMillis(0)));
        assert!(!limits.record_and_should_limit(iface(1), InstantMillis(0)));
    }

    #[test]
    fn a_burst_clears_after_the_hold_once_traffic_calms() {
        let mut limits = limits();
        for i in 0..12u64 {
            limits.record_and_should_limit(iface(1), InstantMillis(i * 8));
        }

        assert!(
            limits.record_and_should_limit(iface(1), InstantMillis(BURST_HOLD_MS - 1)),
            "a lone request before the hold elapses still limits and keeps the latch",
        );
        assert!(
            limits.record_and_should_limit(iface(1), InstantMillis(2 * BURST_HOLD_MS)),
            "the calmed-and-held call still limits per RNS, de-latching for the next",
        );
        assert!(!limits.record_and_should_limit(iface(1), InstantMillis(3 * BURST_HOLD_MS)));
    }

    #[test]
    fn a_new_interface_trips_where_an_established_one_would_not() {
        let mut young = limits();
        let mut young_limited = false;
        for i in 0..6u64 {
            young_limited |= young.record_and_should_limit(iface(1), InstantMillis(i * 200));
        }
        assert!(
            young_limited,
            "5 Hz from a fresh interface's first request trips the strict 3 Hz limit",
        );

        let mut old = limits();
        assert!(!old.record_and_should_limit(iface(2), InstantMillis(0)));
        let start = NEW_INTERFACE_AGE_MS + 1;
        let mut old_limited = false;
        for i in 0..6u64 {
            old_limited |= old.record_and_should_limit(iface(2), InstantMillis(start + i * 200));
        }
        assert!(
            !old_limited,
            "the same 5 Hz on an interface first seen over 2h ago stays under the relaxed 8 Hz",
        );
    }

    #[test]
    fn interfaces_are_tracked_independently() {
        let mut limits = limits();
        for i in 0..12u64 {
            limits.record_and_should_limit(iface(1), InstantMillis(i * 8));
        }
        assert!(
            !limits.record_and_should_limit(iface(2), InstantMillis(0)),
            "a flood on one interface does not limit a quiet neighbor",
        );
    }

    #[test]
    fn egress_control_starts_limiting_after_the_minimum_sample_count() {
        let mut limits = limits();
        let policy = PathRequestEgressControl {
            enabled: true,
            frequency: crate::interfaces::FrequencyMilliHertz::new(5_000),
        };
        for now in 0..6 {
            assert!(!limits.should_egress_limit(iface(1), InstantMillis(now), policy));
            limits.record_egress(iface(1), InstantMillis(now));
        }
        assert!(limits.should_egress_limit(iface(1), InstantMillis(6), policy));
    }

    #[test]
    fn a_zero_span_never_reports_an_egress_frequency() {
        let mut limits = limits();
        let policy = PathRequestEgressControl {
            enabled: true,
            frequency: crate::interfaces::FrequencyMilliHertz::new(0),
        };
        for _ in 0..MIN_EGRESS_SAMPLES_TO_JUDGE {
            limits.record_egress(iface(1), InstantMillis(0));
        }
        assert!(!limits.should_egress_limit(iface(1), InstantMillis(0), policy));
    }
}
