use super::*;
use prns_simulation::{ManualTaskScheduling, SimulationSeed};

const NODES: usize = 3;
const HUB: usize = 0;
const LEAVES: [usize; 2] = [1, 2];

#[derive(Debug, PartialEq, Eq)]
struct StarTranscript {
    discovery: BleTraceSnapshot,
    wire: BleWireSnapshot,
    responses: Vec<Vec<(usize, Vec<u8>)>>,
    boundaries: Vec<usize>,
}

fn address(node: usize) -> BleAddress {
    BleAddress::new([node as u8; 6])
}

fn inventory(peers: &[usize]) -> Vec<(InterfaceId, ConnectionState)> {
    let mut members: Vec<_> = peers
        .iter()
        .map(|peer| {
            (
                InterfaceId::from_channel_tag(InterfaceKind::BluetoothPeer, &[*peer as u8; 16]),
                ConnectionState::Connected,
            )
        })
        .collect();
    members.sort_by_key(|(id, _)| *id);
    members
}

fn run(scheduling: ManualTaskScheduling, seed: u8, marker: u8) -> StarTranscript {
    let capture = BleWireCapture::new(nonzero(8192));
    let lab = VirtualBleLab::with_wire_capture(
        BleMediumConfig::new(
            TopologyConfig::Explicit {
                max_neighbors: nonzero(2),
            },
            NODES,
            8,
            NODES,
            262_144,
        )
        .unwrap_or_else(|error| unreachable!("star lab: {error}")),
        capture.clone(),
    );
    let mut driver =
        ManualTimeDriver::new(ManualMedium::Ble(lab.clone()), Duration::from_millis(1))
            .unwrap_or_else(|error| unreachable!("clock: {error}"));
    let mut runner = ManualTaskRunner::new_with_scheduling(
        &mut driver,
        nonzero(NODES + LEAVES.len()),
        scheduling,
    );
    let nodes: Vec<_> = (0..NODES)
        .map(|index| {
            let start = if index == HUB {
                fixture::start_node_with_peers::<2>
            } else {
                fixture::start_node_with_peers::<1>
            };
            start(
                &mut runner,
                &lab,
                index,
                seed,
                selection::FirstEvent::ALL[index],
                personal_rns::runtime::InterfaceEventSource::Message,
            )
        })
        .collect();
    let destinations: Vec<_> = (0..NODES)
        .map(|node| {
            destination(node)
                .destination_hash()
                .unwrap_or_else(|error| unreachable!("destination: {error:?}"))
        })
        .collect();
    for leaf in LEAVES {
        assert_eq!(
            lab.set_reachability(address(HUB), address(leaf), Reachability::Reachable),
            Ok(TopologyMutation::Applied)
        );
    }
    ble::advance_until(&mut runner, || {
        lab.active_connection_count() == LEAVES.len()
            && ble::member_inventory(&nodes[HUB].control.handle) == inventory(&LEAVES)
            && LEAVES.iter().all(|leaf| {
                ble::member_inventory(&nodes[*leaf].control.handle) == inventory(&[HUB])
            })
    });
    announce(&nodes[HUB].control, destinations[HUB]);
    ble::advance_until(&mut runner, || {
        LEAVES
            .iter()
            .all(|leaf| *nodes[*leaf].heard.borrow() == [destinations[HUB]])
    });
    let mut links = traffic::establish(
        &mut runner,
        &nodes,
        LEAVES.map(|leaf| (leaf, destinations[HUB])),
    );
    let mut responses = vec![traffic::exchange(&mut runner, &nodes, &links, marker)];
    let before = capture.snapshot();
    let mut boundaries = vec![before.values.len()];
    let original = connections(&before.values);
    assert_eq!(original.len(), LEAVES.len());
    assert_ne!(original[&1], original[&2]);

    assert_eq!(
        lab.set_reachability(address(HUB), address(1), Reachability::Isolated),
        Ok(TopologyMutation::Applied)
    );
    assert!(settle(&mut runner).is_empty());
    assert_eq!(lab.active_connection_count(), 1);
    assert_eq!(
        ble::member_inventory(&nodes[HUB].control.handle),
        inventory(&[2])
    );
    assert!(ble::member_inventory(&nodes[1].control.handle).is_empty());
    assert_eq!(
        ble::member_inventory(&nodes[2].control.handle),
        inventory(&[HUB])
    );
    assert!(links.remove(&1).is_some());
    responses.push(traffic::exchange(&mut runner, &nodes, &links, marker));
    let isolated = capture.snapshot();
    assert_eq!(
        connections(&isolated.values[boundaries[0]..]),
        BTreeMap::from([(2, original[&2])])
    );
    boundaries.push(isolated.values.len());

    assert_eq!(
        lab.set_reachability(address(HUB), address(1), Reachability::Reachable),
        Ok(TopologyMutation::Applied)
    );
    ble::advance_until(&mut runner, || {
        lab.active_connection_count() == LEAVES.len()
            && ble::member_inventory(&nodes[HUB].control.handle) == inventory(&LEAVES)
            && LEAVES.iter().all(|leaf| {
                ble::member_inventory(&nodes[*leaf].control.handle) == inventory(&[HUB])
            })
    });
    links.extend(traffic::establish(
        &mut runner,
        &nodes,
        [(1, destinations[HUB])],
    ));
    responses.push(traffic::exchange(&mut runner, &nodes, &links, marker));
    let recovered = capture.snapshot();
    let replacement = connections(&recovered.values[boundaries[1]..]);
    assert_eq!(replacement.len(), 2);
    assert_ne!(replacement[&1], original[&1]);
    assert_eq!(replacement[&2], original[&2]);
    boundaries.push(recovered.values.len());

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
        settle(&mut runner).into_iter().collect::<BTreeMap<_, _>>(),
        expected
    );
    assert_eq!(runner.task_count(), 0);
    assert_eq!(lab.active_connection_count(), 0);
    let discovery = lab.trace();
    assert_eq!(discovery.discarded_events, 0);
    let mut attached = Vec::new();
    let mut detached = Vec::new();
    for event in &discovery.events {
        match event {
            BleSimulationEvent::RadioAttached { radio } => attached.push(*radio),
            BleSimulationEvent::RadioDetached { radio } => detached.push(*radio),
            BleSimulationEvent::ObservationDropped { .. } => unreachable!("observation loss"),
            _ => {}
        }
    }
    attached.sort();
    detached.sort();
    assert_eq!(attached.len(), NODES);
    assert_eq!(attached, detached);
    let wire = capture.snapshot();
    assert_eq!(wire.discarded_values, 0);
    StarTranscript {
        discovery,
        wire,
        responses,
        boundaries,
    }
}

fn connections(
    values: &[prns_simulation::ble::BleWireValue],
) -> BTreeMap<usize, prns_simulation::ble::BleWireConnectionId> {
    let mut connections = BTreeMap::new();
    for value in values {
        let leaf = LEAVES
            .into_iter()
            .find(|leaf| {
                (value.from == address(HUB) && value.to == address(*leaf))
                    || (value.to == address(HUB) && value.from == address(*leaf))
            })
            .unwrap_or_else(|| unreachable!("only star edges may carry values"));
        if let Some(previous) = connections.insert(leaf, value.connection) {
            assert_eq!(previous, value.connection);
        }
    }
    connections
}

fn assert_replay(actual: &StarTranscript, expected: &StarTranscript) {
    for (index, (actual, expected)) in actual
        .wire
        .values
        .iter()
        .zip(&expected.wire.values)
        .enumerate()
    {
        assert_eq!(actual, expected, "wire value {index}");
    }
    for (index, (actual, expected)) in actual
        .discovery
        .events
        .iter()
        .zip(&expected.discovery.events)
        .enumerate()
    {
        assert_eq!(actual, expected, "discovery event {index}");
    }
    assert_eq!(actual, expected);
}

#[test]
fn shared_supervisor_replays_concurrent_leaves_and_selective_reconnect() {
    for scheduling in [
        ManualTaskScheduling::Cyclic,
        ManualTaskScheduling::Seeded {
            seed: SimulationSeed::new(7),
        },
    ] {
        let expected = run(scheduling, 11, 42);
        for _ in 0..8 {
            assert_replay(&run(scheduling, 11, 42), &expected);
        }
    }
}

#[test]
fn shared_supervisor_replay_detects_changed_inputs() {
    let baseline = run(ManualTaskScheduling::Cyclic, 11, 42);
    let seed = run(ManualTaskScheduling::Cyclic, 21, 42);
    assert_eq!(baseline.responses, seed.responses);
    assert_ne!(baseline.wire, seed.wire);
    let payload = run(ManualTaskScheduling::Cyclic, 11, 43);
    assert_ne!(baseline.responses, payload.responses);
    assert_ne!(baseline.wire, payload.wire);
}
