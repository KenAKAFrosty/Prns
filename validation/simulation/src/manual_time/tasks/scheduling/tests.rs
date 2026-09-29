use std::cell::RefCell;
use std::collections::BTreeSet;
use std::future::poll_fn;
use std::num::NonZeroUsize;
use std::rc::Rc;
use std::task::Poll;

use super::super::{
    ready::ReadyTasks, ManualTaskCancellation, ManualTaskId, ManualTaskPoll, ManualTaskRunner,
};
use super::*;

fn policy(seed: u64) -> ManualTaskScheduling {
    ManualTaskScheduling::Seeded {
        seed: SimulationSeed::new(seed),
    }
}

#[test]
fn version_one_has_portable_golden_ranks_and_order() {
    assert_eq!(SEEDED_TASK_SCHEDULING_ALGORITHM_VERSION, 1);
    let ranks: Vec<_> = (0..8).map(|id| policy(0).rank(id)).collect();
    assert_eq!(
        ranks,
        [
            5901671266035840458,
            6042066344586392571,
            3683072317952995791,
            6740828644927580069,
            14297266803166169673,
            4710826639445855965,
            15477604880652232379,
            12328205321864036484
        ]
    );
    let mut ready = ReadyTasks::new(policy(0));
    for id in 0..8 {
        ready.insert(ManualTaskId(id));
    }
    let order: Vec<_> = std::iter::from_fn(|| ready.pop_next()).collect();
    assert_eq!(order, [2, 5, 0, 1, 3, 7, 4, 6].map(ManualTaskId));
    assert_eq!(ready.counts(), (8, 0));
}

#[test]
fn seeded_cycles_are_repeatable_fair_and_wake_coalesced() {
    fn run(scheduling: ManualTaskScheduling) -> Vec<usize> {
        let mut driver = super::super::tests::driver();
        let mut runner = ManualTaskRunner::new_with_scheduling(
            &mut driver,
            NonZeroUsize::new(8).unwrap(),
            scheduling,
        );
        let initial = runner.snapshot().unwrap();
        let order = Rc::new(RefCell::new(Vec::new()));
        for id in 0..8 {
            let order = order.clone();
            runner
                .insert(poll_fn(move |cx| {
                    order.borrow_mut().push(id);
                    for _ in 0..100 {
                        cx.waker().wake_by_ref();
                    }
                    Poll::<()>::Pending
                }))
                .unwrap();
        }
        for _ in 0..32 {
            assert!(matches!(
                runner.poll_next().unwrap(),
                ManualTaskPoll::Pending { .. }
            ));
            assert_eq!(ReadyTasks::lock(&runner.ready).counts(), (8, 8));
        }
        assert_eq!(runner.snapshot().unwrap(), initial);
        let order = order.borrow().clone();
        for cycle in order.as_chunks::<8>().0 {
            assert_eq!(cycle, &order[..8]);
            assert_eq!(
                cycle.iter().copied().collect::<BTreeSet<_>>(),
                (0..8).collect()
            );
        }
        order
    }
    assert_eq!(
        run(ManualTaskScheduling::default()),
        (0..8).cycle().take(32).collect::<Vec<_>>()
    );
    assert_eq!(run(policy(0)), run(policy(0)));
    assert_ne!(run(policy(0)), run(policy(1)));
    assert_ne!(run(policy(1)), run(policy(u64::MAX)));
}

#[test]
fn retirement_and_stale_wakes_do_not_admit_replacement_early() {
    for seed in [0, 1, u64::MAX] {
        let mut driver = super::super::tests::driver();
        let mut runner = ManualTaskRunner::new_with_scheduling(
            &mut driver,
            NonZeroUsize::new(1).unwrap(),
            policy(seed),
        );
        let saved = Rc::new(RefCell::new(None));
        let capture = saved.clone();
        let old = runner
            .insert(poll_fn(move |cx| {
                *capture.borrow_mut() = Some(cx.waker().clone());
                Poll::<()>::Pending
            }))
            .unwrap();
        assert!(matches!(
            runner.poll_next().unwrap(),
            ManualTaskPoll::Pending { .. }
        ));
        assert_eq!(
            runner.cancel(old).unwrap(),
            ManualTaskCancellation::Cancelled
        );
        let new = runner.insert(std::future::pending()).unwrap();
        assert_ne!(new, old);
        assert_eq!(
            runner.poll_next().unwrap(),
            ManualTaskPoll::Pending { task: new }
        );
        saved.borrow().as_ref().unwrap().wake_by_ref();
        assert_eq!(runner.poll_next().unwrap(), ManualTaskPoll::Idle);
        assert_eq!(ReadyTasks::lock(&runner.ready).counts(), (1, 0));
        runner.cancel(new).unwrap();
        let completed = runner.insert(async {}).unwrap();
        assert_eq!(
            runner.poll_next().unwrap(),
            ManualTaskPoll::Completed {
                task: completed,
                output: ()
            }
        );
        assert_eq!(ReadyTasks::lock(&runner.ready).counts(), (0, 0));
    }
}

#[test]
fn sparse_churn_matches_a_scan_reference_without_retaining_retired_ids() {
    // The reference deliberately scans a vector, independent of the ordered index.
    // Include extreme ordinals: no signed/native-word arithmetic is permitted.
    for scheduling in [
        ManualTaskScheduling::Cyclic,
        policy(0),
        policy(7),
        policy(u64::MAX),
    ] {
        let mut ready = ReadyTasks::new(scheduling);
        let mut live = Vec::new();
        let mut queued = Vec::new();
        let mut last = None;
        for step in 0..4096u64 {
            let id = ManualTaskId(if step % 2 == 0 { step } else { u64::MAX - step });
            ready.insert(id);
            live.push(id);
            queued.push(id);
            if step % 3 == 0 {
                let retired = live.remove(0);
                ready.retire(retired);
                queued.retain(|&id| id != retired);
            }
            if step % 4 != 0 {
                continue;
            }
            queued.sort_by_key(|id| (scheduling.rank(id.0), *id));
            let expected = queued
                .iter()
                .copied()
                .find(|id| last.is_none_or(|last| (scheduling.rank(id.0), *id) > last))
                .or_else(|| queued.first().copied());
            assert_eq!(ready.pop_next(), expected);
            if let Some(id) = expected {
                last = Some((scheduling.rank(id.0), id));
                queued.retain(|&queued| queued != id);
            }
            assert_eq!(ready.counts(), (live.len(), queued.len()));
        }
        for id in live {
            ready.retire(id);
        }
        assert_eq!(ready.counts(), (0, 0));
        assert_eq!(ready.pop_next(), None);
    }
}

#[test]
fn sparse_wake_storms_stay_bounded_across_large_actor_cancellation() {
    for seed in [0, 7, u64::MAX] {
        let mut driver = super::super::tests::driver();
        let mut runner = ManualTaskRunner::new_with_scheduling(
            &mut driver,
            NonZeroUsize::new(1024).unwrap(),
            policy(seed),
        );
        let saved = Rc::new(RefCell::new(Vec::new()));
        let mut ids = Vec::new();
        for ordinal in 0..1024 {
            let saved = saved.clone();
            ids.push(
                runner
                    .insert(poll_fn(move |cx| {
                        saved.borrow_mut().push((ordinal, cx.waker().clone()));
                        Poll::<()>::Pending
                    }))
                    .unwrap(),
            );
        }
        for _ in 0..1024 {
            runner.poll_next().unwrap();
        }
        assert_eq!(ReadyTasks::lock(&runner.ready).counts(), (1024, 0));
        let wakers = std::mem::take(&mut *saved.borrow_mut());
        for (ordinal, &id) in ids.iter().enumerate() {
            if ordinal % 127 != 0 {
                runner.cancel(id).unwrap();
            }
        }
        for (_, waker) in &wakers {
            for _ in 0..10 {
                waker.wake_by_ref();
            }
        }
        assert_eq!(ReadyTasks::lock(&runner.ready).counts(), (9, 9));
        let mut polled = BTreeSet::new();
        for _ in 0..9 {
            match runner.poll_next().unwrap() {
                ManualTaskPoll::Pending { task } => {
                    assert!(polled.insert(task));
                }
                other => panic!("expected one survivor poll, got {other:?}"),
            }
        }
        assert_eq!(polled, ids.iter().step_by(127).copied().collect());
        assert_eq!(runner.poll_next().unwrap(), ManualTaskPoll::Idle);
        assert_eq!(ReadyTasks::lock(&runner.ready).counts(), (9, 0));
    }
}

#[test]
fn seeded_ready_actors_still_prevent_clock_jumps() {
    use crate::{ManualTimeError, SimulationTick};
    let mut driver = super::super::tests::driver();
    let mut runner = ManualTaskRunner::new_with_scheduling(
        &mut driver,
        NonZeroUsize::new(1).unwrap(),
        policy(7),
    );
    runner
        .insert(async { tokio::task::yield_now().await })
        .unwrap();
    let initial = runner.snapshot().unwrap();
    let deadline = SimulationTick::from_ticks(10);
    for _ in 0..2 {
        assert!(matches!(
            runner.advance_to_next_event(deadline),
            Err(ManualTimeError::ReadyTasks { count: 1 })
        ));
        assert_eq!(runner.snapshot().unwrap(), initial);
        runner.poll_next().unwrap();
    }
    assert_eq!(runner.poll_next().unwrap(), ManualTaskPoll::Idle);
    assert!(runner.advance_to_next_event(deadline).is_ok());
}
