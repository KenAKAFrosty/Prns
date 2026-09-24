use super::*;
use prns_simulation::{
    FaultPlan, ManualMedium, TopologyConfig, VirtualMedium, VirtualMediumConfig,
};
use std::time::Duration;

fn clock() -> ManualTimeDriver {
    ManualTimeDriver::new(
        ManualMedium::Frames(VirtualMedium::new(
            VirtualMediumConfig::new(
                TopologyConfig::FullyConnected,
                1,
                1,
                1,
                32,
                FaultPlan::none(),
            )
            .unwrap(),
        )),
        Duration::from_millis(1),
    )
    .unwrap()
}

#[test]
fn wake_after_idle_completes_without_advancing_either_clock() {
    use std::{cell::RefCell, rc::Rc, task::Poll};

    let lease = ClockLease::acquire();
    let mut driver = clock();
    let mut tasks = EmbassyTasks::new(&mut driver, lease);
    let before = tasks.snapshot();
    let wake = Rc::new(RefCell::new(None::<std::task::Waker>));
    let captured = wake.clone();
    let mut polled = false;
    let operation = std::future::poll_fn(move |context| {
        if polled {
            return Poll::Ready(Instant::now().as_millis());
        }
        polled = true;
        *captured.borrow_mut() = Some(context.waker().clone());
        Poll::Pending
    });
    let mut wakes = 0;
    let completed_at = tasks.complete_with_budget_before_advance(
        CompletionBudget {
            deadline: SimulationTick::from_ticks(10),
            polls_per_tick: NonZeroUsize::new(4).unwrap(),
        },
        operation,
        || {
            wakes += 1;
            wake.borrow().as_ref().unwrap().wake_by_ref();
        },
    );
    assert_eq!((completed_at, wakes), (0, 1));
    assert_eq!(tasks.snapshot(), before);
}

#[test]
#[should_panic(expected = "operation failed to yield within its per-tick poll budget")]
fn repeated_wakes_after_idle_still_exhaust_the_per_tick_poll_budget() {
    use std::{cell::RefCell, rc::Rc, task::Poll};

    let lease = ClockLease::acquire();
    let mut driver = clock();
    let mut tasks = EmbassyTasks::new(&mut driver, lease);
    let wake = Rc::new(RefCell::new(None::<std::task::Waker>));
    let captured = wake.clone();
    let operation = std::future::poll_fn(move |context| {
        *captured.borrow_mut() = Some(context.waker().clone());
        Poll::<()>::Pending
    });
    tasks.complete_with_budget_before_advance(
        CompletionBudget {
            deadline: SimulationTick::from_ticks(10),
            polls_per_tick: NonZeroUsize::new(4).unwrap(),
        },
        operation,
        || {
            assert_eq!(Instant::now().as_millis(), 0);
            wake.borrow().as_ref().unwrap().wake_by_ref();
        },
    );
}

#[test]
fn distinct_waiters_beyond_the_old_capacity_sleep_until_their_deadline() {
    const WAITERS: usize = 128;
    let lease = ClockLease::acquire();
    let mut driver = clock();
    let mut tasks = EmbassyTasks::with_limits(
        &mut driver,
        lease,
        NonZeroUsize::new(WAITERS).unwrap(),
        NonZeroUsize::new(WAITERS * 4).unwrap(),
        prns_simulation::ManualTaskScheduling::Cyclic,
    );
    let completed = std::rc::Rc::new(std::cell::Cell::new(0));
    for _ in 0..WAITERS {
        let completed = completed.clone();
        tasks.insert(async move {
            embassy_time::Timer::after_millis(50).await;
            assert_eq!(Instant::now().as_millis(), 50);
            completed.set(completed.get() + 1);
            std::future::pending::<()>().await;
        });
    }
    tasks.settle();
    assert_eq!(completed.get(), 0);
    assert_eq!(
        tasks.timer_stats(),
        QueueStats {
            pending: WAITERS,
            peak: WAITERS,
            capacity: 1024
        }
    );
    tasks
        .advance_to_next_wake(SimulationTick::from_ticks(86_400_000))
        .unwrap();
    assert_eq!(tasks.snapshot().tick, SimulationTick::from_ticks(50));
    tasks.settle();
    assert_eq!(completed.get(), WAITERS);
    assert_eq!(
        tasks.timer_stats(),
        QueueStats {
            pending: 0,
            peak: WAITERS,
            capacity: 1024
        }
    );
}

#[test]
fn embassy_and_tokio_timers_rearm_on_one_day_long_timeline() {
    let lease = ClockLease::acquire();
    let mut driver = clock();
    let mut tasks = EmbassyTasks::new(&mut driver, lease);
    let hours = tasks.complete_with_budget(
        CompletionBudget {
            deadline: SimulationTick::from_ticks(86_400_000),
            polls_per_tick: NonZeroUsize::new(128).unwrap(),
        },
        async {
            for hour in 1..=24 {
                embassy_time::Timer::after_secs(1800).await;
                assert_eq!(
                    Instant::now().as_millis(),
                    (hour - 1) * 3_600_000 + 1_800_000
                );
                tokio::time::sleep(Duration::from_secs(1800)).await;
                assert_eq!(Instant::now().as_millis(), hour * 3_600_000);
            }
            24
        },
    );
    assert_eq!(hours, 24);
    assert_eq!(
        tasks.snapshot().runtime_elapsed,
        Duration::from_secs(86_400)
    );
}

#[test]
fn submillisecond_embassy_deadline_is_refused_before_clock_movement() {
    let lease = ClockLease::acquire();
    let mut driver = clock();
    let mut tasks = EmbassyTasks::new(&mut driver, lease);
    tasks.insert(async { embassy_time::Timer::after_micros(1).await });
    tasks.settle();
    let before = tasks.snapshot();
    assert!(matches!(
        tasks.advance_to_next_wake(SimulationTick::from_ticks(10)),
        Err(ClockAdvanceError::SubmillisecondDeadline { micros: 1 })
    ));
    assert_eq!(tasks.snapshot(), before);
}

#[test]
fn concurrent_runtime_timers_observe_both_clocks_at_their_own_deadlines() {
    let lease = ClockLease::acquire();
    let mut driver = clock();
    let mut tasks = EmbassyTasks::new(&mut driver, lease);
    let observed = tasks.complete_with_budget(
        CompletionBudget {
            deadline: SimulationTick::from_ticks(100),
            polls_per_tick: NonZeroUsize::new(128).unwrap(),
        },
        async {
            tokio::join!(
                async {
                    embassy_time::Timer::after_millis(10).await;
                    Instant::now().as_millis()
                },
                async {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    Instant::now().as_millis()
                },
            )
        },
    );
    assert_eq!(observed, (10, 5));
    assert_eq!(tasks.snapshot().tick, SimulationTick::from_ticks(10));
}

#[test]
fn production_embassy_supervisor_handshake_deadline_bounds_a_day_long_horizon() {
    use personal_rns::interfaces::bluetooth_auto::{
        AdvertisingMode, BleBackend, Endpoint, Esp32Host, RadioMode,
    };
    let lease = ClockLease::acquire();
    let lab = super::super::fixture::lab();
    let (supervisor, fleet, _lifecycle) =
        super::super::fixture::supervisor(&lab, 1, Endpoint::Esp32(Esp32Host::Esp32));
    let status = supervisor.status();
    let mut remote = super::super::fixture::backend(&lab, 2);
    let mut driver =
        ManualTimeDriver::new(ManualMedium::Ble(lab.clone()), Duration::from_millis(1)).unwrap();
    let mut tasks = EmbassyTasks::new(&mut driver, lease);
    let _remote = tasks.complete_ready(async move {
        BleBackend::<2>::set_radio_mode(&mut remote, RadioMode::On)
            .await
            .unwrap();
        BleBackend::<2>::set_advertising(&mut remote, AdvertisingMode::On)
            .await
            .unwrap();
        remote
    });
    tasks.insert(supervisor.run(fleet));
    for step in 0..8 {
        tasks.settle();
        if status.setup_failure_events() == 1 {
            break;
        }
        assert!(step < 7, "bounded discovery and handshake steps");
        tasks
            .advance_to_next_wake(SimulationTick::from_ticks(86_400_000))
            .unwrap();
    }
    assert_eq!(tasks.snapshot().tick, SimulationTick::from_ticks(10_000));
    assert_eq!(status.setup_failure_events(), 1);
    assert_eq!(lab.active_connection_count(), 0);
}
