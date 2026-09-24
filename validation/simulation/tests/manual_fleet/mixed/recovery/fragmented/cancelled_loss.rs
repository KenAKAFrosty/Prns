use super::*;
use prns_simulation::ManualTaskCancellation;

#[derive(Clone, Copy)]
enum RecoveryTiming {
    BeforeTimeout,
    AfterTimeout,
}

fn reachability(ble: &VirtualBleLab, state: Reachability) {
    assert_eq!(
        ble.set_reachability(
            BleAddress::new([BRIDGE as u8; 6]),
            BleAddress::new([BLE_PEER as u8; 6]),
            state
        ),
        Ok(TopologyMutation::Applied)
    );
}

fn exercise(
    direction: Direction,
    exchange: Exchange,
    boundary: CutBoundary,
    recovery: RecoveryTiming,
    profile: Profile,
    scheduling: ManualTaskScheduling,
) {
    with_bridge(
        profile,
        scheduling,
        2,
        |runner, frames, ble, _, nodes, frame_id| {
            converge(runner, ble, nodes);
            routes(runner, nodes, frame_id);
            let crossing = link(runner, nodes, direction.requester(), direction.responder());
            let local = link(runner, nodes, 0, BRIDGE);
            let tasks: Vec<_> = nodes.iter().map(|node| node.task).collect();
            let observed_direction = exchange.direction(direction);
            for cycle in 0..CYCLES {
                let started = frames.now();
                let before = observed_direction.counters(ble);
                let handle = nodes[direction.requester()].control.handle.clone();
                let abandoned = runner
                    .insert(async move {
                        let bytes = vec![0x21 + cycle as u8; 256];
                        let result = handle
                            .request_with_response_timeout(
                                crossing,
                                RequestPathHash::of(QUERY_PATH),
                                &bytes,
                                RequestResponseTimeout::Exact(DurationMillis(REQUEST_TIMEOUT)),
                            )
                            .await;
                        unreachable!("cancelled caller must never finish: {result:?}");
                    })
                    .unwrap_or_else(|error| unreachable!("abandoned actor: {error}"));
                stop_partial(
                    runner,
                    ble,
                    nodes,
                    observed_direction,
                    boundary,
                    before,
                    FragmentTarget {
                        context: exchange.context(),
                        link: crossing,
                    },
                );
                let clock = runner
                    .snapshot()
                    .unwrap_or_else(|error| unreachable!("clock: {error}"));
                let activity = ble.data_snapshots();
                assert_eq!(
                    runner.cancel(abandoned).ok(),
                    Some(ManualTaskCancellation::Cancelled)
                );
                assert_eq!(ble.data_snapshots(), activity);
                assert_eq!(runner.snapshot().ok().as_ref(), Some(&clock));
                reachability(ble, Reachability::Isolated);
                assert!(settle(runner).is_empty());
                assert_eq!(runner.snapshot().ok().as_ref(), Some(&clock));
                assert_eq!(runner.task_count(), 3);
                assert_eq!(ble.active_connection_count(), 0);
                assert!(ble.data_snapshots().is_empty());
                for node in [BRIDGE, BLE_PEER] {
                    assert!(member_inventory(&nodes[node].control.handle).is_empty());
                }
                echo(runner, nodes, 0, local, 0x41 + cycle as u8);
                // A live caller provides a positive timeout control alongside the
                // retired caller; absence of all timer progress cannot pass this test.
                let live = lost_requests(runner, nodes, &[(direction.requester(), crossing)]);
                assert!(settle(runner).is_empty());
                assert_eq!(runner.task_count(), 4);
                let pending = match recovery {
                    RecoveryTiming::BeforeTimeout => live,
                    RecoveryTiming::AfterTimeout => {
                        expire(runner, frames, ble, started, live);
                        BTreeMap::new()
                    }
                };
                assert_eq!(
                    runner.cancel(abandoned).ok(),
                    Some(ManualTaskCancellation::NotLive)
                );
                assert_eq!(runner.task_count(), 3 + pending.len());
                echo(runner, nodes, 0, local, 0x51 + cycle as u8);

                reachability(ble, Reachability::Reachable);
                converge(runner, ble, nodes);
                if let RecoveryTiming::BeforeTimeout = recovery {
                    assert!(
                        frames.now().get() < started.get() + REQUEST_TIMEOUT,
                        "reconnection must precede the original deadline"
                    );
                    assert_eq!(runner.task_count(), 4);
                }
                routes(runner, nodes, frame_id);
                cancellation::replacements(runner, nodes, direction, crossing, cycle);
                assert_eq!(ble.active_connection_count(), 1);
                if let RecoveryTiming::BeforeTimeout = recovery {
                    assert!(frames.now().get() < started.get() + REQUEST_TIMEOUT);
                    assert_eq!(runner.task_count(), 4);
                    expire(runner, frames, ble, started, pending);
                    assert_eq!(runner.task_count(), 3);
                }
                // Exercise a second deadline interval after replacement, still with
                // no callback/completion from the abandoned actor.
                let recovered = frames.now();
                expire(runner, frames, ble, recovered, BTreeMap::new());
                cancellation::replacements(runner, nodes, direction, crossing, cycle + CYCLES);
                assert_eq!(
                    runner.cancel(abandoned).ok(),
                    Some(ManualTaskCancellation::NotLive)
                );
                assert_eq!(
                    nodes.iter().map(|node| node.task).collect::<Vec<_>>(),
                    tasks
                );
                traffic::clocks(runner, nodes, [tick(0); 3], frames, ble);
            }
        },
    );
}

fn matrix(exchange: Exchange, boundary: CutBoundary, recovery: RecoveryTiming) {
    for direction in [Direction::TowardBle, Direction::FromBle] {
        for profile in [Profile::AppleBridge, Profile::BluezBridge] {
            for scheduling in [
                ManualTaskScheduling::Cyclic,
                ManualTaskScheduling::Seeded {
                    seed: SimulationSeed::new(0),
                },
                ManualTaskScheduling::Seeded {
                    seed: SimulationSeed::new(7),
                },
                ManualTaskScheduling::Seeded {
                    seed: SimulationSeed::new(u64::MAX),
                },
            ] {
                exercise(direction, exchange, boundary, recovery, profile, scheduling);
            }
        }
    }
}

#[test]
fn cancelled_caller_survives_loss_of_queued_request_fragments() {
    matrix(
        Exchange::Request,
        CutBoundary::Queued,
        RecoveryTiming::AfterTimeout,
    );
}

#[test]
fn cancelled_caller_survives_loss_of_consumed_request_fragments() {
    matrix(
        Exchange::Request,
        CutBoundary::Consumed,
        RecoveryTiming::AfterTimeout,
    );
}

#[test]
fn cancelled_caller_survives_loss_of_queued_response_fragments() {
    matrix(
        Exchange::Response,
        CutBoundary::Queued,
        RecoveryTiming::AfterTimeout,
    );
}

#[test]
fn cancelled_caller_survives_loss_of_consumed_response_fragments() {
    matrix(
        Exchange::Response,
        CutBoundary::Consumed,
        RecoveryTiming::AfterTimeout,
    );
}

#[test]
fn early_recovery_after_abandoned_queued_request_preserves_old_timeout() {
    matrix(
        Exchange::Request,
        CutBoundary::Queued,
        RecoveryTiming::BeforeTimeout,
    );
}

#[test]
fn early_recovery_after_abandoned_consumed_request_preserves_old_timeout() {
    matrix(
        Exchange::Request,
        CutBoundary::Consumed,
        RecoveryTiming::BeforeTimeout,
    );
}

#[test]
fn early_recovery_after_abandoned_queued_response_preserves_old_timeout() {
    matrix(
        Exchange::Response,
        CutBoundary::Queued,
        RecoveryTiming::BeforeTimeout,
    );
}

#[test]
fn early_recovery_after_abandoned_consumed_response_preserves_old_timeout() {
    matrix(
        Exchange::Response,
        CutBoundary::Consumed,
        RecoveryTiming::BeforeTimeout,
    );
}
