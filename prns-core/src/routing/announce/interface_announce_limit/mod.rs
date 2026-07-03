mod impls;

pub use impls::*;

use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;

/// RNS `Interface.IC_BURST_FREQ_NEW` (3 Hz, an interface younger than [`NEW_INTERFACE_AGE_MS`])
pub const STRICT_RATE_LIMIT_HZ: u64 = 3;
/// RNS `Interface.IC_BURST_FREQ` (10 Hz, an established interface)
pub const RELAXED_RATE_LIMIT_HZ: u64 = 10;
/// RNS `Interface.IC_NEW_TIME` (2 hours)
pub const NEW_INTERFACE_AGE_MS: u64 = 2 * 60 * 60 * 1_000;
/// RNS `Interface.IC_BURST_HOLD` (15 seconds): the minimum a burst stays latched
pub const BURST_HOLD_MS: u64 = 15 * 1_000;
/// RNS `Interface.AR_FREQ_DECAY` (10 seconds): the rate-measurement window
pub const FREQUENCY_WINDOW_MS: u64 = 10 * 1_000;
/// RNS `Interface.IC_DEQUE_MIN_SAMPLE` + 1: samples needed before a rate is judged
pub const MIN_SAMPLES_TO_JUDGE: u16 = 3;
/// RNS `Interface.IC_BURST_PENALTY` (15 seconds): the wait after a burst latches before the first held announce may drip out
pub const BURST_PENALTY_MS: u64 = 15 * 1_000;
/// RNS `Interface.IC_HELD_RELEASE_INTERVAL` (5 seconds): the minimum spacing between drip-released announces
pub const HELD_RELEASE_INTERVAL_MS: u64 = 5 * 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BurstState {
    Calm,
    Bursting(InstantMillis),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceAnnounceLimit {
    pub interface: InterfaceId,
    pub created_at: InstantMillis,
    pub window_start: InstantMillis,
    pub window_count: u16,
    pub burst: BurstState,
    pub held_release: InstantMillis,
}

fn over_threshold(row: &InterfaceAnnounceLimit, now: InstantMillis) -> bool {
    let threshold = if now.0.saturating_sub(row.created_at.0) < NEW_INTERFACE_AGE_MS {
        STRICT_RATE_LIMIT_HZ
    } else {
        RELAXED_RATE_LIMIT_HZ
    };
    let elapsed_ms = now.0.saturating_sub(row.window_start.0);
    row.window_count >= MIN_SAMPLES_TO_JUDGE
        && u64::from(row.window_count) * 1_000 > threshold * elapsed_ms
}

pub trait InterfaceAnnounceLimitColumns {
    fn capacity(&self) -> usize;
    fn rows(&self) -> &[InterfaceAnnounceLimit];
    fn rows_mut(&mut self) -> &mut [InterfaceAnnounceLimit];
    fn push(&mut self, row: InterfaceAnnounceLimit);
    fn swap_remove(&mut self, index: usize);
}

#[derive(Debug, Default)]
pub struct InterfaceAnnounceLimits<C: InterfaceAnnounceLimitColumns> {
    columns: C,
}

impl<C: InterfaceAnnounceLimitColumns> InterfaceAnnounceLimits<C> {
    /// RNS 1.3.5 `Interface.received_announce` (Interface.py:259): the sample deque appends
    /// on every announce, known or unknown destination; [`Self::should_limit`] reads the window.
    pub fn record(&mut self, interface: InterfaceId, now: InstantMillis) {
        let index = self.index_or_insert(interface, now);
        let row = &mut self.columns.rows_mut()[index];
        if now.0.saturating_sub(row.window_start.0) >= FREQUENCY_WINDOW_MS {
            row.window_start = now;
            row.window_count = 1;
        } else {
            row.window_count = row.window_count.saturating_add(1);
        }
    }

    /// RNS 1.3.5 `Interface.should_ingress_limit` (Interface.py:145): latch or clear the
    /// burst and report whether an unknown-destination announce arriving now should be held.
    /// Call [`Self::record`] first; an interface never recorded is treated as calm.
    pub fn should_limit(&mut self, interface: InterfaceId, now: InstantMillis) -> bool {
        let Some(index) = self
            .columns
            .rows()
            .iter()
            .position(|row| row.interface == interface)
        else {
            return false;
        };
        let row = &mut self.columns.rows_mut()[index];
        let over_threshold = over_threshold(row, now);

        match row.burst {
            BurstState::Bursting(since) => {
                if !over_threshold && now.0 >= since.0.saturating_add(BURST_HOLD_MS) {
                    row.burst = BurstState::Calm;
                }
                true
            }
            BurstState::Calm => {
                if over_threshold {
                    row.burst = BurstState::Bursting(now);
                    row.held_release = InstantMillis(now.0.saturating_add(BURST_PENALTY_MS));
                    true
                } else {
                    false
                }
            }
        }
    }

    /// RNS `Interface.ic_held_release` (Interface.py:236): the next instant a held announce
    /// may drip out, set to `now + IC_BURST_PENALTY` when the burst latches.
    pub fn held_release_for(&self, interface: InterfaceId) -> Option<InstantMillis> {
        self.columns
            .rows()
            .iter()
            .find(|row| row.interface == interface)
            .map(|row| row.held_release)
    }

    /// RNS advances `ic_held_release` by `IC_HELD_RELEASE_INTERVAL` on each release
    /// (Interface.py:250).
    pub fn advance_held_release(&mut self, interface: InterfaceId, now: InstantMillis) {
        if let Some(row) = self
            .columns
            .rows_mut()
            .iter_mut()
            .find(|row| row.interface == interface)
        {
            row.held_release = InstantMillis(now.0.saturating_add(HELD_RELEASE_INTERVAL_MS));
        }
    }

    /// The gate RNS puts on each release: `ia_freq < freq_threshold` (Interface.py:239).
    /// An interface with no samples reads as subsided.
    pub fn rate_subsided(&self, interface: InterfaceId, now: InstantMillis) -> bool {
        self.columns
            .rows()
            .iter()
            .find(|row| row.interface == interface)
            .is_none_or(|row| !over_threshold(row, now))
    }

    pub fn len(&self) -> usize {
        self.columns.rows().len()
    }

    pub fn is_empty(&self) -> bool {
        self.columns.rows().is_empty()
    }

    fn index_or_insert(&mut self, interface: InterfaceId, now: InstantMillis) -> usize {
        if let Some(index) = self
            .columns
            .rows()
            .iter()
            .position(|row| row.interface == interface)
        {
            return index;
        }
        if self.columns.rows().len() >= self.columns.capacity() {
            self.evict_least_recent();
        }
        self.columns.push(InterfaceAnnounceLimit {
            interface,
            created_at: now,
            window_start: now,
            window_count: 0,
            burst: BurstState::Calm,
            held_release: InstantMillis(0),
        });
        self.columns.rows().len() - 1
    }

    fn evict_least_recent(&mut self) {
        if let Some(index) = self
            .columns
            .rows()
            .iter()
            .enumerate()
            .min_by_key(|(_, row)| row.window_start.0)
            .map(|(index, _)| index)
        {
            self.columns.swap_remove(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iface(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 8])
    }

    fn limits() -> InterfaceAnnounceLimits<FixedInterfaceAnnounceLimitColumns<4>> {
        InterfaceAnnounceLimits::default()
    }

    fn record_then_judge(
        limits: &mut InterfaceAnnounceLimits<FixedInterfaceAnnounceLimitColumns<4>>,
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
