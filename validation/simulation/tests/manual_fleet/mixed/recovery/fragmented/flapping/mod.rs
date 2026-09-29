use super::*;
use prns_simulation::ManualTaskCancellation;

mod deadlines;
mod mixed;
mod retirement;
use retirement::Retirement;

#[derive(Clone, Copy)]
enum ExchangeOrder {
    Uniform,
    Alternating,
}

impl ExchangeOrder {
    fn at(self, first: Exchange, wave: usize) -> Exchange {
        match (self, wave % 2, first) {
            (Self::Uniform, _, exchange) | (Self::Alternating, 0, exchange) => exchange,
            (Self::Alternating, _, Exchange::Request) => Exchange::Response,
            (Self::Alternating, _, Exchange::Response) => Exchange::Request,
        }
    }
}

const WAVES: usize = 3;
const BATCHES: usize = 2;
const WAVE_GAP_MS: u64 = 7;

struct PendingRequest {
    started: SimulationTick,
    deadline: SimulationTick,
    task: ManualTaskId,
}

struct Scenario {
    retirement: Retirement,
    order: ExchangeOrder,
    deadlines: deadlines::DeadlineOrder,
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
    scenario: &Scenario,
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
            for batch in 0..BATCHES {
                let origin = frames.now();
                let mut pending: Vec<PendingRequest> = Vec::with_capacity(WAVES);
                for wave in 0..WAVES {
                    let exchange = scenario.order.at(exchange, wave);
                    let observed_direction = exchange.direction(direction);
                    if wave != 0 {
                        let target = tick(frames.now().get() + WAVE_GAP_MS);
                        assert!(pending.iter().all(|request| target < request.deadline));
                        for _ in 0..POLL_BUDGET {
                            if frames.now() == target {
                                break;
                            }
                            assert!(runner.advance_to_next_event(target).is_ok());
                            assert!(settle(runner).is_empty());
                            assert_eq!(frames.now(), ble.now());
                        }
                        assert_eq!(frames.now(), target);
                    }
                    let started = frames.now();
                    if let Some(previous) = pending.last() {
                        assert!(
                            started > previous.started,
                            "outage admissions must be staggered"
                        );
                    }
                    let deadline = scenario.deadlines.at(origin, started, wave);
                    assert!(deadline > started);
                    let timeout = DurationMillis(deadline.get() - started.get());
                    let before = observed_direction.counters(ble);
                    let handle = nodes[direction.requester()].control.handle.clone();
                    let task = runner
                        .insert(async move {
                            let bytes = vec![0x20 + (batch * WAVES + wave) as u8; 256];
                            assert_eq!(
                                handle
                                    .request_with_response_timeout(
                                        crossing,
                                        RequestPathHash::of(QUERY_PATH),
                                        &bytes,
                                        RequestResponseTimeout::Exact(timeout)
                                    )
                                    .await,
                                Err(SendError::Failed(SendRequestFailure::Timeout))
                            );
                            Completion::TimedOut {
                                node: direction.requester(),
                            }
                        })
                        .unwrap_or_else(|error| unreachable!("flapping request: {error}"));
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
                    reachability(ble, Reachability::Isolated);
                    assert!(settle(runner).is_empty());
                    assert_eq!(runner.snapshot().ok().as_ref(), Some(&clock));
                    assert_eq!(ble.active_connection_count(), 0);
                    assert!(ble.data_snapshots().is_empty());
                    for node in [BRIDGE, BLE_PEER] {
                        assert!(member_inventory(&nodes[node].control.handle).is_empty());
                    }
                    pending.push(PendingRequest {
                        started,
                        deadline,
                        task,
                    });
                    assert_eq!(runner.task_count(), 3 + pending.len());
                    echo(runner, nodes, 0, local, 0x40 + (batch * WAVES + wave) as u8);
                    reachability(ble, Reachability::Reachable);
                    converge(runner, ble, nodes);
                    assert!(
                        pending
                            .iter()
                            .all(|request| frames.now() < request.deadline),
                        "all reconnections must precede the first pending deadline"
                    );
                    routes(runner, nodes, frame_id);
                    cancellation::replacements(
                        runner,
                        nodes,
                        direction,
                        crossing,
                        batch * 8 + wave,
                    );
                    assert_eq!(runner.task_count(), 3 + pending.len());
                    assert_eq!(ble.active_connection_count(), 1);
                }
                assert_eq!(runner.task_count(), 3 + WAVES);
                let mut live = WAVES - scenario.retirement.apply(runner, frames, ble, &pending);
                let mut batches = BTreeMap::<_, Vec<_>>::new();
                for (index, request) in pending.into_iter().enumerate() {
                    batches
                        .entry(request.deadline)
                        .or_default()
                        .push((index, request.task));
                }
                for (index, (deadline, requests)) in batches.into_iter().enumerate() {
                    let mut expected = BTreeMap::new();
                    for &(admission, task) in &requests {
                        if !scenario.retirement.cancels(admission) {
                            live -= 1;
                            expected.insert(
                                task,
                                Completion::TimedOut {
                                    node: direction.requester(),
                                },
                            );
                        }
                    }
                    traffic::expire_at(runner, frames, ble, deadline, expected);
                    assert_eq!(runner.task_count(), 3 + live);
                    for (_, task) in requests {
                        assert_eq!(
                            runner.cancel(task).ok(),
                            Some(ManualTaskCancellation::NotLive)
                        );
                    }
                    cancellation::replacements(
                        runner,
                        nodes,
                        direction,
                        crossing,
                        batch * 8 + WAVES + index,
                    );
                    echo(
                        runner,
                        nodes,
                        0,
                        local,
                        0x50 + (batch * WAVES + index) as u8,
                    );
                }
                assert_eq!(runner.task_count(), 3);
                assert_eq!(
                    nodes.iter().map(|node| node.task).collect::<Vec<_>>(),
                    tasks
                );
                traffic::clocks(runner, nodes, [tick(0); 3], frames, ble);
            }
        },
    );
}

fn matrix(exchange: Exchange, boundary: CutBoundary, retirement: Retirement) {
    matrix_with_order(exchange, boundary, retirement, ExchangeOrder::Uniform);
}

fn matrix_with_order(
    exchange: Exchange,
    boundary: CutBoundary,
    retirement: Retirement,
    order: ExchangeOrder,
) {
    matrix_scenario(
        exchange,
        boundary,
        Scenario {
            retirement,
            order,
            deadlines: deadlines::DeadlineOrder::Admission,
        },
    );
}

fn matrix_scenario(exchange: Exchange, boundary: CutBoundary, scenario: Scenario) {
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
                exercise(
                    direction, exchange, boundary, &scenario, profile, scheduling,
                );
            }
        }
    }
}

#[test]
fn staggered_timeouts_survive_repeated_queued_request_loss() {
    matrix(Exchange::Request, CutBoundary::Queued, Retirement::KeepAll);
}

#[test]
fn staggered_timeouts_survive_repeated_consumed_request_loss() {
    matrix(
        Exchange::Request,
        CutBoundary::Consumed,
        Retirement::KeepAll,
    );
}

#[test]
fn staggered_timeouts_survive_repeated_queued_response_loss() {
    matrix(Exchange::Response, CutBoundary::Queued, Retirement::KeepAll);
}

#[test]
fn staggered_timeouts_survive_repeated_consumed_response_loss() {
    matrix(
        Exchange::Response,
        CutBoundary::Consumed,
        Retirement::KeepAll,
    );
}

#[test]
fn cancelling_middle_queued_request_preserves_neighbor_deadlines() {
    matrix(
        Exchange::Request,
        CutBoundary::Queued,
        Retirement::CancelMiddle,
    );
}

#[test]
fn cancelling_middle_consumed_request_preserves_neighbor_deadlines() {
    matrix(
        Exchange::Request,
        CutBoundary::Consumed,
        Retirement::CancelMiddle,
    );
}

#[test]
fn cancelling_middle_queued_response_preserves_neighbor_deadlines() {
    matrix(
        Exchange::Response,
        CutBoundary::Queued,
        Retirement::CancelMiddle,
    );
}

#[test]
fn cancelling_middle_consumed_response_preserves_neighbor_deadlines() {
    matrix(
        Exchange::Response,
        CutBoundary::Consumed,
        Retirement::CancelMiddle,
    );
}
