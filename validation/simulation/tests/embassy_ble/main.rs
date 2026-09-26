#![cfg(feature = "controlled-time")]
#![allow(clippy::unwrap_used)]

use std::cell::Cell;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::rc::Rc;
use std::task::Poll;
use std::time::Duration;

use personal_rns::interfaces::bluetooth_auto::{
    AdvertisingMode, BleBackend, Endpoint, Esp32Host, Nrf52Host, RadioMode,
};
use personal_rns::interfaces::{ConnectionState, InterfaceStatus};
use prns_interfaces_embassy::bluetooth_auto::BluetoothRecoveryCounters;
use prns_runtime_embassy::manifold::driver::InterfaceLifecycle;
use prns_simulation::{ManualMedium, ManualTimeDriver, ManualTimeError, SimulationTick};

mod clock;
mod echo;
mod fixture;
mod interop;
mod node;
mod packet_limits;
mod resources;
mod tokio_node;
mod traffic;
use clock::{ClockLease, CompletionBudget, EmbassyTasks};
use fixture::{backend, lab, supervisor, MAX_PEERS};

fn tick(milliseconds: u64) -> SimulationTick {
    SimulationTick::from_ticks(milliseconds)
}

#[test]
fn silent_handshake_expires_at_the_embassy_deadline_without_wall_time_or_forced_polls() {
    for endpoint in [
        Endpoint::Esp32(Esp32Host::Esp32),
        Endpoint::Nrf52(Nrf52Host::Nrf52),
    ] {
        let clock = ClockLease::acquire();
        let lab = lab();
        let (supervisor, fleet, lifecycle) = supervisor(&lab, 1, endpoint);
        let status = supervisor.status();
        let mut remote = backend(&lab, 2);
        let mut driver =
            ManualTimeDriver::new(ManualMedium::Ble(lab.clone()), Duration::from_millis(1))
                .unwrap();
        assert_eq!(
            driver
                .poll(pin!(async {
                    BleBackend::<MAX_PEERS>::set_radio_mode(&mut remote, RadioMode::On)
                        .await
                        .unwrap();
                    BleBackend::<MAX_PEERS>::set_advertising(&mut remote, AdvertisingMode::On)
                        .await
                        .unwrap();
                }))
                .unwrap(),
            Poll::Ready(())
        );
        let mut tasks = EmbassyTasks::new(&mut driver, clock);
        tasks.insert(supervisor.run(fleet));
        assert!(tasks.settle() > 0);
        assert_eq!(lab.active_connection_count(), 0);
        let _ = tasks.advance(tick(9_999)).unwrap();
        assert_eq!(tasks.snapshot().tick, SimulationTick::ZERO);
        assert!(tasks.settle() > 0);
        assert_eq!(lab.active_connection_count(), 1);

        let _ = tasks.advance(tick(9_999)).unwrap();
        assert_eq!(tasks.settle(), 0);
        assert_eq!(
            (lab.active_connection_count(), status.setup_failure_events()),
            (1, 0)
        );
        let _ = tasks.advance(tick(10_000)).unwrap();
        assert!(tasks.settle() > 0);
        assert_eq!(
            (
                lab.active_connection_count(),
                status.members().count(),
                status.connection(),
                status.recovery_counters()
            ),
            (
                0,
                0,
                ConnectionState::Reconnecting,
                BluetoothRecoveryCounters {
                    ingress_pressure: 0,
                    setup_failures: 1,
                    transport_closures: 0
                },
            )
        );
        assert!(lifecycle.try_receive().is_err());
        assert_eq!(tasks.snapshot().runtime_elapsed, Duration::from_secs(10));
        status.disable();
        assert!(tasks.settle() > 0);
        status.enable();
        assert!(tasks.settle() > 0);
        assert_eq!(status.setup_failure_events(), 1);
        drop(tasks);
        assert_eq!(lab.active_connection_count(), 0);
    }
}

#[test]
fn real_embassy_supervisors_admit_one_peer_and_disable_cleans_both_ends() {
    let clock = ClockLease::acquire();
    let lab = lab();
    let (first, first_fleet, first_events) = supervisor(&lab, 1, Endpoint::Esp32(Esp32Host::Esp32));
    let (second, second_fleet, second_events) =
        supervisor(&lab, 2, Endpoint::Nrf52(Nrf52Host::Nrf52));
    let first_status = first.status();
    let second_status = second.status();
    let mut driver =
        ManualTimeDriver::new(ManualMedium::Ble(lab.clone()), Duration::from_millis(1)).unwrap();
    let mut tasks = EmbassyTasks::new(&mut driver, clock);
    tasks.insert(first.run(first_fleet));
    tasks.insert(second.run(second_fleet));
    assert!(tasks.settle() > 0);
    let _ = tasks.advance(tick(0)).unwrap();
    assert!(tasks.settle() > 0);
    assert_eq!(
        (
            lab.active_connection_count(),
            first_status.members().count(),
            second_status.members().count()
        ),
        (1, 1, 1)
    );
    let mut ids = Vec::new();
    for (events, status) in [(first_events, first_status), (second_events, second_status)] {
        let InterfaceLifecycle::Add { descriptor } = events.try_receive().unwrap() else {
            unreachable!("one admitted member")
        };
        assert_eq!(
            status
                .members()
                .map(InterfaceStatus::id)
                .collect::<Vec<_>>(),
            vec![descriptor.id]
        );
        assert!(events.try_receive().is_err());
        ids.push(descriptor.id);
    }
    first_status.disable();
    assert!(tasks.settle() > 0);
    assert_eq!(
        (
            lab.active_connection_count(),
            first_status.members().count(),
            second_status.members().count()
        ),
        (0, 0, 0)
    );
    for (events, id) in [first_events, second_events].into_iter().zip(ids) {
        assert!(
            matches!(events.try_receive(), Ok(InterfaceLifecycle::Remove { id: removed }) if removed == id)
        );
        assert!(events.try_receive().is_err());
    }
    assert_eq!(tasks.snapshot().runtime_elapsed, Duration::ZERO);
    drop(tasks);
    assert_eq!(lab.active_connection_count(), 0);
}

#[test]
fn refused_steps_preserve_all_clocks_and_clock_leases_reset_between_scenarios() {
    struct ObserveDrop(Rc<Cell<Option<embassy_time::Instant>>>);
    impl Drop for ObserveDrop {
        fn drop(&mut self) {
            self.0.set(Some(embassy_time::Instant::now()));
        }
    }

    for _ in 0..2 {
        let clock = ClockLease::acquire();
        let lab = lab();
        let mut driver =
            ManualTimeDriver::new(ManualMedium::Ble(lab), Duration::from_millis(1)).unwrap();
        let mut tasks = EmbassyTasks::new(&mut driver, clock);
        let dropped_at = Rc::new(Cell::new(None));
        let observer = ObserveDrop(dropped_at.clone());
        tasks.insert(async move {
            let _observer = observer;
            embassy_time::Timer::after_secs(1).await;
            std::future::pending().await
        });
        let before = tasks.snapshot();
        assert!(matches!(
            tasks.advance(tick(1)),
            Err(ManualTimeError::ReadyTasks { count: 1 })
        ));
        assert_eq!(tasks.snapshot(), before);
        assert_eq!(tasks.settle(), 1);
        let _ = tasks.advance(tick(1)).unwrap();
        let before = tasks.snapshot();
        assert_eq!(before.runtime_elapsed, Duration::from_millis(1));
        assert!(
            matches!(tasks.advance(tick(0)), Err(ManualTimeError::BeforeCurrent { current, requested }) if current == tick(1) && requested == tick(0))
        );
        assert_eq!(tasks.snapshot(), before);
        drop(tasks);
        assert_eq!(
            dropped_at.get(),
            Some(embassy_time::Instant::from_millis(1))
        );
    }
}

#[test]
fn bounded_completion_drives_timers_without_relaxing_timeless_completion() {
    let clock = ClockLease::acquire();
    let mut driver =
        ManualTimeDriver::new(ManualMedium::Ble(lab()), Duration::from_millis(1)).unwrap();
    let mut tasks = EmbassyTasks::new(&mut driver, clock);
    assert_eq!(
        tasks.complete_with_budget(
            CompletionBudget {
                deadline: tick(10),
                polls_per_tick: NonZeroUsize::new(128).unwrap()
            },
            async {
                embassy_time::Timer::at(embassy_time::Instant::from_millis(7)).await;
                42
            }
        ),
        42
    );
    let at_seven = prns_simulation::ManualTimeSnapshot {
        tick: tick(7),
        runtime_elapsed: Duration::from_millis(7),
    };
    assert_eq!(tasks.snapshot(), at_seven);
    assert_eq!(
        tasks.complete_ready(async {
            tokio::task::yield_now().await;
            43
        }),
        43
    );
    assert_eq!(tasks.snapshot(), at_seven);
    tasks.complete_with_budget(
        CompletionBudget {
            deadline: tick(10),
            polls_per_tick: NonZeroUsize::new(128).unwrap(),
        },
        async {
            embassy_time::Timer::at(embassy_time::Instant::from_millis(10)).await;
        },
    );
    assert_eq!(
        tasks.snapshot(),
        prns_simulation::ManualTimeSnapshot {
            tick: tick(10),
            runtime_elapsed: Duration::from_millis(10),
        }
    );
    let late = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        tasks.complete_with_budget(
            CompletionBudget {
                deadline: tick(12),
                polls_per_tick: NonZeroUsize::new(128).unwrap(),
            },
            async {
                embassy_time::Timer::at(embassy_time::Instant::from_millis(13)).await;
            },
        );
    }));
    assert!(late.is_err());
    assert_eq!(
        tasks.snapshot(),
        prns_simulation::ManualTimeSnapshot {
            tick: tick(12),
            runtime_elapsed: Duration::from_millis(12),
        }
    );
}

#[test]
#[should_panic(expected = "per-tick poll budget")]
fn a_self_waking_operation_cannot_spend_the_future_ticks_poll_budget() {
    let clock = ClockLease::acquire();
    let mut driver =
        ManualTimeDriver::new(ManualMedium::Ble(lab()), Duration::from_millis(1)).unwrap();
    let mut tasks = EmbassyTasks::new(&mut driver, clock);
    tasks.complete_with_budget(
        CompletionBudget {
            deadline: tick(10),
            polls_per_tick: NonZeroUsize::new(128).unwrap(),
        },
        async {
            std::future::poll_fn(|cx| {
                assert_eq!(
                    embassy_time::Instant::now(),
                    embassy_time::Instant::from_millis(0)
                );
                cx.waker().wake_by_ref();
                Poll::<()>::Pending
            })
            .await;
        },
    );
}
