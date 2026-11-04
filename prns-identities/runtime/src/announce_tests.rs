#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TestError {
    Before,
    After,
    Conflict,
}
struct Memory<const N: usize> {
    state: Snapshot<N>,
    writes: usize,
    failure: Option<TestError>,
}
impl<const N: usize> Storage<N> for Memory<N> {
    type Error = TestError;
    fn load(&mut self, into: &mut Snapshot<N>) -> Result<(), TestError> {
        *into = self.state;
        Ok(())
    }
    fn compare_and_store(&mut self, revision: u64, next: &Snapshot<N>) -> Result<(), TestError> {
        if revision != self.state.revision {
            return Err(TestError::Conflict);
        }
        if self.failure == Some(TestError::Before) {
            return Err(TestError::Before);
        }
        self.state = *next;
        self.writes += 1;
        if self.failure == Some(TestError::After) {
            return Err(TestError::After);
        }
        Ok(())
    }
}
struct Harness<const N: usize> {
    memory: Memory<N>,
    state: Snapshot<N>,
    scratch: Snapshot<N>,
    runtime: Runtime<N>,
}
impl<const N: usize> Harness<N> {
    fn new() -> Self {
        let mut result = Self {
            memory: Memory {
                state: Snapshot::empty(),
                writes: 0,
                failure: None,
            },
            state: Snapshot::empty(),
            scratch: Snapshot::empty(),
            runtime: Runtime::empty(),
        };
        result.controller().reload(0, Some(100_000)).unwrap();
        result
    }
    fn controller(&mut self) -> Scheduler<'_, N, Memory<N>> {
        Scheduler {
            storage: &mut self.memory,
            snapshot: &mut self.state,
            scratch: &mut self.scratch,
            runtime: &mut self.runtime,
        }
    }
    fn configure(&mut self, id: u64, enabled: bool, interval: Option<u64>, now: u64) {
        self.controller()
            .configure(
                key(id),
                Configuration {
                    channels: 3,
                    interval_ms: interval,
                    enabled,
                },
                now,
                None,
            )
            .unwrap();
    }
    fn reserve(&mut self, now: u64) -> Option<Reservation> {
        self.controller().reserve_next(now, None, true, 0).unwrap()
    }
}
fn key(id: u64) -> AnnounceKey {
    AnnounceKey {
        identity: LocalIdentityId {
            incarnation: [7; 16],
            sequence: id,
        },
        service: ServiceId(9),
    }
}

#[test]
fn startup_waits_for_interface_and_coalesces_without_timer_writes() {
    let mut test = Harness::<2>::new();
    test.configure(1, true, Some(900_000), 0);
    let writes = test.memory.writes;
    for now in [0, 900_000, 9_000_000] {
        assert!(test
            .controller()
            .reserve_next(now, None, false, 0)
            .unwrap()
            .is_none());
    }
    assert_eq!(test.memory.writes, writes);
    let attempt = test.reserve(9_000_000).unwrap();
    assert_eq!(attempt.key(), key(1));
    assert_eq!(
        test.memory.state.entries[0]
            .unwrap()
            .last_attempt
            .unwrap()
            .sequence,
        attempt.sequence()
    );
    assert!(test.reserve(9_060_000).is_none());
    assert!(test.reserve(9_899_999).is_none());
    assert!(test.reserve(9_900_000).is_some());
}

#[test]
fn global_budget_and_round_robin_prevent_manual_starvation() {
    let mut test = Harness::<3>::new();
    test.configure(1, true, None, 0);
    test.configure(2, true, Some(900_000), 0);
    test.configure(3, true, Some(900_000), 0);
    test.controller().request_manual(key(1), 0, None).unwrap();
    assert_eq!(test.reserve(0).unwrap().key(), key(1));
    for now in 1..10 {
        assert!(matches!(
            test.controller().request_manual(key(1), now, None).unwrap(),
            ManualRequest::Throttled { .. }
        ));
    }
    assert!(test.reserve(59_999).is_none());
    assert_eq!(test.reserve(60_000).unwrap().key(), key(2));
    assert_eq!(test.reserve(120_000).unwrap().key(), key(3));
    assert_eq!(test.reserve(180_000).unwrap().key(), key(1));
    assert!(test.reserve(240_000).is_none());
}

#[test]
fn pause_cancels_only_its_work_and_preferences_never_reset_cooldown() {
    let mut test = Harness::<2>::new();
    test.configure(1, true, Some(900_000), 0);
    test.configure(2, true, Some(900_000), 0);
    assert_eq!(test.reserve(0).unwrap().key(), key(1));
    test.controller().request_manual(key(1), 1, None).unwrap();
    test.configure(1, false, Some(900_000), 2);
    assert_eq!(
        test.controller().request_manual(key(1), 3, None).unwrap(),
        ManualRequest::Disabled
    );
    assert_eq!(test.reserve(60_000).unwrap().key(), key(2));
    assert!(test.reserve(120_000).is_none());
    test.configure(1, true, Some(900_000), 120_000);
    assert_eq!(test.reserve(120_000).unwrap().key(), key(1));
    test.configure(1, true, None, 120_001);
    test.configure(1, true, Some(1_800_000), 120_002);
    assert_eq!(
        test.controller()
            .request_manual(key(1), 120_003, None)
            .unwrap(),
        ManualRequest::Throttled {
            retry_after_ms: 59_997
        }
    );
    assert!(test.reserve(179_999).is_none());
    assert_eq!(test.reserve(180_000).unwrap().key(), key(1));
}

#[test]
fn manual_only_and_interval_edit_do_not_create_startup_work() {
    let mut test = Harness::<1>::new();
    test.configure(1, true, None, 0);
    assert!(test.reserve(1).is_none());
    test.configure(1, true, Some(900_000), 2);
    assert!(test.reserve(3).is_none());
    assert!(test.reserve(900_001).is_none());
    assert!(test.reserve(900_002).is_some());
}

#[test]
fn partial_outcomes_are_durable_and_replay_is_exact() {
    let mut test = Harness::<1>::new();
    test.configure(1, true, Some(900_000), 0);
    let reservation = test.reserve(0).unwrap();
    assert_eq!(
        test.controller().complete(reservation, 1, 1),
        Err(Error::Fault(Fault::InvalidOutcome))
    );
    test.controller().complete(reservation, 1, 2).unwrap();
    let writes = test.memory.writes;
    test.controller().complete(reservation, 1, 2).unwrap();
    assert_eq!(test.memory.writes, writes);
    assert_eq!(
        test.controller().complete(reservation, 3, 0),
        Err(Error::Fault(Fault::InvalidOutcome))
    );
    let durable = test.memory.state.entries[0].unwrap().last_attempt.unwrap();
    assert!(durable.completed);
    assert_eq!((durable.requested_mask, durable.failed_mask), (1, 2));
    test.controller()
        .request_manual(key(1), 60_000, None)
        .unwrap();
    test.reserve(60_000).unwrap();
    assert_eq!(
        test.controller().complete(reservation, 1, 2),
        Err(Error::Fault(Fault::StaleReservation))
    );
}

#[test]
fn uncertain_storage_error_fences_dispatch_and_restart_never_replays_attempt() {
    for failure in [TestError::Before, TestError::After] {
        let mut test = Harness::<1>::new();
        test.configure(1, true, Some(900_000), 0);
        test.reserve(0).unwrap();
        test.controller()
            .request_manual(key(1), 60_000, None)
            .unwrap();
        test.memory.failure = Some(failure);
        assert_eq!(
            test.controller().reserve_next(60_000, None, true, 0),
            Err(Error::Storage(failure))
        );
        assert_eq!(
            test.controller().reserve_next(120_000, None, true, 0),
            Err(Error::Fault(Fault::ReloadRequired))
        );
        let last = test.memory.state.last_sequence;
        test.memory.failure = None;
        test.controller().reload(0, None).unwrap();
        assert!(test.reserve(59_999).is_none());
        let next = test.reserve(60_000).unwrap();
        assert_eq!(next.sequence(), last + 1);
    }
}

#[test]
fn reboot_and_clock_rollbacks_start_fresh_cooldowns() {
    let mut test = Harness::<1>::new();
    test.configure(1, true, Some(900_000), 0);
    test.reserve(0).unwrap();
    test.controller().reload(500_000, Some(1_000_000)).unwrap();
    assert!(test.reserve(559_999).is_none());
    assert!(test.reserve(560_000).is_some());
    assert_eq!(
        test.controller()
            .request_manual(key(1), 600_000, Some(999_999))
            .unwrap(),
        ManualRequest::Throttled {
            retry_after_ms: 60_000
        }
    );
    assert!(test.reserve(659_999).is_none());
    assert!(test.reserve(660_000).is_some());
    assert_eq!(
        test.controller().request_manual(key(1), 1, None).unwrap(),
        ManualRequest::Throttled {
            retry_after_ms: 60_000
        }
    );
    assert!(test.reserve(60_000).is_none());
    assert!(test.reserve(60_001).is_some());
}

#[test]
fn jitter_is_positive_bounded_and_applied_to_each_period() {
    assert_eq!(jittered_interval(900_000, 0), 900_000);
    assert_eq!(jittered_interval(900_000, u16::MAX), 1_125_000);
    let mut test = Harness::<1>::new();
    test.configure(1, true, Some(900_000), 0);
    assert!(test
        .controller()
        .reserve_next(0, None, true, u16::MAX)
        .unwrap()
        .is_some());
    assert!(test.reserve(1_124_999).is_none());
    assert!(test.reserve(1_125_000).is_some());
}

#[test]
fn bounded_capacity_and_invalid_snapshot_fail_closed() {
    let mut test = Harness::<1>::new();
    test.configure(1, true, None, 0);
    assert_eq!(
        test.controller().configure(
            key(2),
            Configuration {
                channels: 1,
                interval_ms: None,
                enabled: true
            },
            0,
            None
        ),
        Err(Error::Fault(Fault::Capacity))
    );
    test.memory.state.last_sequence = 5;
    assert_eq!(
        test.controller().reload(0, None),
        Err(Error::Fault(Fault::InvalidSnapshot))
    );
    assert_eq!(
        test.controller().request_manual(key(1), 1, None),
        Err(Error::Fault(Fault::ReloadRequired))
    );
}

#[test]
fn exhausted_clock_cannot_turn_saturated_deadlines_into_unlimited_announces() {
    let mut test = Harness::<1>::new();
    test.configure(1, true, Some(900_000), 0);
    assert_eq!(
        test.controller().reserve_next(u64::MAX, None, true, 0),
        Err(Error::Fault(Fault::Exhausted))
    );
    assert_eq!(test.memory.state.last_sequence, 0);
    assert_eq!(
        test.controller().reload(u64::MAX, None),
        Err(Error::Fault(Fault::Exhausted))
    );
}

#[test]
fn busy_owner_keeps_its_pending_work_without_blocking_another_owner() {
    let mut test = Harness::<2>::new();
    test.configure(1, true, None, 0);
    test.configure(2, true, Some(900_000), 0);
    test.controller().request_manual(key(1), 0, None).unwrap();
    let writes = test.memory.writes;
    assert!(test
        .controller()
        .reserve_next_matching(0, None, true, 0, |_| false)
        .unwrap()
        .is_none());
    assert_eq!(test.memory.writes, writes);
    let attempt = test
        .controller()
        .reserve_next_matching(0, None, true, 0, |candidate| candidate != key(1))
        .unwrap()
        .unwrap();
    assert_eq!(attempt.key(), key(2));
    assert!(test.reserve(59_999).is_none());
    assert_eq!(test.reserve(60_000).unwrap().key(), key(1));
}

#[test]
fn timer_hint_is_idle_for_manual_only_and_tracks_shared_cooldown_and_period() {
    let mut test = Harness::<2>::new();
    test.configure(1, true, None, 0);
    assert_eq!(test.controller().next_ready_at(0).unwrap(), None);
    test.controller().request_manual(key(1), 0, None).unwrap();
    assert_eq!(test.controller().next_ready_at(0).unwrap(), Some(0));
    test.reserve(0).unwrap();
    assert_eq!(test.controller().next_ready_at(1).unwrap(), None);
    test.configure(2, true, Some(900_000), 1);
    assert_eq!(test.controller().next_ready_at(1).unwrap(), Some(60_000));
    test.reserve(60_000).unwrap();
    assert_eq!(
        test.controller().next_ready_at(60_001).unwrap(),
        Some(960_000)
    );
    test.configure(2, false, Some(900_000), 60_001);
    assert_eq!(test.controller().next_ready_at(60_001).unwrap(), None);
}
