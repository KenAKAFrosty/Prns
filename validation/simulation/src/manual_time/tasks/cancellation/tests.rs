use std::cell::{Cell, RefCell};
use std::future::{pending, poll_fn};
use std::num::NonZeroUsize;
use std::rc::Rc;
use std::task::{Poll, Waker};
use std::time::Duration;

use super::*;
use crate::{
    FaultPlan, ManualMedium, ManualTaskAdmissionError, ManualTaskPoll, ManualTimeDriver,
    ManualTimeSnapshot, SimulationTick, TopologyConfig, VirtualMedium, VirtualMediumConfig,
};

fn capacity(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).unwrap_or_else(|| unreachable!("nonzero test bound"))
}

fn medium() -> VirtualMedium {
    VirtualMedium::new(
        VirtualMediumConfig::new(
            TopologyConfig::FullyConnected,
            2,
            2,
            2,
            32,
            FaultPlan::none(),
        )
        .unwrap_or_else(|error| unreachable!("valid medium: {error}")),
    )
}

fn driver(medium: VirtualMedium) -> ManualTimeDriver {
    ManualTimeDriver::new(ManualMedium::Frames(medium), Duration::from_millis(1))
        .unwrap_or_else(|error| unreachable!("manual clock: {error}"))
}

fn admit<T>(
    runner: &mut ManualTaskRunner<'_, T>,
    future: impl std::future::Future<Output = T> + 'static,
) -> ManualTaskId {
    runner
        .insert(future)
        .unwrap_or_else(|error| unreachable!("admitted actor: {error}"))
}

fn poll<T>(runner: &mut ManualTaskRunner<'_, T>) -> ManualTaskPoll<T> {
    runner
        .poll_next()
        .unwrap_or_else(|error| unreachable!("poll: {error}"))
}

fn cancel<T>(runner: &mut ManualTaskRunner<'_, T>, task: ManualTaskId) -> ManualTaskCancellation {
    runner
        .cancel(task)
        .unwrap_or_else(|error| unreachable!("cancel: {error}"))
}

struct Dropped(Rc<Cell<usize>>);

#[test]
fn dropping_runner_retires_all_wakes_and_preserves_destructor_runtime_context() {
    let mut driver = driver(medium());
    let mut runner = ManualTaskRunner::new(&mut driver, capacity(2));
    let ready = runner.ready.clone();
    let mut observations = Vec::new();
    for _ in 0..2 {
        let saved = Rc::new(RefCell::new(None));
        let dropped_at = Rc::new(Cell::new(None));
        let guard = WakeOnDrop {
            saved: saved.clone(),
            dropped_at: dropped_at.clone(),
        };
        let captured = saved.clone();
        let task = admit(&mut runner, async move {
            let _guard = guard;
            poll_fn(move |cx| {
                *captured.borrow_mut() = Some(cx.waker().clone());
                Poll::<()>::Pending
            })
            .await;
        });
        assert_eq!(poll(&mut runner), ManualTaskPoll::Pending { task });
        observations.push((saved, dropped_at));
    }
    let expected_at = {
        let _entered = runner.driver.runtime.enter();
        tokio::time::Instant::now()
    };
    drop(runner);
    for (saved, dropped_at) in observations {
        assert_eq!(dropped_at.get(), Some(expected_at));
        saved.borrow().as_ref().unwrap().wake_by_ref();
    }
    assert_eq!(ReadyTasks::lock(&ready).counts(), (0, 0));
    assert!(driver.snapshot().is_ok());
}

impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

#[test]
fn cancellation_before_first_poll_drops_once_without_output_and_reuses_capacity() {
    let mut driver = driver(medium());
    let mut runner = ManualTaskRunner::new(&mut driver, capacity(1));
    let count = Rc::new(Cell::new(0));
    let guard = Dropped(count.clone());
    let task = admit(&mut runner, async move {
        let _guard = guard;
        unreachable!("cancelled before any poll")
    });
    let before = runner.snapshot().ok();
    assert_eq!(cancel(&mut runner, task), ManualTaskCancellation::Cancelled);
    assert_eq!((count.get(), runner.task_count()), (1, 0));
    assert_eq!(poll::<()>(&mut runner), ManualTaskPoll::Idle);
    assert_eq!(cancel(&mut runner, task), ManualTaskCancellation::NotLive);
    assert_eq!(runner.snapshot().ok(), before);
    let replacement = admit(&mut runner, async {});
    assert_ne!(replacement, task);
    assert_eq!(
        poll(&mut runner),
        ManualTaskPoll::Completed {
            task: replacement,
            output: ()
        }
    );
    assert_eq!(
        cancel(&mut runner, replacement),
        ManualTaskCancellation::NotLive
    );
    assert_eq!(count.get(), 1);
}

struct WakeOnDrop {
    saved: Rc<RefCell<Option<Waker>>>,
    dropped_at: Rc<Cell<Option<tokio::time::Instant>>>,
}

impl Drop for WakeOnDrop {
    fn drop(&mut self) {
        assert!(tokio::runtime::Handle::try_current().is_ok());
        self.dropped_at.set(Some(tokio::time::Instant::now()));
        self.saved
            .borrow()
            .as_ref()
            .unwrap_or_else(|| unreachable!("polled actor"))
            .wake_by_ref();
    }
}

#[test]
fn ready_actor_is_retired_before_its_destructor_wakes_it_and_stale_wakes_stay_dead() {
    let mut driver = driver(medium());
    let mut runner = ManualTaskRunner::new(&mut driver, capacity(1));
    let saved = Rc::new(RefCell::new(None));
    let dropped_at = Rc::new(Cell::new(None));
    let guard = WakeOnDrop {
        saved: saved.clone(),
        dropped_at: dropped_at.clone(),
    };
    let captured = saved.clone();
    let task = admit(&mut runner, async move {
        let _guard = guard;
        poll_fn(move |cx| {
            *captured.borrow_mut() = Some(cx.waker().clone());
            cx.waker().wake_by_ref();
            Poll::<()>::Pending
        })
        .await;
    });
    assert_eq!(poll(&mut runner), ManualTaskPoll::Pending { task });
    let expected_at = {
        let _entered = runner.driver.runtime.enter();
        tokio::time::Instant::now()
    };
    assert_eq!(cancel(&mut runner, task), ManualTaskCancellation::Cancelled);
    assert_eq!(dropped_at.get(), Some(expected_at));
    let replacement = admit(&mut runner, pending::<()>());
    assert_eq!(
        poll(&mut runner),
        ManualTaskPoll::Pending { task: replacement }
    );
    for _ in 0..32 {
        saved
            .borrow()
            .as_ref()
            .unwrap_or_else(|| unreachable!("saved wake"))
            .wake_by_ref();
    }
    assert_eq!(poll(&mut runner), ManualTaskPoll::Idle);
    assert_eq!(ReadyTasks::lock(&runner.ready).counts(), (1, 0));
}

#[test]
fn cancelling_a_timer_does_not_wake_a_replacement_at_the_old_deadline() {
    let mut driver = driver(medium());
    let mut runner = ManualTaskRunner::new(&mut driver, capacity(1));
    let task = admit(&mut runner, async {
        tokio::time::sleep(Duration::from_millis(10)).await
    });
    assert_eq!(poll(&mut runner), ManualTaskPoll::Pending { task });
    assert_eq!(cancel(&mut runner, task), ManualTaskCancellation::Cancelled);
    let replacement = admit(&mut runner, pending::<()>());
    assert_eq!(
        poll(&mut runner),
        ManualTaskPoll::Pending { task: replacement }
    );
    assert!(runner
        .advance_to_next_event(SimulationTick::from_ticks(10))
        .is_ok());
    assert_eq!(poll(&mut runner), ManualTaskPoll::Idle);
    assert_eq!(
        runner.snapshot().ok(),
        Some(ManualTimeSnapshot {
            tick: SimulationTick::from_ticks(10),
            runtime_elapsed: Duration::from_millis(10),
        })
    );
}

#[test]
fn destructor_channel_closure_wakes_the_survivor_without_polling_the_cancelled_actor() {
    let mut driver = driver(medium());
    let mut runner = ManualTaskRunner::new(&mut driver, capacity(2));
    let (sender, receiver) = tokio::sync::oneshot::channel::<()>();
    let owner = admit(&mut runner, async move {
        let _sender = sender;
        pending::<u8>().await
    });
    let survivor = admit(&mut runner, async move {
        assert!(receiver.await.is_err());
        7
    });
    assert_eq!(poll(&mut runner), ManualTaskPoll::Pending { task: owner });
    assert_eq!(
        poll(&mut runner),
        ManualTaskPoll::Pending { task: survivor }
    );
    assert_eq!(poll(&mut runner), ManualTaskPoll::Idle);
    assert_eq!(
        cancel(&mut runner, owner),
        ManualTaskCancellation::Cancelled
    );
    assert!(matches!(
        runner.advance_to_next_event(SimulationTick::from_ticks(1)),
        Err(ManualTimeError::ReadyTasks { count: 1 })
    ));
    assert_eq!(
        poll(&mut runner),
        ManualTaskPoll::Completed {
            task: survivor,
            output: 7
        }
    );
}

#[test]
fn medium_drift_refuses_cancellation_before_mutating_actor_ownership() {
    let medium = medium();
    let mut driver = driver(medium.clone());
    let mut runner = ManualTaskRunner::new(&mut driver, capacity(1));
    let count = Rc::new(Cell::new(0));
    let guard = Dropped(count.clone());
    let task = admit(&mut runner, async move {
        let _guard = guard;
        pending::<()>().await
    });
    assert!(medium.advance_to(SimulationTick::from_ticks(1)).is_ok());
    assert!(matches!(
        runner.cancel(task),
        Err(ManualTimeError::MediumDrift { .. })
    ));
    assert_eq!((count.get(), runner.task_count()), (0, 1));
    assert_eq!(ReadyTasks::lock(&runner.ready).counts(), (1, 1));
    drop(runner);
    assert_eq!(count.get(), 1);
}

struct SpawnOnDrop;

impl Drop for SpawnOnDrop {
    fn drop(&mut self) {
        drop(tokio::spawn(pending::<()>()));
    }
}

#[test]
fn destructor_spawn_is_refused_after_irreversible_removal() {
    let mut driver = driver(medium());
    let mut runner = ManualTaskRunner::new(&mut driver, capacity(1));
    let guard = SpawnOnDrop;
    let task = admit(&mut runner, async move {
        let _guard = guard;
        pending::<()>().await
    });
    assert!(matches!(
        runner.cancel(task),
        Err(ManualTimeError::SpawnedTasks { count: 1 })
    ));
    assert_eq!(runner.task_count(), 0);
    assert_eq!(ReadyTasks::lock(&runner.ready).counts(), (0, 0));
}

#[test]
fn sparse_cancellation_preserves_cyclic_order_and_bounded_ready_storage_at_scale() {
    const ACTORS: usize = 1024;
    let mut driver = driver(medium());
    let mut runner = ManualTaskRunner::new(&mut driver, capacity(ACTORS));
    let saved = Rc::new(RefCell::new(Vec::new()));
    let tasks: Vec<_> = (0..ACTORS)
        .map(|_| {
            let captured = saved.clone();
            admit(
                &mut runner,
                poll_fn(move |cx| {
                    captured.borrow_mut().push(cx.waker().clone());
                    cx.waker().wake_by_ref();
                    Poll::<()>::Pending
                }),
            )
        })
        .collect();
    for &task in &tasks {
        assert_eq!(poll(&mut runner), ManualTaskPoll::Pending { task });
    }
    for &task in tasks.iter().step_by(2) {
        assert_eq!(cancel(&mut runner, task), ManualTaskCancellation::Cancelled);
    }
    for waker in saved.borrow().iter() {
        waker.wake_by_ref();
    }
    assert_eq!(
        ReadyTasks::lock(&runner.ready).counts(),
        (ACTORS / 2, ACTORS / 2)
    );
    for &task in tasks.iter().skip(1).step_by(2) {
        assert_eq!(poll(&mut runner), ManualTaskPoll::Pending { task });
    }
    assert_eq!(runner.task_count(), ACTORS / 2);
    for task in tasks {
        let _ = cancel(&mut runner, task);
    }
    assert_eq!(ReadyTasks::lock(&runner.ready).counts(), (0, 0));
    assert_eq!(poll(&mut runner), ManualTaskPoll::Idle);
}

#[test]
fn cancellation_never_rewinds_exhausted_ordinals() {
    let mut driver = driver(medium());
    let mut runner = ManualTaskRunner::new(&mut driver, capacity(1));
    runner.next_id = Some(u64::MAX);
    let last = admit(&mut runner, pending::<()>());
    assert_eq!(cancel(&mut runner, last), ManualTaskCancellation::Cancelled);
    assert_eq!(
        runner.insert(pending::<()>()),
        Err(ManualTaskAdmissionError::IdsExhausted)
    );
    assert_eq!(runner.task_count(), 0);
}
