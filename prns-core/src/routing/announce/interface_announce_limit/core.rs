use core::cmp::Ordering;

use crate::engine::InstantMillis;
use crate::interfaces::{IngressControlPolicy, InterfaceCommonPolicy, InterfaceId};

/// RNS `Interface.IC_NEW_TIME` (2 hours)
pub const NEW_INTERFACE_AGE_MS: u64 = 2 * 60 * 60 * 1_000;
/// RNS `Interface.IC_BURST_FREQ_NEW` (3 Hz, an interface younger than [`NEW_INTERFACE_AGE_MS`])
pub const STRICT_RATE_LIMIT_HZ: u64 = 3;
/// RNS `Interface.IC_BURST_FREQ` (10 Hz, for an established interface)
pub const RELAXED_RATE_LIMIT_HZ: u64 = 10;
/// RNS `Interface.IC_BURST_HOLD` (15 seconds): the minimum a burst stays latched
pub const BURST_HOLD_MS: u64 = 15 * 1_000;
/// RNS `Interface.IC_BURST_MIN_SAMPLES` (6): a latched burst may not clear until at least this many samples back the calm reading
pub const BURST_CLEAR_MIN_SAMPLES: u16 = 6;
/// Our tumbling rate-measurement window, sized to RNS `Interface.AR_FREQ_DECAY` (10 seconds), the reference's lazy sample-decay horizon
pub const FREQUENCY_WINDOW_MS: u64 = 10 * 1_000;
/// RNS `Interface.IC_DEQUE_MIN_SAMPLE` + 1: fewer samples than this reads as zero frequency
pub const MIN_SAMPLES_TO_JUDGE: u16 = 3;
/// RNS `Interface.IC_BURST_PENALTY` (15 seconds): the wait after a burst latches before the first held announce may drip out
pub const BURST_PENALTY_MS: u64 = 15 * 1_000;
/// RNS `Interface.IC_HELD_RELEASE_INTERVAL` (5 seconds): the minimum spacing between drip-released announces
pub const HELD_RELEASE_MIN_INTERVAL_MS: u64 = 5 * 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BurstState {
    Calm,
    Bursting { since: InstantMillis },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceAnnounceLimit {
    pub interface: InterfaceId,
    pub created_at: InstantMillis,
    pub window_started_at: InstantMillis,
    pub window_count: u16,
    pub burst: BurstState,
    pub next_held_release_at: InstantMillis,
}

#[expect(
    clippy::enum_variant_names,
    reason = "the shared postfix is the point: every reading is relative to the one limit"
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RateReading {
    UnderLimit,
    AtLimit,
    OverLimit,
}

/// RNS `Interface.incoming_announce_frequency` against the age-keyed limit.
/// Too few samples or a zero span read as zero frequency (the reference returns 0 for both), so both read `UnderLimit`.
/// `AtLimit` neither latches nor releases: RNS compares strictly in both directions.
fn rate_reading(
    row: &InterfaceAnnounceLimit,
    now: InstantMillis,
    policy: IngressControlPolicy,
) -> RateReading {
    let limit = if now.0.saturating_sub(row.created_at.0) < policy.new_interface_millis {
        policy.announce_burst_frequency_new.get()
    } else {
        policy.announce_burst_frequency.get()
    };
    let elapsed_ms = now.0.saturating_sub(row.window_started_at.0);
    if row.window_count < MIN_SAMPLES_TO_JUDGE || elapsed_ms == 0 {
        return RateReading::UnderLimit;
    }
    match (u128::from(row.window_count) * 1_000_000)
        .cmp(&(u128::from(limit) * u128::from(elapsed_ms)))
    {
        Ordering::Less => RateReading::UnderLimit,
        Ordering::Equal => RateReading::AtLimit,
        Ordering::Greater => RateReading::OverLimit,
    }
}

pub trait InterfaceAnnounceLimitTable {
    fn capacity(&self) -> usize;
    fn rows(&self) -> &[InterfaceAnnounceLimit];
    fn rows_mut(&mut self) -> &mut [InterfaceAnnounceLimit];
    fn push(&mut self, row: InterfaceAnnounceLimit);
    fn swap_remove(&mut self, index: usize);
}

#[derive(Debug, Default)]
pub struct InterfaceAnnounceLimits<C: InterfaceAnnounceLimitTable> {
    table: C,
}

impl<C: InterfaceAnnounceLimitTable> InterfaceAnnounceLimits<C> {
    /// RNS 1.4.0 `Interface.received_announce`
    pub fn record(&mut self, interface: InterfaceId, now: InstantMillis) {
        let index = self.index_or_insert(interface, now);
        let row = &mut self.table.rows_mut()[index];
        if row.window_count == 0
            || now.0.saturating_sub(row.window_started_at.0) >= FREQUENCY_WINDOW_MS
        {
            row.window_started_at = now;
            row.window_count = 1;
        } else {
            row.window_count = row.window_count.saturating_add(1);
        }
    }

    /// Pins the interface's age clock: RNS thresholds key on `Interface.age()`, time since the interface object's creation, so hosts call this at attach.
    /// An interface never pinned starts its clock at the first recorded announce instead, and a returning interface keeps its original clock.
    pub fn interface_attached(&mut self, interface: InterfaceId, now: InstantMillis) {
        let _ = self.index_or_insert(interface, now);
    }

    /// RNS 1.4.0 `Interface.should_ingress_limit`: latch or clear the burst and report whether an unknown-destination announce arriving now should be held.
    /// [`Self::record`] runs before this for every announce, known or unknown destination, so the announce being judged already counts toward its own reading; only unknown destinations consult this judgment, so known-destination floods raise the rate without touching the latch. Both behaviors mirror the reference's call order.
    pub fn should_limit(&mut self, interface: InterfaceId, now: InstantMillis) -> bool {
        self.should_limit_with_policy(
            interface,
            now,
            InterfaceCommonPolicy::RNS_DEFAULT.ingress_control,
        )
    }

    pub fn should_limit_with_policy(
        &mut self,
        interface: InterfaceId,
        now: InstantMillis,
        policy: IngressControlPolicy,
    ) -> bool {
        if !policy.enabled {
            return false;
        }
        let Some(index) = self
            .table
            .rows()
            .iter()
            .position(|row| row.interface == interface)
        else {
            return false;
        };
        let row = &mut self.table.rows_mut()[index];
        let reading = rate_reading(row, now, policy);

        match row.burst {
            BurstState::Bursting { since } => {
                if reading == RateReading::UnderLimit
                    && now.0 >= since.0.saturating_add(policy.burst_hold_millis)
                    && row.window_count >= BURST_CLEAR_MIN_SAMPLES
                {
                    row.burst = BurstState::Calm;
                }
                true
            }
            BurstState::Calm => {
                if reading == RateReading::OverLimit {
                    row.burst = BurstState::Bursting { since: now };
                    row.next_held_release_at =
                        InstantMillis(now.0.saturating_add(policy.burst_penalty_millis));
                    true
                } else {
                    false
                }
            }
        }
    }

    /// RNS `Interface.ic_held_release`: the next instant a held announce may drip out, stamped `now + IC_BURST_PENALTY` when the burst latches.
    pub fn next_held_release_at(&self, interface: InterfaceId) -> Option<InstantMillis> {
        self.table
            .rows()
            .iter()
            .find(|row| row.interface == interface)
            .map(|row| row.next_held_release_at)
    }

    /// RNS `Interface.process_held_announces` advances `ic_held_release` by `IC_HELD_RELEASE_INTERVAL` on each release.
    pub fn schedule_next_held_release(&mut self, interface: InterfaceId, now: InstantMillis) {
        self.schedule_next_held_release_with_policy(
            interface,
            now,
            InterfaceCommonPolicy::RNS_DEFAULT.ingress_control,
        );
    }

    pub fn schedule_next_held_release_with_policy(
        &mut self,
        interface: InterfaceId,
        now: InstantMillis,
        policy: IngressControlPolicy,
    ) {
        if let Some(row) = self
            .table
            .rows_mut()
            .iter_mut()
            .find(|row| row.interface == interface)
        {
            row.next_held_release_at =
                InstantMillis(now.0.saturating_add(policy.held_release_interval_millis));
        }
    }

    /// The gate RNS `Interface.process_held_announces` puts on each release: `ia_freq < freq_threshold`, strictly.
    /// An interface with no samples reads under.
    pub fn rate_is_under_limit(&self, interface: InterfaceId, now: InstantMillis) -> bool {
        self.rate_is_under_limit_with_policy(
            interface,
            now,
            InterfaceCommonPolicy::RNS_DEFAULT.ingress_control,
        )
    }

    pub fn rate_is_under_limit_with_policy(
        &self,
        interface: InterfaceId,
        now: InstantMillis,
        policy: IngressControlPolicy,
    ) -> bool {
        self.table
            .rows()
            .iter()
            .find(|row| row.interface == interface)
            .is_none_or(|row| rate_reading(row, now, policy) == RateReading::UnderLimit)
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
        self.table.push(InterfaceAnnounceLimit {
            interface,
            created_at: now,
            window_started_at: now,
            window_count: 0,
            burst: BurstState::Calm,
            next_held_release_at: InstantMillis(0),
        });
        self.table.rows().len() - 1
    }

    fn evict_least_recent(&mut self) {
        if let Some(index) = self
            .table
            .rows()
            .iter()
            .enumerate()
            .min_by_key(|(_, row)| row.window_started_at.0)
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

    fn limits() -> InterfaceAnnounceLimits<FixedInterfaceAnnounceLimitTable<4>> {
        InterfaceAnnounceLimits::default()
    }

    fn record_then_judge(
        limits: &mut InterfaceAnnounceLimits<FixedInterfaceAnnounceLimitTable<4>>,
        interface: InterfaceId,
        now: InstantMillis,
    ) -> bool {
        limits.record(interface, now);
        limits.should_limit(interface, now)
    }

    #[test]
    fn a_calm_trickle_is_never_limited() {
        let mut limits = limits();
        for second in 0..20u64 {
            assert!(
                !record_then_judge(&mut limits, iface(1), InstantMillis(second * 1_000)),
                "one announce per second stays under even the strict 3 Hz limit",
            );
        }
    }

    #[test]
    fn a_flood_latches_a_burst_that_outlasts_the_traffic() {
        let mut limits = limits();
        let mut limited_any = false;
        for i in 0..12u64 {
            limited_any |= record_then_judge(&mut limits, iface(1), InstantMillis(i * 8));
        }
        assert!(
            limited_any,
            "twelve announces in 100 ms is 120 Hz — the flood trips the limiter",
        );
        assert!(
            record_then_judge(&mut limits, iface(1), InstantMillis(200)),
            "still latched a moment later though no new traffic cleared it",
        );
    }

    #[test]
    fn recorded_announces_count_toward_the_rate_that_limits_an_unknown_one() {
        let mut limits = limits();
        for i in 0..12u64 {
            limits.record(iface(1), InstantMillis(i * 8));
        }
        assert!(
            limits.should_limit(iface(1), InstantMillis(96)),
            "every announce counts via record (RNS received_announce), so a flood — even of known destinations — is the rate the unknown-destination judgment sees",
        );
    }

    #[test]
    fn a_new_interface_trips_where_an_established_one_would_not() {
        let mut young = limits();
        let mut young_limited = false;
        for i in 0..6u64 {
            young_limited |= record_then_judge(&mut young, iface(1), InstantMillis(i * 200));
        }
        assert!(
            young_limited,
            "5 Hz from a fresh interface trips the strict 3 Hz limit",
        );

        let mut old = limits();
        assert!(!record_then_judge(&mut old, iface(2), InstantMillis(0)));
        let start = NEW_INTERFACE_AGE_MS + 1;
        let mut old_limited = false;
        for i in 0..6u64 {
            old_limited |= record_then_judge(&mut old, iface(2), InstantMillis(start + i * 200));
        }
        assert!(
            !old_limited,
            "the same 5 Hz on an interface first seen over 2h ago stays under the relaxed 10 Hz",
        );
    }

    #[test]
    fn a_sustained_flood_stays_latched_through_a_window_tumble() {
        let mut limits = limits();
        for i in 0..12u64 {
            record_then_judge(&mut limits, iface(1), InstantMillis(i * 8));
        }
        for step in 0..3u64 {
            assert!(
                record_then_judge(&mut limits, iface(1), InstantMillis(16_000 + step * 50)),
                "the tumble resets the window to too few samples, and too few samples may not clear a latched burst (RNS IC_BURST_MIN_SAMPLES)",
            );
        }
    }

    #[test]
    fn a_genuinely_calm_interface_clears_after_enough_calm_samples() {
        let mut limits = limits();
        for i in 0..12u64 {
            record_then_judge(&mut limits, iface(1), InstantMillis(i * 8));
        }
        for second in 20..=25u64 {
            assert!(
                record_then_judge(&mut limits, iface(1), InstantMillis(second * 1_000)),
                "still latched while the calm reading gathers its six samples",
            );
        }
        assert!(
            !record_then_judge(&mut limits, iface(1), InstantMillis(26_000)),
            "six slow samples proved the calm and the burst cleared on the previous call",
        );
    }

    #[test]
    fn a_rate_exactly_at_threshold_neither_latches_nor_reads_subsided() {
        let mut limits = limits();
        limits.record(iface(1), InstantMillis(0));
        limits.record(iface(1), InstantMillis(500));
        assert!(
            !record_then_judge(&mut limits, iface(1), InstantMillis(1_000)),
            "three samples in exactly one second is exactly 3 Hz, and RNS latches only strictly above",
        );
        assert!(
            !limits.rate_is_under_limit(iface(1), InstantMillis(1_000)),
            "RNS releases only strictly below the threshold",
        );
    }

    #[test]
    fn simultaneous_announces_read_zero_frequency() {
        let mut limits = limits();
        for _ in 0..4 {
            assert!(
                !record_then_judge(&mut limits, iface(1), InstantMillis(5)),
                "a zero span reads as zero frequency, mirroring the reference's span guard",
            );
        }
    }

    #[test]
    fn an_attach_pinned_interface_ages_from_attach_not_first_announce() {
        let mut limits = limits();
        limits.interface_attached(iface(1), InstantMillis(0));
        let start = NEW_INTERFACE_AGE_MS + 1;
        let mut limited = false;
        for i in 0..6u64 {
            limited |= record_then_judge(&mut limits, iface(1), InstantMillis(start + i * 200));
        }
        assert!(
            !limited,
            "5 Hz two hours after attach is judged by the relaxed 10 Hz limit even though these are the first announces ever",
        );
    }

    #[test]
    fn a_pinned_interface_measures_rate_from_its_first_sample_not_from_attach() {
        let mut limits = limits();
        limits.interface_attached(iface(1), InstantMillis(0));
        limits.record(iface(1), InstantMillis(9_900));
        limits.record(iface(1), InstantMillis(9_950));
        assert!(
            record_then_judge(&mut limits, iface(1), InstantMillis(10_000)),
            "three announces in 100 ms trip the strict limit; the quiet stretch since attach does not dilute the reading",
        );
    }

    #[test]
    fn interfaces_are_tracked_independently() {
        let mut limits = limits();
        for i in 0..12u64 {
            record_then_judge(&mut limits, iface(1), InstantMillis(i * 8));
        }
        assert!(
            !record_then_judge(&mut limits, iface(2), InstantMillis(0)),
            "a flood on one interface does not limit a quiet neighbor",
        );
    }
}
