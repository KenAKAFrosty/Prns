//! Allocation-free scheduling policy for bounded group exchanges.
//!
//! A first-hop bitrate is not an end-to-end throughput measurement. Unknown
//! routes and paths through a router retain a conservative rate estimate even
//! when the local interface is fast. These estimates choose work and deadlines;
//! they confer no delivery, custody, or network capacity guarantee.

/// Locally observed path information; no new network advertisement is required.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PathCapabilities {
    pub local_bitrate_bps: Option<u64>,
    /// One is a directly reachable peer. `None` means no current route evidence.
    pub hops: Option<u8>,
    /// Measured Link round trip, not the local interface's connection latency.
    pub rtt_millis: Option<u64>,
}

/// Caller-owned bounds. All times use monotonic milliseconds in the executor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupSyncPolicy"))]
pub struct SyncPolicy {
    pub min_exchange_millis: u64,
    pub max_exchange_millis: u64,
    pub unknown_bitrate_bps: u64,
    pub unknown_rtt_millis: u64,
    pub fast_bitrate_bps: u64,
    pub fast_rtt_millis: u64,
    pub fast_poll_millis: u64,
    pub constrained_poll_millis: u64,
    /// Healthy direct fast paths stay interactive even after a long idle.
    pub max_fast_idle_millis: u64,
    pub max_idle_millis: u64,
    /// Unsolicited destination refresh, separate from synchronization polling.
    /// The default follows Reticulum's six-hour application guidance.
    pub announce_interval_millis: u64,
}

impl Default for SyncPolicy {
    fn default() -> Self {
        Self {
            min_exchange_millis: 10_000,
            max_exchange_millis: 600_000,
            unknown_bitrate_bps: 1_200,
            unknown_rtt_millis: 2_000,
            fast_bitrate_bps: 64_000,
            fast_rtt_millis: 500,
            fast_poll_millis: 2_000,
            constrained_poll_millis: 15_000,
            max_fast_idle_millis: 10_000,
            max_idle_millis: 300_000,
            announce_interval_millis: 6 * 60 * 60 * 1_000,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyncTuning {
    pub active_poll_millis: u64,
    pub idle_poll_millis: u64,
    pub max_idle_millis: u64,
    pub retry_initial_millis: u64,
    pub retry_max_millis: u64,
    pub announce_interval_millis: u64,
    pub max_page_items: usize,
    pub max_requests: usize,
    pub max_envelopes: usize,
    /// Soft work target: one larger envelope may progress within caller limits.
    pub target_bytes: usize,
}

impl SyncPolicy {
    pub fn is_valid(self) -> bool {
        const WEEK_MILLIS: u64 = 7 * 24 * 60 * 60 * 1_000;
        self.min_exchange_millis > 0
            && self.max_exchange_millis >= self.min_exchange_millis
            && self.max_exchange_millis <= WEEK_MILLIS
            && self.unknown_bitrate_bps > 0
            && self.unknown_rtt_millis > 0
            && self.unknown_rtt_millis <= self.max_exchange_millis
            && self.fast_bitrate_bps > 0
            && self.fast_rtt_millis > 0
            && self.fast_poll_millis > 0
            && self.constrained_poll_millis >= self.fast_poll_millis
            && self.max_fast_idle_millis >= self.fast_poll_millis
            && self.max_fast_idle_millis <= WEEK_MILLIS
            && self.max_idle_millis >= self.constrained_poll_millis
            && self.max_idle_millis <= WEEK_MILLIS
            && self.announce_interval_millis >= self.constrained_poll_millis
            && self.announce_interval_millis >= 60_000
            && self.announce_interval_millis <= WEEK_MILLIS
    }

    fn estimated_bitrate(self, path: PathCapabilities) -> u64 {
        let local = path
            .local_bitrate_bps
            .filter(|rate| *rate > 0)
            .unwrap_or(self.unknown_bitrate_bps);
        if path.hops == Some(1) {
            local
        } else {
            local.min(self.unknown_bitrate_bps)
        }
        .max(1)
    }

    fn estimated_rtt(self, path: PathCapabilities) -> u64 {
        path.rtt_millis
            .filter(|rtt| *rtt > 0)
            .unwrap_or(self.unknown_rtt_millis)
    }

    pub fn tuning(self, path: PathCapabilities) -> SyncTuning {
        let fast = path.hops == Some(1)
            && path
                .local_bitrate_bps
                .is_some_and(|rate| rate >= self.fast_bitrate_bps)
            && path
                .rtt_millis
                .is_none_or(|rtt| rtt <= self.fast_rtt_millis);
        let poll = if fast {
            self.fast_poll_millis
        } else {
            self.constrained_poll_millis
        };
        let idle_max = if fast {
            self.max_fast_idle_millis.min(self.max_idle_millis)
        } else {
            self.max_idle_millis
        };
        SyncTuning {
            active_poll_millis: poll,
            idle_poll_millis: poll.saturating_mul(4).min(idle_max),
            max_idle_millis: idle_max,
            retry_initial_millis: poll,
            retry_max_millis: self.max_idle_millis,
            announce_interval_millis: self.announce_interval_millis,
            max_page_items: if fast { 32 } else { 4 },
            max_requests: if fast { 8 } else { 4 },
            max_envelopes: if fast { 4 } else { 1 },
            target_bytes: if fast { 65_536 } else { 4_096 },
        }
    }

    /// Bounded allowance for round trips, serialization and retransmission.
    /// This is an estimate, not a transport throughput claim. Larger legal work
    /// can still require a caller-selected larger ceiling on extremely slow paths.
    pub fn exchange_deadline_millis(
        self,
        path: PathCapabilities,
        request_bytes: usize,
        response_bytes: usize,
    ) -> u64 {
        let bytes = u64::try_from(request_bytes)
            .unwrap_or(u64::MAX)
            .saturating_add(u64::try_from(response_bytes).unwrap_or(u64::MAX))
            .saturating_add(1_024);
        let rate = self.estimated_bitrate(path);
        let serialization = bytes.saturating_mul(8_000).div_ceil(rate);
        self.estimated_rtt(path)
            .saturating_mul(8)
            .saturating_add(serialization.saturating_mul(3))
            .saturating_add(self.min_exchange_millis)
            .clamp(
                self.min_exchange_millis,
                self.max_exchange_millis.max(self.min_exchange_millis),
            )
    }

    pub fn connect_deadline_millis(self, path: PathCapabilities) -> u64 {
        // Path lookup and the identified Link handshake need several packets.
        self.exchange_deadline_millis(path, 2_048, 2_048)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncActivity {
    /// Durable progress or a bounded scan continuation.
    Progress,
    /// The selected inventory was exhausted without new work.
    Idle,
    Failed,
}

/// Disposable executor observations, never durable receipt or reachability
/// guarantees. An application must query record delivery evidence separately.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupSyncPhase"))]
pub enum SyncPhase {
    Unknown,
    Waiting,
    Connecting,
    Synchronizing,
    RetryScheduled,
    StorageBlocked,
    MembershipCatchUp,
}

/// Bounded categories suitable for every application profile. No payload,
/// platform error string, key material or transport implementation leaks here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupSyncIssue"))]
pub enum SyncIssue {
    Transport,
    StorageBusy,
    StorageFull,
    StorageUnavailable,
    MembershipChanged,
    TimeUncertain,
    Rejected,
    UnsupportedPeer,
    Configuration,
}

/// Small, disposable scheduling state. It is never a durable receipt or cursor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PeerSchedule {
    idle_rounds: u8,
    failures: u8,
}

/// Evidence that may shorten an existing delay. An executor must obtain fresh
/// work from durable storage and peer recovery from current authenticated
/// membership; announcements and generic notifications are not such evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncWake {
    FreshWork,
    PendingWork,
    PeerRecovered,
}

impl PeerSchedule {
    pub fn failures(&self) -> u8 {
        self.failures
    }

    /// Coalescing, minimum-spacing-preserving acceleration. This does not erase
    /// failure history or grant another attempt. Repeated evidence cannot move a
    /// due time later, and the executor advances the floor after every attempt.
    pub fn accelerated_delay_millis(
        &self,
        remaining: u64,
        minimum: u64,
        evidence: SyncWake,
    ) -> u64 {
        if evidence == SyncWake::PendingWork && self.failures != 0 {
            remaining
        } else {
            remaining.min(minimum)
        }
    }

    /// Reset idle suppression for newly admitted local work without erasing
    /// failure backoff. Executors must still honor the previous minimum spacing.
    pub fn wake(&mut self) {
        self.idle_rounds = 0;
    }

    pub fn delay_millis(&mut self, tuning: SyncTuning, activity: SyncActivity, jitter: u32) -> u64 {
        let (base, ceiling) = match activity {
            SyncActivity::Progress => {
                self.idle_rounds = 0;
                self.failures = 0;
                (tuning.active_poll_millis, tuning.retry_max_millis)
            }
            SyncActivity::Idle => {
                self.failures = 0;
                let delay = tuning
                    .idle_poll_millis
                    .saturating_mul(1_u64 << self.idle_rounds.min(8));
                self.idle_rounds = self.idle_rounds.saturating_add(1);
                (delay, tuning.max_idle_millis)
            }
            SyncActivity::Failed => {
                let delay = tuning
                    .retry_initial_millis
                    .saturating_mul(1_u64 << self.failures.min(8));
                self.failures = self.failures.saturating_add(1);
                (delay, tuning.retry_max_millis)
            }
        };
        let base = base.min(ceiling);
        // Add 0..25 percent, never retry sooner than the selected base spacing.
        base.saturating_add(u64::from(jitter) % (base / 4 + 1))
            .min(ceiling)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acceleration_preserves_spacing_failure_history_and_ceiling() {
        let tuning = SyncPolicy::default().tuning(PathCapabilities::default());
        let mut state = PeerSchedule::default();
        for _ in 0..10 {
            state.delay_millis(tuning, SyncActivity::Failed, 0);
        }
        let failures = state.failures();
        assert_eq!(
            state.accelerated_delay_millis(300_000, 15_000, SyncWake::PendingWork),
            300_000
        );
        for evidence in [SyncWake::FreshWork, SyncWake::PeerRecovered] {
            assert_eq!(
                state.accelerated_delay_millis(300_000, 15_000, evidence),
                15_000
            );
            assert_eq!(
                state.accelerated_delay_millis(7_000, 15_000, evidence),
                7_000
            );
            assert_eq!(
                state.accelerated_delay_millis(15_000, 15_000, evidence),
                15_000
            );
            assert_eq!(state.failures(), failures);
        }
        assert_eq!(
            state.delay_millis(tuning, SyncActivity::Failed, u32::MAX),
            tuning.retry_max_millis
        );
        state.delay_millis(tuning, SyncActivity::Idle, 0);
        assert_eq!(
            state.accelerated_delay_millis(60_000, 15_000, SyncWake::PendingWork),
            15_000
        );
    }

    #[test]
    fn destination_refresh_is_infrequent_and_independent_of_path_polling() {
        let policy = SyncPolicy::default();
        assert!(policy.is_valid());
        assert_eq!(policy.announce_interval_millis, 6 * 60 * 60 * 1_000);
        for path in [
            PathCapabilities::default(),
            PathCapabilities {
                local_bitrate_bps: Some(1_000_000),
                hops: Some(1),
                rtt_millis: Some(20),
            },
        ] {
            let tuning = policy.tuning(path);
            assert_eq!(
                tuning.announce_interval_millis,
                policy.announce_interval_millis
            );
            assert!(tuning.active_poll_millis < tuning.announce_interval_millis);
        }
    }

    #[test]
    fn announcement_interval_obeys_shared_one_minute_floor() {
        assert!(!SyncPolicy {
            announce_interval_millis: 59_999,
            ..SyncPolicy::default()
        }
        .is_valid());
        assert!(SyncPolicy {
            announce_interval_millis: 60_000,
            ..SyncPolicy::default()
        }
        .is_valid());
    }

    #[test]
    fn fast_first_hop_does_not_hide_a_downstream_bottleneck() {
        let policy = SyncPolicy::default();
        let direct = PathCapabilities {
            local_bitrate_bps: Some(1_000_000),
            hops: Some(1),
            rtt_millis: Some(20),
        };
        let routed = PathCapabilities {
            hops: Some(3),
            ..direct
        };
        assert!(policy.is_valid());
        assert!(
            policy.exchange_deadline_millis(routed, 100, 4_096)
                > policy.exchange_deadline_millis(direct, 100, 4_096)
        );
        assert_eq!(policy.tuning(routed).max_envelopes, 1);
        assert!(policy.tuning(routed).idle_poll_millis > policy.tuning(direct).idle_poll_millis);
    }

    #[test]
    fn slower_interface_larger_resource_and_measured_rtt_extend_deadlines() {
        let policy = SyncPolicy::default();
        let base = PathCapabilities {
            local_bitrate_bps: Some(9_600),
            hops: Some(1),
            rtt_millis: Some(100),
        };
        let slow = PathCapabilities {
            local_bitrate_bps: Some(1_200),
            ..base
        };
        let delayed = PathCapabilities {
            rtt_millis: Some(3_000),
            ..base
        };
        let baseline = policy.exchange_deadline_millis(base, 100, 1_024);
        assert!(policy.exchange_deadline_millis(slow, 100, 1_024) > baseline);
        assert!(policy.exchange_deadline_millis(delayed, 100, 1_024) > baseline);
        assert!(policy.exchange_deadline_millis(base, 100, 16_384) > baseline);
    }

    #[test]
    fn unknown_and_overflowing_inputs_remain_conservative_and_bounded() {
        let policy = SyncPolicy::default();
        assert!(policy.connect_deadline_millis(PathCapabilities::default()) > 10_000);
        let invalid_rate = PathCapabilities {
            local_bitrate_bps: Some(0),
            ..PathCapabilities::default()
        };
        assert_eq!(policy.tuning(invalid_rate).max_envelopes, 1);
        assert_eq!(
            policy.exchange_deadline_millis(invalid_rate, usize::MAX, usize::MAX),
            policy.max_exchange_millis
        );
        assert!(!SyncPolicy {
            unknown_bitrate_bps: 0,
            ..policy
        }
        .is_valid());
        assert!(!SyncPolicy {
            max_exchange_millis: 1,
            ..policy
        }
        .is_valid());
    }

    #[test]
    fn idle_and_failed_peers_back_off_but_progress_resets_only_scheduling() {
        let tuning = SyncPolicy::default().tuning(PathCapabilities::default());
        let mut schedule = PeerSchedule::default();
        let first = schedule.delay_millis(tuning, SyncActivity::Idle, 0);
        assert!(schedule.delay_millis(tuning, SyncActivity::Idle, 0) > first);
        assert_eq!(
            schedule.delay_millis(tuning, SyncActivity::Progress, 0),
            tuning.active_poll_millis
        );
        let failure = schedule.delay_millis(tuning, SyncActivity::Failed, 0);
        schedule.wake();
        assert!(schedule.delay_millis(tuning, SyncActivity::Failed, 0) > failure);
        for _ in 0..300 {
            assert!(
                schedule.delay_millis(tuning, SyncActivity::Failed, u32::MAX)
                    <= tuning.retry_max_millis
            );
        }
    }

    #[test]
    fn slow_round_trip_disables_fast_polling_even_on_a_direct_interface() {
        let policy = SyncPolicy::default();
        let path = PathCapabilities {
            local_bitrate_bps: Some(1_000_000),
            hops: Some(1),
            rtt_millis: Some(2_000),
        };
        assert_eq!(policy.tuning(path).max_envelopes, 1);
    }

    #[test]
    fn healthy_fast_peers_keep_an_interactive_idle_bound_without_shortening_failure_backoff() {
        let policy = SyncPolicy::default();
        let path = PathCapabilities {
            local_bitrate_bps: Some(1_000_000),
            hops: Some(1),
            rtt_millis: Some(20),
        };
        let tuning = policy.tuning(path);
        let mut schedule = PeerSchedule::default();
        for _ in 0..100 {
            assert!(
                schedule.delay_millis(tuning, SyncActivity::Idle, u32::MAX)
                    <= policy.max_fast_idle_millis
            );
        }
        let mut failure = 0;
        for _ in 0..100 {
            failure = schedule.delay_millis(tuning, SyncActivity::Failed, 0);
        }
        assert!(failure > policy.max_fast_idle_millis);
        assert_eq!(failure, policy.max_idle_millis);
    }
}
