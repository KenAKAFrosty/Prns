use std::cell::{Cell, RefCell};
use std::future::{pending, poll_fn};
use std::rc::Rc;
use std::time::Duration;

use super::*;
use crate::{FaultPlan, ManualMedium, TopologyConfig, VirtualMedium, VirtualMediumConfig};

fn capacity(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).unwrap_or_else(|| unreachable!("nonzero test capacity"))
}

fn driver() -> ManualTimeDriver {
    let config = VirtualMediumConfig::new(
        TopologyConfig::FullyConnected,
        2,
        2,
        2,
        32,
        FaultPlan::none(),
    )
    .unwrap_or_else(|error| unreachable!("valid medium: {error}"));
    ManualTimeDriver::new(
        ManualMedium::Frames(VirtualMedium::new(config)),
        Duration::from_millis(1),
    )
    .unwrap_or_else(|error| unreachable!("valid driver: {error}"))
}

fn insert<T, F: Future<Output = T> + 'static>(
    runner: &mut ManualTaskRunner<'_, T>,
    future: F,
) -> ManualTaskId {
    runner
        .insert(future)
        .unwrap_or_else(|error| unreachable!("admission: {error}"))
}

fn poll<T>(runner: &mut ManualTaskRunner<'_, T>) -> ManualTaskPoll<T> {
    runner
        .poll_next()
        .unwrap_or_else(|error| unreachable!("poll: {error}"))
}

#[test]
fn pending_without_a_wake_is_not_polled_again_and_idle_does_not_move_time() {
    let mut driver = driver();
    let mut runner = ManualTaskRunner::new(&mut driver, capacity(1));
    let polls = Rc::new(Cell::new(0));
    let counted = polls.clone();
    let task = insert(
        &mut runner,
        poll_fn(move |_| {
            counted.set(counted.get() + 1);
            Poll::<()>::Pending
        }),
    );
    assert_eq!(poll(&mut runner), ManualTaskPoll::Pending { task });
    for _ in 0..8 {
        assert_eq!(poll(&mut runner), ManualTaskPoll::Idle);
    }
    assert_eq!((polls.get(), runner.task_count()), (1, 1));
    assert_eq!(
        runner.snapshot().ok(),
        Some(ManualTimeSnapshot {
            tick: SimulationTick::ZERO,
            runtime_elapsed: Duration::ZERO,
        })
    );
}

#[test]
fn self_waking_tasks_are_coalesced_and_round_robin_across_single_poll_calls() {
    let mut driver = driver();
    let mut runner = ManualTaskRunner::new(&mut driver, capacity(3));
    let ids: Vec<_> = (0..3)
        .map(|_| {
            insert(
                &mut runner,
                poll_fn(|cx| {
                    for _ in 0..100 {
                        cx.waker().wake_by_ref();
                    }
                    Poll::<()>::Pending
                }),
            )
        })
        .collect();
    for _ in 0..4 {
        for &task in &ids {
            assert_eq!(poll(&mut runner), ManualTaskPoll::Pending { task });
        }
    }
    let ready = ReadyTasks::lock(&runner.ready);
    assert_eq!(ready.counts(), (3, 3));
}

#[test]
fn completion_retires_final_poll_wakes_and_stale_wakers_cannot_reach_a_new_task() {
    let mut driver = driver();
    let mut runner = ManualTaskRunner::new(&mut driver, capacity(1));
    let saved = Rc::new(RefCell::new(None));
    let captured = saved.clone();
    let first = insert(
        &mut runner,
        poll_fn(move |cx| {
            *captured.borrow_mut() = Some(cx.waker().clone());
            cx.waker().wake_by_ref();
            Poll::Ready(17)
        }),
    );
    assert_eq!(
        poll(&mut runner),
        ManualTaskPoll::Completed {
            task: first,
            output: 17
        }
    );
    assert_eq!(runner.task_count(), 0);
    let second = insert(&mut runner, pending::<i32>());
    assert_ne!(first, second);
    assert_eq!(poll(&mut runner), ManualTaskPoll::Pending { task: second });
    let stale = saved
        .borrow()
        .as_ref()
        .cloned()
        .unwrap_or_else(|| unreachable!("captured waker"));
    for _ in 0..100 {
        stale.wake_by_ref();
    }
    assert_eq!(poll(&mut runner), ManualTaskPoll::Idle);
    let ready = ReadyTasks::lock(&runner.ready);
    assert_eq!(ready.counts(), (1, 0));
}

#[test]
fn capacity_refusal_does_not_poll_or_consume_an_id_and_exhaustion_never_wraps() {
    let mut driver = driver();
    let mut runner = ManualTaskRunner::new(&mut driver, capacity(1));
    let first = insert(&mut runner, async { 1 });
    assert_eq!(
        runner.insert(poll_fn(|_| -> Poll<i32> {
            unreachable!("rejected future must not be polled")
        })),
        Err(ManualTaskAdmissionError::Capacity {
            maximum: capacity(1)
        })
    );
    assert_eq!(
        poll(&mut runner),
        ManualTaskPoll::Completed {
            task: first,
            output: 1
        }
    );
    let second = insert(&mut runner, async { 2 });
    assert_eq!(second, ManualTaskId(1));
    assert_eq!(
        poll(&mut runner),
        ManualTaskPoll::Completed {
            task: second,
            output: 2
        }
    );
    runner.next_id = Some(u64::MAX);
    let last = insert(&mut runner, async { 3 });
    assert_eq!(last, ManualTaskId(u64::MAX));
    assert_eq!(
        poll(&mut runner),
        ManualTaskPoll::Completed {
            task: last,
            output: 3
        }
    );
    assert_eq!(
        runner.insert(async { 4 }),
        Err(ManualTaskAdmissionError::IdsExhausted)
    );
    assert_eq!(
        (runner.task_count(), poll(&mut runner)),
        (0, ManualTaskPoll::Idle)
    );
}

#[test]
fn cross_thread_wake_during_a_pending_poll_is_not_lost() {
    let mut driver = driver();
    let mut runner = ManualTaskRunner::new(&mut driver, capacity(1));
    let mut first_poll = true;
    let task = insert(
        &mut runner,
        poll_fn(move |cx| {
            if first_poll {
                first_poll = false;
                let waker = cx.waker().clone();
                std::thread::scope(|scope| {
                    scope.spawn(move || waker.wake());
                });
                return Poll::Pending;
            }
            Poll::Ready(23)
        }),
    );
    assert_eq!(poll(&mut runner), ManualTaskPoll::Pending { task });
    assert_eq!(
        poll(&mut runner),
        ManualTaskPoll::Completed { task, output: 23 }
    );
    assert_eq!(poll(&mut runner), ManualTaskPoll::Idle);
}

#[test]
fn one_woken_actor_does_not_repoll_a_thousand_idle_actors() {
    const ACTORS: usize = 1_024;
    const SELECTED: usize = 731;
    let mut driver = driver();
    let mut runner = ManualTaskRunner::new(&mut driver, capacity(ACTORS));
    let counts = Rc::new(RefCell::new(vec![0; ACTORS]));
    let mut senders = Vec::new();
    let mut ids = Vec::new();
    for actor in 0..ACTORS {
        let (sender, mut receiver) = tokio::sync::oneshot::channel();
        senders.push(sender);
        let counts = counts.clone();
        ids.push(insert(
            &mut runner,
            poll_fn(move |cx| {
                counts.borrow_mut()[actor] += 1;
                Pin::new(&mut receiver).poll(cx)
            }),
        ));
    }
    for &task in &ids {
        assert_eq!(poll(&mut runner), ManualTaskPoll::Pending { task });
    }
    assert_eq!(poll(&mut runner), ManualTaskPoll::Idle);
    assert_eq!(senders.remove(SELECTED).send(55), Ok(()));
    assert_eq!(
        poll(&mut runner),
        ManualTaskPoll::Completed {
            task: ids[SELECTED],
            output: Ok(55)
        }
    );
    assert_eq!(poll(&mut runner), ManualTaskPoll::Idle);
    let mut expected = vec![1; ACTORS];
    expected[SELECTED] = 2;
    assert_eq!(*counts.borrow(), expected);
    drop(senders);
    let mut completed = Vec::new();
    for _ in 0..ACTORS - 1 {
        let ManualTaskPoll::Completed {
            task,
            output: Err(_),
        } = poll(&mut runner)
        else {
            unreachable!("every remaining closed channel completes")
        };
        completed.push(task);
    }
    let expected: Vec<_> = ids[SELECTED + 1..]
        .iter()
        .chain(&ids[..SELECTED])
        .copied()
        .collect();
    assert_eq!(completed, expected);
    assert_eq!(
        (runner.task_count(), poll(&mut runner)),
        (0, ManualTaskPoll::Idle)
    );
}

#[test]
fn timers_wake_only_due_actors_without_repolling_sleepers_or_auto_advancing() {
    let mut driver = driver();
    let mut runner = ManualTaskRunner::new(&mut driver, capacity(2));
    let ids: Vec<_> = [5, 10]
        .into_iter()
        .map(|millis| {
            insert(&mut runner, async move {
                tokio::time::sleep(Duration::from_millis(millis)).await;
                millis
            })
        })
        .collect();
    for &task in &ids {
        assert_eq!(poll(&mut runner), ManualTaskPoll::Pending { task });
    }
    assert_eq!(poll(&mut runner), ManualTaskPoll::Idle);
    for boundary in [4, 5, 9, 10] {
        assert!(runner
            .advance_to_next_event(SimulationTick::from_ticks(boundary))
            .is_ok());
        if boundary == 5 || boundary == 10 {
            assert_eq!(
                poll(&mut runner),
                ManualTaskPoll::Completed {
                    task: ids[if boundary == 5 { 0 } else { 1 }],
                    output: boundary,
                }
            );
        }
        assert_eq!(poll(&mut runner), ManualTaskPoll::Idle);
        assert_eq!(
            runner.snapshot().ok(),
            Some(ManualTimeSnapshot {
                tick: SimulationTick::from_ticks(boundary),
                runtime_elapsed: Duration::from_millis(boundary),
            })
        );
    }
}

#[test]
fn dropping_the_runner_drops_futures_and_retained_wakers_do_not_retain_the_scheduler() {
    struct DropCount(Rc<Cell<usize>>);
    impl Drop for DropCount {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    let mut driver = driver();
    let mut runner = ManualTaskRunner::new(&mut driver, capacity(1));
    let drops = Rc::new(Cell::new(0));
    let guard = DropCount(drops.clone());
    let saved = Rc::new(RefCell::new(None));
    let captured = saved.clone();
    let task = insert(
        &mut runner,
        poll_fn(move |cx| {
            let _ = &guard;
            *captured.borrow_mut() = Some(cx.waker().clone());
            Poll::<()>::Pending
        }),
    );
    assert_eq!(poll(&mut runner), ManualTaskPoll::Pending { task });
    let scheduler = Arc::downgrade(&runner.ready);
    drop(runner);
    assert_eq!(drops.get(), 1);
    assert!(scheduler.upgrade().is_none());
    let stale = saved
        .borrow()
        .as_ref()
        .cloned()
        .unwrap_or_else(|| unreachable!("captured waker"));
    stale.wake();
    assert!(driver.snapshot().is_ok());
}

#[test]
fn cooperative_tokio_yields_resume_without_advancing_to_an_unrelated_timer() {
    let mut driver = driver();
    let mut runner = ManualTaskRunner::new(&mut driver, capacity(2));
    let sleeper = insert(&mut runner, async {
        tokio::time::sleep(Duration::from_secs(60)).await;
        1
    });
    let yielding = insert(&mut runner, async {
        tokio::task::yield_now().await;
        2
    });
    assert_eq!(poll(&mut runner), ManualTaskPoll::Pending { task: sleeper });
    assert_eq!(
        poll(&mut runner),
        ManualTaskPoll::Pending { task: yielding }
    );
    assert_eq!(
        poll(&mut runner),
        ManualTaskPoll::Completed {
            task: yielding,
            output: 2
        }
    );
    assert_eq!(poll(&mut runner), ManualTaskPoll::Idle);
    assert_eq!(
        runner.snapshot().ok(),
        Some(ManualTimeSnapshot {
            tick: SimulationTick::ZERO,
            runtime_elapsed: Duration::ZERO,
        })
    );
}

#[test]
fn time_cannot_skip_ready_actors_including_deferred_cooperative_wakes() {
    let mut driver = driver();
    let mut runner = ManualTaskRunner::new(&mut driver, capacity(1));
    let task = insert(&mut runner, async { tokio::task::yield_now().await });
    let initial = runner.snapshot().ok();
    let later = SimulationTick::from_ticks(10);
    assert!(matches!(
        runner.advance_to_next_event(later),
        Err(ManualTimeError::ReadyTasks { count: 1 })
    ));
    assert_eq!(runner.snapshot().ok(), initial);
    assert_eq!(poll(&mut runner), ManualTaskPoll::Pending { task });
    assert!(matches!(
        runner.advance_to_next_event(later),
        Err(ManualTimeError::ReadyTasks { count: 1 })
    ));
    assert_eq!(runner.snapshot().ok(), initial);
    assert_eq!(
        poll(&mut runner),
        ManualTaskPoll::Completed { task, output: () }
    );
    assert!(runner.advance_to_next_event(later).is_ok());
}

#[test]
fn task_polling_preserves_the_drivers_spawned_task_refusal() {
    let mut driver = driver();
    let mut runner = ManualTaskRunner::new(&mut driver, capacity(1));
    insert(&mut runner, async {
        drop(tokio::spawn(pending::<()>()));
    });
    assert!(matches!(
        runner.poll_next(),
        Err(ManualTimeError::SpawnedTasks { count: 1 })
    ));
}
