use super::*;
use crate::scenario::POLL_BUDGET;

pub(super) fn clocks(
    runner: &mut ManualTaskRunner<'_, Completion>,
    nodes: &[fixture::Node],
    origins: [SimulationTick; 3],
    frames: &VirtualMedium,
    ble: &VirtualBleLab,
) {
    let snapshot = runner
        .snapshot()
        .unwrap_or_else(|error| unreachable!("clock snapshot: {error}"));
    assert_eq!((frames.now(), ble.now()), (snapshot.tick, snapshot.tick));
    let expected: BTreeMap<_, _> = nodes
        .iter()
        .enumerate()
        .map(|(node, live)| {
            let clock = live.control.clock.clone();
            let origin = live.control.origin;
            let task = runner
                .insert(async move {
                    Completion::Clock {
                        node,
                        elapsed: clock.now().duration_since(origin),
                    }
                })
                .unwrap_or_else(|error| unreachable!("clock observer: {error}"));
            (
                task,
                Completion::Clock {
                    node,
                    elapsed: DurationMillis(snapshot.tick.get() - origins[node].get()),
                },
            )
        })
        .collect();
    assert_eq!(
        settle(runner).into_iter().collect::<BTreeMap<_, _>>(),
        expected
    );
    assert_eq!(runner.snapshot().ok(), Some(snapshot));
}

pub(super) fn lost_requests(
    runner: &mut ManualTaskRunner<'_, Completion>,
    nodes: &[fixture::Node],
    crossing: &[(usize, LinkId)],
) -> BTreeMap<ManualTaskId, Completion> {
    crossing
        .iter()
        .map(|&(node, link)| {
            let handle = nodes[node].control.handle.clone();
            let task = runner
                .insert(async move {
                    assert_eq!(
                        handle
                            .request_with_response_timeout(
                                link,
                                RequestPathHash::of(QUERY_PATH),
                                b"interrupted bridge path",
                                RequestResponseTimeout::Exact(DurationMillis(REQUEST_TIMEOUT))
                            )
                            .await,
                        Err(SendError::Failed(SendRequestFailure::Timeout))
                    );
                    Completion::TimedOut { node }
                })
                .unwrap_or_else(|error| unreachable!("request actor: {error}"));
            (task, Completion::TimedOut { node })
        })
        .collect()
}

pub(super) fn expire(
    runner: &mut ManualTaskRunner<'_, Completion>,
    frames: &VirtualMedium,
    ble: &VirtualBleLab,
    started: SimulationTick,
    expected: BTreeMap<ManualTaskId, Completion>,
) {
    expire_at(
        runner,
        frames,
        ble,
        tick(started.get() + REQUEST_TIMEOUT),
        expected,
    );
}

pub(super) fn expire_at(
    runner: &mut ManualTaskRunner<'_, Completion>,
    frames: &VirtualMedium,
    ble: &VirtualBleLab,
    deadline: SimulationTick,
    expected: BTreeMap<ManualTaskId, Completion>,
) {
    assert!(deadline >= frames.now());
    let remaining_actors = runner.task_count() - expected.len();
    for _ in 0..POLL_BUDGET {
        let now = runner
            .snapshot()
            .unwrap_or_else(|error| unreachable!("clock: {error}"))
            .tick
            .get();
        if now < deadline.get() {
            assert!(runner
                .advance_to_next_event(tick((now + 1).min(deadline.get())))
                .is_ok());
        }
        assert_eq!(frames.now(), ble.now());
        let completed = settle(runner);
        if frames.now() == deadline {
            assert_eq!(completed.into_iter().collect::<BTreeMap<_, _>>(), expected);
            assert_eq!(runner.task_count(), remaining_actors);
            return;
        }
        assert!(completed.is_empty(), "no early old-link settlement");
    }
    unreachable!("bounded mixed-medium expiry must reach the exact deadline");
}

pub(super) fn routes(
    runner: &mut ManualTaskRunner<'_, Completion>,
    nodes: &[fixture::Node],
    frame_id: InterfaceId,
) {
    let mut expected = BTreeMap::new();
    for (node, destination, interface) in [
        (0, BLE_PEER, frame_id),
        (
            BLE_PEER,
            0,
            InterfaceId::from_channel_tag(InterfaceKind::BluetoothPeer, &[BRIDGE as u8; 16]),
        ),
    ] {
        let handle = nodes[node].control.handle.clone();
        let task = runner
            .insert(async move {
                let route = handle
                    .route(hash(destination))
                    .await
                    .unwrap_or_else(|| unreachable!("bridged route"));
                Completion::Route {
                    node,
                    hops: route.hops,
                    interface: route.interface,
                }
            })
            .unwrap_or_else(|error| unreachable!("route observer: {error}"));
        expected.insert(
            task,
            Completion::Route {
                node,
                hops: 2,
                interface,
            },
        );
    }
    assert_eq!(
        settle(runner).into_iter().collect::<BTreeMap<_, _>>(),
        expected
    );
}
