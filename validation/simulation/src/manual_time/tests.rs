use std::future::pending;
use std::pin::pin;

use personal_rns::engine::InstantMillis;
use personal_rns::manifold::tokio::TokioClock;

use super::*;
use crate::{
    FaultPlan, SimulationDurationInTicks, TopologyConfig, TransmissionOrdinal, TransmissionRule,
    VirtualMediumConfig,
};

fn checked<T>(value: Result<T, ManualTimeError>) -> T {
    value.unwrap_or_else(|error| unreachable!("manual clock operation succeeds: {error}"))
}

fn tick(value: u64) -> SimulationTick {
    SimulationTick::from_ticks(value)
}

fn medium() -> VirtualMedium {
    let faults = FaultPlan::new(vec![TransmissionRule::delay(
        TransmissionOrdinal::new(0),
        SimulationDurationInTicks::from_ticks(5),
    )])
    .unwrap_or_else(|error| unreachable!("canonical fault plan: {error}"));
    VirtualMedium::new(
        VirtualMediumConfig::new(TopologyConfig::FullyConnected, 2, 2, 2, 32, faults)
            .unwrap_or_else(|error| unreachable!("valid medium: {error}")),
    )
}

fn driver(medium: &VirtualMedium) -> ManualTimeDriver {
    checked(ManualTimeDriver::new(
        ManualMedium::Frames(medium.clone()),
        Duration::from_millis(2),
    ))
}

#[test]
fn a_medium_event_and_runtime_deadline_share_a_tick_with_medium_effects_first() {
    let medium = medium();
    let sender = medium
        .attach(b"sender")
        .unwrap_or_else(|error| unreachable!("attach: {error}"));
    let _receiver = medium
        .attach(b"receiver")
        .unwrap_or_else(|error| unreachable!("attach: {error}"));
    let mut driver = driver(&medium);
    assert_eq!(medium.transmit(sender.endpoint_id(), vec![1, 2, 3]), Ok(()));
    let mut waiting = pin!(async {
        let clock = TokioClock::start_at(InstantMillis(100));
        clock.sleep_until(InstantMillis(110)).await;
        (clock.now(), medium.now(), medium.pending_delivery_count())
    });
    assert_eq!(checked(driver.poll(waiting.as_mut())), Poll::Pending);
    assert_eq!(
        checked(driver.snapshot()),
        ManualTimeSnapshot {
            tick: tick(0),
            runtime_elapsed: Duration::ZERO,
        }
    );
    assert_eq!(
        checked(driver.advance_to_next_event(tick(4))),
        ManualAdvance::Frames(AdvanceReport {
            from: tick(0),
            to: tick(4),
            receptions_queued: 0,
            receptions_dropped: 0,
        })
    );
    assert_eq!(checked(driver.poll(waiting.as_mut())), Poll::Pending);
    assert_eq!(
        checked(driver.advance_to_next_event(tick(5))),
        ManualAdvance::Frames(AdvanceReport {
            from: tick(4),
            to: tick(5),
            receptions_queued: 1,
            receptions_dropped: 0,
        })
    );
    assert_eq!(
        checked(driver.poll(waiting.as_mut())),
        Poll::Ready((InstantMillis(110), tick(5), 0))
    );
    assert_eq!(
        checked(driver.snapshot()),
        ManualTimeSnapshot {
            tick: tick(5),
            runtime_elapsed: Duration::from_millis(10),
        }
    );
}

#[test]
fn pending_polls_never_auto_advance_and_nonzero_medium_origins_are_preserved() {
    let medium = medium();
    assert!(medium.advance_to(tick(37)).is_ok());
    let mut driver = driver(&medium);
    let mut waiting = pin!(async { tokio::time::sleep(Duration::from_secs(60)).await });
    for _ in 0..8 {
        assert_eq!(checked(driver.poll(waiting.as_mut())), Poll::Pending);
        assert_eq!(
            checked(driver.snapshot()),
            ManualTimeSnapshot {
                tick: tick(37),
                runtime_elapsed: Duration::ZERO,
            }
        );
    }
    assert!(matches!(
        checked(driver.advance_to_next_event(tick(38))),
        ManualAdvance::Frames(_)
    ));
    assert_eq!(
        checked(driver.snapshot()),
        ManualTimeSnapshot {
            tick: tick(38),
            runtime_elapsed: Duration::from_millis(2),
        }
    );
}

#[test]
fn backward_and_overflow_refusals_leave_both_clocks_and_trace_unchanged() {
    let medium = medium();
    let mut driver = driver(&medium);
    let _ = checked(driver.advance_to_next_event(tick(1)));
    let before = (checked(driver.snapshot()), medium.trace());
    assert!(matches!(driver.advance_to_next_event(tick(0)),
        Err(ManualTimeError::BeforeCurrent { current, requested }) if current == tick(1) && requested == tick(0)));
    assert_eq!((checked(driver.snapshot()), medium.trace()), before);
    assert!(matches!(driver.advance_to_next_event(tick(u64::MAX)),
        Err(ManualTimeError::ClockRange { tick: at }) if at == tick(u64::MAX)));
    assert_eq!((checked(driver.snapshot()), medium.trace()), before);
}

#[test]
fn independently_advanced_medium_time_is_refused_before_runtime_progress() {
    let medium = medium();
    let mut driver = driver(&medium);
    assert!(medium.advance_to(tick(1)).is_ok());
    let before = medium.trace();
    assert!(matches!(driver.advance_to_next_event(tick(5)),
        Err(ManualTimeError::MediumDrift { expected, observed }) if expected == tick(0) && observed == tick(1)));
    assert_eq!(medium.trace(), before);
    let _entered = driver.runtime.enter();
    assert_eq!(Instant::now(), driver.origin_instant);
}

#[test]
fn a_future_cannot_silently_change_the_drivers_clock() {
    let medium = medium();
    let mut driver = driver(&medium);
    assert!(
        matches!(driver.poll(pin!(tokio::time::advance(Duration::from_millis(1)))),
        Err(ManualTimeError::ClockDrift { expected, observed })
            if expected == Duration::ZERO && observed == Duration::from_millis(1))
    );
    assert_eq!(medium.now(), tick(0));
}

#[test]
fn spawned_async_tasks_are_refused_instead_of_claiming_quiescence() {
    let medium = medium();
    let mut driver = driver(&medium);
    assert!(matches!(
        driver.poll(pin!(async {
            drop(tokio::spawn(pending::<()>()));
        })),
        Err(ManualTimeError::SpawnedTasks { count: 1 })
    ));
    assert!(matches!(
        driver.advance_to_next_event(tick(1)),
        Err(ManualTimeError::SpawnedTasks { count: 1 })
    ));
    assert_eq!(medium.now(), tick(0));
}

#[tokio::test]
async fn nested_runtime_construction_is_a_typed_refusal() {
    assert!(matches!(
        ManualTimeDriver::new(ManualMedium::Frames(medium()), Duration::from_millis(1)),
        Err(ManualTimeError::InsideRuntime)
    ));
}

#[test]
fn invalid_tick_durations_are_rejected_as_whole_values() {
    for requested in [
        Duration::ZERO,
        Duration::from_nanos(1),
        Duration::from_micros(1_001),
        Duration::MAX,
    ] {
        let medium = medium();
        let before = (medium.schedule(), medium.trace());
        assert!(
            matches!(ManualTimeDriver::new(ManualMedium::Frames(medium.clone()), requested),
            Err(ManualTimeError::InvalidTickDuration { requested: actual }) if actual == requested)
        );
        assert_eq!((medium.schedule(), medium.trace()), before);
    }
}
