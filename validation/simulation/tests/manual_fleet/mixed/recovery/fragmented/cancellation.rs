use super::*;
use prns_simulation::ManualTaskCancellation;

#[derive(Clone, Copy, Debug)]
enum ReplacementTiming {
    BeforeDrain,
    AfterDrain,
}

pub(super) fn replacements(
    runner: &mut ManualTaskRunner<'_, Completion>,
    nodes: &[fixture::Node],
    direction: Direction,
    crossing: LinkId,
    cycle: usize,
) {
    let remaining_actors = runner.task_count();
    let expected: BTreeMap<_, _> = [0x61, 0x91]
        .into_iter()
        .map(|marker| {
            let bytes = vec![marker + cycle as u8; 256];
            let task = request(
                runner,
                direction.requester(),
                nodes[direction.requester()].control.handle.clone(),
                crossing,
                bytes.clone(),
            );
            (
                task,
                Completion::Response {
                    node: direction.requester(),
                    bytes,
                },
            )
        })
        .collect();
    assert_eq!(
        settle(runner).into_iter().collect::<BTreeMap<_, _>>(),
        expected
    );
    assert_eq!(runner.task_count(), remaining_actors);
}

fn exercise(
    direction: Direction,
    exchange: Exchange,
    boundary: CutBoundary,
    timing: ReplacementTiming,
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
            let response_direction = Exchange::Response.direction(direction);
            for cycle in 0..CYCLES {
                let started = frames.now();
                let before = observed_direction.counters(ble);
                let response_before = response_direction.counters(ble);
                let handle = nodes[direction.requester()].control.handle.clone();
                let abandoned = runner
                    .insert(async move {
                        let bytes = vec![0x21 + cycle as u8; 256];
                        let (bytes, _) = handle
                            .request_with_response_timeout(
                                crossing,
                                RequestPathHash::of(QUERY_PATH),
                                &bytes,
                                RequestResponseTimeout::Exact(DurationMillis(REQUEST_TIMEOUT)),
                            )
                            .await
                            .unwrap_or_else(|error| {
                                unreachable!("abandoned waiter unexpectedly resumed: {error:?}")
                            });
                        Completion::Response {
                            node: direction.requester(),
                            bytes,
                        }
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
                    .unwrap_or_else(|error| unreachable!("cancel clock: {error}"));
                let activity = ble.data_snapshots();
                let partial = observed_direction.counters(ble);
                let trace = ble.trace();
                assert_eq!(runner.task_count(), 4);
                assert_eq!(
                    runner.cancel(abandoned).ok(),
                    Some(ManualTaskCancellation::Cancelled)
                );
                assert_eq!(
                    runner.cancel(abandoned).ok(),
                    Some(ManualTaskCancellation::NotLive)
                );
                assert_eq!(runner.task_count(), 3);
                assert_eq!(runner.snapshot().ok().as_ref(), Some(&clock));
                assert_eq!(ble.data_snapshots(), activity);
                assert_eq!(ble.trace(), trace);
                assert_eq!(ble.active_connection_count(), 1);

                match timing {
                    ReplacementTiming::BeforeDrain => {}
                    ReplacementTiming::AfterDrain => {
                        assert!(settle(runner).is_empty());
                        let after = observed_direction.counters(ble);
                        assert_eq!(
                            (
                                after.sends_started,
                                after.sends_completed,
                                after.frames_reassembled
                            ),
                            (
                                partial.sends_started,
                                partial.sends_completed + 1,
                                partial.frames_reassembled + 1
                            )
                        );
                        let (response, observation) = response_direction.activity(ble);
                        let observation = observation
                            .unwrap_or_else(|| unreachable!("peer response after cancellation"));
                        assert!(observation.before.sends_started >= response_before.sends_started);
                        assert!(observation.header.is_some_and(|header| {
                            header.context == WireContext::Response
                                && header.address == crossing.to_address()
                        }));
                        assert_eq!(
                            response.sends_completed,
                            observation.before.sends_completed + 1
                        );
                        assert_eq!(
                            response.frames_reassembled,
                            observation.before.frames_reassembled + 1
                        );
                        assert_eq!(runner.snapshot().ok().as_ref(), Some(&clock));
                    }
                }
                replacements(runner, nodes, direction, crossing, cycle);
                assert_eq!(runner.snapshot().ok().as_ref(), Some(&clock));
                assert_eq!(ble.active_connection_count(), 1);
                echo(runner, nodes, 0, local, 0x41 + cycle as u8);
                expire(runner, frames, ble, started, BTreeMap::new());
                assert_eq!(
                    runner.cancel(abandoned).ok(),
                    Some(ManualTaskCancellation::NotLive)
                );
                replacements(runner, nodes, direction, crossing, cycle + CYCLES);
                routes(runner, nodes, frame_id);
                assert_eq!(
                    nodes.iter().map(|node| node.task).collect::<Vec<_>>(),
                    tasks
                );
                assert_eq!(ble.active_connection_count(), 1);
                traffic::clocks(runner, nodes, [tick(0); 3], frames, ble);
            }
        },
    );
}

fn matrix(boundary: CutBoundary, timing: ReplacementTiming, exchange: Exchange) {
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
                exercise(direction, exchange, boundary, timing, profile, scheduling);
            }
        }
    }
}

#[test]
fn cancelled_queued_response_accepts_replacements_before_drain() {
    matrix(
        CutBoundary::Queued,
        ReplacementTiming::BeforeDrain,
        Exchange::Response,
    );
}

#[test]
fn cancelled_consumed_response_accepts_replacements_before_drain() {
    matrix(
        CutBoundary::Consumed,
        ReplacementTiming::BeforeDrain,
        Exchange::Response,
    );
}

#[test]
fn cancelled_queued_response_accepts_replacements_after_drain() {
    matrix(
        CutBoundary::Queued,
        ReplacementTiming::AfterDrain,
        Exchange::Response,
    );
}

#[test]
fn cancelled_consumed_response_accepts_replacements_after_drain() {
    matrix(
        CutBoundary::Consumed,
        ReplacementTiming::AfterDrain,
        Exchange::Response,
    );
}

#[test]
fn cancelled_queued_request_accepts_replacements_before_drain() {
    matrix(
        CutBoundary::Queued,
        ReplacementTiming::BeforeDrain,
        Exchange::Request,
    );
}

#[test]
fn cancelled_consumed_request_accepts_replacements_before_drain() {
    matrix(
        CutBoundary::Consumed,
        ReplacementTiming::BeforeDrain,
        Exchange::Request,
    );
}

#[test]
fn cancelled_queued_request_accepts_replacements_after_drain() {
    matrix(
        CutBoundary::Queued,
        ReplacementTiming::AfterDrain,
        Exchange::Request,
    );
}

#[test]
fn cancelled_consumed_request_accepts_replacements_after_drain() {
    matrix(
        CutBoundary::Consumed,
        ReplacementTiming::AfterDrain,
        Exchange::Request,
    );
}
