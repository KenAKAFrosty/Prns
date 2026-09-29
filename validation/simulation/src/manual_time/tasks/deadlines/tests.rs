use super::*;
use crate::manual_time::tasks::tests::{capacity, driver, insert, poll};
use crate::{
    FaultPlan, ManualMedium, SimulationDurationInTicks, TopologyConfig, TransmissionOrdinal,
    TransmissionRule, VirtualMedium, VirtualMediumConfig,
};

#[test]
fn concurrent_timers_stop_at_the_earliest_deadline_and_keep_tied_actors_ready() {
    let mut driver = driver();
    let mut runner = ManualTaskRunner::new(&mut driver, capacity(3));
    let actors: Vec<_> = [10, 5, 5]
        .into_iter()
        .map(|delay| {
            insert(&mut runner, async move {
                tokio::time::sleep(Duration::from_millis(delay)).await;
                delay
            })
        })
        .collect();
    for actor in &actors {
        assert_eq!(poll(&mut runner), ManualTaskPoll::Pending { task: *actor });
    }
    runner
        .advance_to_next_wake(SimulationTick::from_ticks(100))
        .unwrap_or_else(|e| unreachable!("{e}"));
    assert_eq!(
        runner.snapshot().ok().map(|s| s.tick),
        Some(SimulationTick::from_ticks(5))
    );
    for actor in &actors[1..] {
        assert_eq!(
            poll(&mut runner),
            ManualTaskPoll::Completed {
                task: *actor,
                output: 5
            }
        );
    }
    assert_eq!(poll(&mut runner), ManualTaskPoll::Idle);
    runner
        .advance_to_next_wake(SimulationTick::from_ticks(100))
        .unwrap_or_else(|e| unreachable!("{e}"));
    assert_eq!(
        runner.snapshot().ok().map(|s| s.tick),
        Some(SimulationTick::from_ticks(10))
    );
    assert_eq!(
        poll(&mut runner),
        ManualTaskPoll::Completed {
            task: actors[0],
            output: 10
        }
    );
}

#[test]
fn nonzero_medium_origin_and_an_earlier_horizon_preserve_timer_deadlines() {
    let medium = VirtualMedium::new(
        VirtualMediumConfig::new(
            TopologyConfig::FullyConnected,
            1,
            1,
            1,
            16,
            FaultPlan::none(),
        )
        .unwrap_or_else(|e| unreachable!("{e}")),
    );
    medium
        .advance_to(SimulationTick::from_ticks(100))
        .unwrap_or_else(|e| unreachable!("{e}"));
    let mut driver = ManualTimeDriver::new(ManualMedium::Frames(medium), Duration::from_millis(1))
        .unwrap_or_else(|e| unreachable!("{e}"));
    let mut runner = ManualTaskRunner::new(&mut driver, capacity(1));
    let actor = insert(&mut runner, async {
        tokio::time::sleep(Duration::from_millis(10)).await;
        1
    });
    assert_eq!(poll(&mut runner), ManualTaskPoll::Pending { task: actor });
    runner
        .advance_to_next_wake(SimulationTick::from_ticks(105))
        .unwrap_or_else(|e| unreachable!("{e}"));
    assert_eq!(poll(&mut runner), ManualTaskPoll::Idle);
    runner
        .advance_to_next_wake(SimulationTick::from_ticks(200))
        .unwrap_or_else(|e| unreachable!("{e}"));
    assert_eq!(
        runner.snapshot().ok(),
        Some(crate::ManualTimeSnapshot {
            tick: SimulationTick::from_ticks(110),
            runtime_elapsed: Duration::from_millis(10),
        })
    );
    assert_eq!(
        poll(&mut runner),
        ManualTaskPoll::Completed {
            task: actor,
            output: 1
        }
    );
}

#[test]
fn wakes_at_runtime_deadlines_without_polling_actors_or_skipping_rearmed_timers() {
    let mut driver = driver();
    let mut runner = ManualTaskRunner::new(&mut driver, capacity(1));
    let actor = insert(&mut runner, async {
        for _ in 0..24 {
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
        24
    });
    assert_eq!(poll(&mut runner), ManualTaskPoll::Pending { task: actor });
    for hour in 1..=24 {
        runner
            .advance_to_next_wake(SimulationTick::from_ticks(86_400_000))
            .unwrap_or_else(|error| unreachable!("advance: {error}"));
        assert_eq!(
            runner
                .snapshot()
                .unwrap_or_else(|error| unreachable!("snapshot: {error}"))
                .tick,
            SimulationTick::from_ticks(hour * 3_600_000)
        );
        assert_eq!(runner.task_count(), 1);
        if hour == 24 {
            assert_eq!(
                poll(&mut runner),
                ManualTaskPoll::Completed {
                    task: actor,
                    output: 24
                }
            );
        } else {
            assert_eq!(poll(&mut runner), ManualTaskPoll::Pending { task: actor });
        }
    }
}

#[test]
fn canceled_timer_is_not_a_live_actor_and_horizon_terminates_an_idle_run() {
    let mut driver = driver();
    let mut runner = ManualTaskRunner::new(&mut driver, capacity(2));
    let canceled = insert(&mut runner, async {
        tokio::time::sleep(Duration::from_millis(2)).await
    });
    let live = insert(&mut runner, async {
        tokio::time::sleep(Duration::from_millis(10)).await
    });
    assert!(matches!(poll(&mut runner), ManualTaskPoll::Pending { .. }));
    assert!(matches!(poll(&mut runner), ManualTaskPoll::Pending { .. }));
    assert_eq!(
        runner.cancel(canceled).ok(),
        Some(super::super::ManualTaskCancellation::Cancelled)
    );
    runner
        .advance_to_next_wake(SimulationTick::from_ticks(100))
        .unwrap_or_else(|e| unreachable!("{e}"));
    assert_eq!(
        runner.snapshot().ok().map(|s| s.tick),
        Some(SimulationTick::from_ticks(10))
    );
    assert_eq!(
        poll(&mut runner),
        ManualTaskPoll::Completed {
            task: live,
            output: ()
        }
    );
    runner
        .advance_to_next_wake(SimulationTick::from_ticks(100))
        .unwrap_or_else(|e| unreachable!("{e}"));
    assert_eq!(
        runner.snapshot().ok().map(|s| s.tick),
        Some(SimulationTick::from_ticks(100))
    );
    assert!(matches!(
        runner.advance_to_next_wake(SimulationTick::ZERO),
        Err(ManualTimeError::BeforeCurrent { .. })
    ));
}

#[test]
fn medium_events_and_runtime_timers_share_the_earliest_boundary() {
    let medium = VirtualMedium::new(
        VirtualMediumConfig::new(
            TopologyConfig::FullyConnected,
            2,
            2,
            2,
            32,
            FaultPlan::new(vec![TransmissionRule::delay(
                TransmissionOrdinal::new(0),
                SimulationDurationInTicks::from_ticks(5),
            )])
            .unwrap_or_else(|e| unreachable!("{e}")),
        )
        .unwrap_or_else(|e| unreachable!("{e}")),
    );
    let sender = medium
        .attach(b"sender")
        .unwrap_or_else(|e| unreachable!("{e}"));
    let _receiver = medium
        .attach(b"receiver")
        .unwrap_or_else(|e| unreachable!("{e}"));
    assert_eq!(medium.transmit(sender.endpoint_id(), vec![1]), Ok(()));
    let mut driver = ManualTimeDriver::new(
        ManualMedium::Frames(medium.clone()),
        Duration::from_millis(1),
    )
    .unwrap_or_else(|e| unreachable!("{e}"));
    let mut runner = ManualTaskRunner::new(&mut driver, capacity(1));
    let observed = medium.clone();
    let actor = insert(&mut runner, async move {
        tokio::time::sleep(Duration::from_millis(2)).await;
        assert_eq!(observed.now(), SimulationTick::from_ticks(2));
        tokio::time::sleep(Duration::from_millis(3)).await;
        observed.now()
    });
    assert!(matches!(
        runner.advance_to_next_wake(SimulationTick::from_ticks(20)),
        Err(ManualTimeError::ReadyTasks { count: 1 })
    ));
    assert_eq!(poll(&mut runner), ManualTaskPoll::Pending { task: actor });
    runner
        .advance_to_next_wake(SimulationTick::from_ticks(20))
        .unwrap_or_else(|e| unreachable!("{e}"));
    assert_eq!(medium.now(), SimulationTick::from_ticks(2));
    assert_eq!(poll(&mut runner), ManualTaskPoll::Pending { task: actor });
    let report = runner
        .advance_to_next_wake(SimulationTick::from_ticks(20))
        .unwrap_or_else(|e| unreachable!("{e}"));
    assert!(matches!(
        report,
        ManualAdvance::Frames(crate::AdvanceReport {
            receptions_queued: 1,
            ..
        })
    ));
    assert_eq!(
        poll(&mut runner),
        ManualTaskPoll::Completed {
            task: actor,
            output: SimulationTick::from_ticks(5)
        }
    );
}
