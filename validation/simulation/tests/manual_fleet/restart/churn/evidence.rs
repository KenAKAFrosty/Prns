use super::*;

pub(super) fn capacity(medium: &VirtualMedium, nodes: &[LiveNode]) {
    let before = medium.inspect_trace(|trace| (trace.discarded_events, trace.events().len()));
    assert_eq!(
        medium.attach(b"over-capacity").err(),
        Some(prns_simulation::AttachError::EndpointCapacityReached)
    );
    assert_eq!(
        medium.set_reachability(
            nodes[0].endpoint,
            nodes[2].endpoint,
            Reachability::Reachable,
        ),
        Err(prns_simulation::TopologyError::NeighborCapacityReached {
            node: nodes[0].endpoint,
            maximum: 1,
        })
    );
    assert_eq!(
        medium.inspect_trace(|trace| (trace.discarded_events, trace.events().len())),
        before
    );
}

pub(super) fn clocks(runner: &mut ManualTaskRunner<'_, Completion>, nodes: &[LiveNode]) {
    let at = now(runner);
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
                    elapsed: DurationMillis(at - live.born_at),
                },
            )
        })
        .collect();
    assert_eq!(runner.task_count(), ACTOR_CAPACITY);
    assert_eq!(
        runner.insert(std::future::pending::<Completion>()),
        Err(prns_simulation::ManualTaskAdmissionError::Capacity {
            maximum: nonzero(ACTOR_CAPACITY),
        })
    );
    assert_eq!(
        settle(runner).into_iter().collect::<BTreeMap<_, _>>(),
        expected
    );
    assert_eq!(runner.task_count(), NODES);
}

pub(super) fn shutdown(runner: &mut ManualTaskRunner<'_, Completion>, nodes: Vec<LiveNode>) {
    let expected: BTreeMap<_, _> = nodes
        .into_iter()
        .enumerate()
        .map(|(node, live)| {
            assert_eq!(live.control.shutdown.send(()), Ok(()));
            (
                live.task,
                Completion::Stopped {
                    node,
                    result: Ok(()),
                },
            )
        })
        .collect();
    assert_eq!(
        settle(runner).into_iter().collect::<BTreeMap<_, _>>(),
        expected
    );
    assert_eq!(runner.task_count(), 0);
}

pub(super) fn trace(medium: &VirtualMedium, expected_attachments: usize) {
    assert_eq!(medium.pending_delivery_count(), 0);
    medium.inspect_trace(|trace| {
        assert_eq!(trace.discarded_events, 0);
        let mut pairs = BTreeMap::new();
        let mut detached = Vec::new();
        let mut transmissions = BTreeMap::new();
        let mut delivered = 0;
        for event in trace.events() {
            match event {
                MediumEvent::EndpointAttached {
                    endpoint,
                    channel_tag,
                } => {
                    let index = u64::from_be_bytes(
                        channel_tag
                            .as_slice()
                            .try_into()
                            .unwrap_or_else(|_| unreachable!("node tags are eight-byte indices")),
                    );
                    assert!(index < NODES as u64);
                    assert_eq!(pairs.insert(*endpoint, index / 2), None);
                }
                MediumEvent::EndpointDetached { endpoint } => detached.push(*endpoint),
                MediumEvent::TransmissionAccepted { ordinal, from, .. } => {
                    assert_eq!(transmissions.insert(*ordinal, pairs[from]), None);
                }
                MediumEvent::ReceptionQueued { ordinal, to, .. } => {
                    assert_eq!(
                        transmissions[ordinal], pairs[to],
                        "traffic cannot escape its pair"
                    );
                    delivered += 1;
                }
                MediumEvent::ReceptionDropped { .. } => {
                    unreachable!("no bounded queue may overflow")
                }
                MediumEvent::ReceptionScheduled { .. } => {
                    unreachable!("this scenario has no delivery delay")
                }
                MediumEvent::ReachabilityChanged { .. } | MediumEvent::TimeAdvanced { .. } => {}
            }
        }
        assert!(delivered > 0);
        detached.sort();
        assert_eq!(pairs.len(), expected_attachments);
        assert_eq!(detached, pairs.keys().copied().collect::<Vec<_>>());
    });
}
