use super::*;
use prns_simulation::{EndpointId, ManualTaskScheduling, SimulationSeed};

#[derive(Clone, Copy, Debug)]
enum RecoveryOrder {
    FramesFirst,
    BleFirst,
}

struct Paths {
    crossing: [(usize, LinkId); 2],
    frame_local: (usize, LinkId),
    ble_local: (usize, LinkId),
}

impl Paths {
    fn all(&self) -> [(usize, LinkId); 4] {
        [
            self.crossing[0],
            self.crossing[1],
            self.frame_local,
            self.ble_local,
        ]
    }

    fn unavailable(&self, order: RecoveryOrder) -> [(usize, LinkId); 3] {
        let local = match order {
            RecoveryOrder::FramesFirst => self.ble_local,
            RecoveryOrder::BleFirst => self.frame_local,
        };
        [self.crossing[0], self.crossing[1], local]
    }

    fn restored_local(&self, order: RecoveryOrder) -> (usize, LinkId) {
        match order {
            RecoveryOrder::FramesFirst => self.frame_local,
            RecoveryOrder::BleFirst => self.ble_local,
        }
    }
}

fn frames_reachability(frames: &VirtualMedium, endpoints: [EndpointId; 2], state: Reachability) {
    assert_eq!(
        frames.set_reachability(endpoints[0], endpoints[1], state),
        Ok(TopologyMutation::Applied)
    );
}

fn ble_reachability(ble: &VirtualBleLab, state: Reachability) {
    assert_eq!(
        ble.set_reachability(
            BleAddress::new([BRIDGE as u8; 6]),
            BleAddress::new([BLE_PEER as u8; 6]),
            state
        ),
        Ok(TopologyMutation::Applied)
    );
}

fn restore_first(
    order: RecoveryOrder,
    frames: &VirtualMedium,
    endpoints: [EndpointId; 2],
    ble: &VirtualBleLab,
) {
    match order {
        RecoveryOrder::FramesFirst => {
            frames_reachability(frames, endpoints, Reachability::Reachable)
        }
        RecoveryOrder::BleFirst => ble_reachability(ble, Reachability::Reachable),
    }
}

fn restore_second(
    order: RecoveryOrder,
    frames: &VirtualMedium,
    endpoints: [EndpointId; 2],
    ble: &VirtualBleLab,
) {
    match order {
        RecoveryOrder::FramesFirst => ble_reachability(ble, Reachability::Reachable),
        RecoveryOrder::BleFirst => frames_reachability(frames, endpoints, Reachability::Reachable),
    }
}

fn ble_connected(
    runner: &mut ManualTaskRunner<'_, Completion>,
    ble: &VirtualBleLab,
    nodes: &[fixture::Node],
) {
    // End-to-end announcements cannot cross the still-partitioned frame medium.
    advance_until(runner, || {
        ble.active_connection_count() == 1
            && [BRIDGE, BLE_PEER].into_iter().all(|node| {
                member_inventory(&nodes[node].control.handle)
                    == vec![(
                        InterfaceId::from_channel_tag(
                            InterfaceKind::BluetoothPeer,
                            &[(3 - node) as u8; 16],
                        ),
                        ConnectionState::Connected,
                    )]
            })
    });
}

fn concurrent_echo(
    runner: &mut ManualTaskRunner<'_, Completion>,
    nodes: &[fixture::Node],
    paths: &[(usize, LinkId)],
    marker: u8,
) {
    let before = runner
        .snapshot()
        .unwrap_or_else(|error| unreachable!("clock: {error}"));
    let expected: BTreeMap<_, _> = paths
        .iter()
        .enumerate()
        .map(|(lane, &(node, link))| {
            let mut bytes = vec![marker; 256];
            bytes[0] = lane as u8;
            bytes[1] = node as u8;
            let task = request(
                runner,
                node,
                nodes[node].control.handle.clone(),
                link,
                bytes.clone(),
            );
            (task, Completion::Response { node, bytes })
        })
        .collect();
    assert_eq!(
        settle(runner).into_iter().collect::<BTreeMap<_, _>>(),
        expected
    );
    assert_eq!(runner.snapshot().ok(), Some(before));
}

fn exercise(order: RecoveryOrder, profile: Profile, scheduling: ManualTaskScheduling) {
    with_bridge(
        profile,
        scheduling,
        2,
        |runner, frames, ble, _, nodes, frame_id| {
            converge(runner, ble, nodes);
            routes(runner, nodes, frame_id);
            let endpoints = frames.inspect_trace(|trace| {
                let endpoints: Vec<_> = trace
                    .events()
                    .filter_map(|event| match event {
                        MediumEvent::EndpointAttached { endpoint, .. } => Some(*endpoint),
                        _ => None,
                    })
                    .collect();
                <[EndpointId; 2]>::try_from(endpoints)
                    .unwrap_or_else(|_| unreachable!("two frame attachments"))
            });
            let tasks: Vec<_> = nodes.iter().map(|node| node.task).collect();
            let paths = Paths {
                crossing: [
                    (0, link(runner, nodes, 0, BLE_PEER)),
                    (BLE_PEER, link(runner, nodes, BLE_PEER, 0)),
                ],
                frame_local: (0, link(runner, nodes, 0, BRIDGE)),
                ble_local: (BLE_PEER, link(runner, nodes, BLE_PEER, BRIDGE)),
            };
            for cycle in 0..CYCLES {
                concurrent_echo(runner, nodes, &paths.all(), 0x20 + cycle as u8);
                let before = runner
                    .snapshot()
                    .unwrap_or_else(|error| unreachable!("clock: {error}"));
                frames_reachability(frames, endpoints, Reachability::Isolated);
                ble_reachability(ble, Reachability::Isolated);
                assert!(settle(runner).is_empty());
                assert_eq!(runner.snapshot().ok(), Some(before));
                assert_eq!(ble.active_connection_count(), 0);
                for node in [BRIDGE, BLE_PEER] {
                    assert!(member_inventory(&nodes[node].control.handle).is_empty());
                }
                let started = frames.now();
                let lost = lost_requests(runner, nodes, &paths.all());
                assert_eq!(runner.task_count(), 7);
                assert!(settle(runner).is_empty());
                expire(runner, frames, ble, started, lost);

                restore_first(order, frames, endpoints, ble);
                match order {
                    RecoveryOrder::FramesFirst => {
                        assert!(settle(runner).is_empty());
                        assert_eq!(ble.active_connection_count(), 0);
                    }
                    RecoveryOrder::BleFirst => ble_connected(runner, ble, nodes),
                }
                let started = frames.now();
                let lost = lost_requests(runner, nodes, &paths.unavailable(order));
                concurrent_echo(
                    runner,
                    nodes,
                    &[paths.restored_local(order)],
                    0x40 + cycle as u8,
                );
                expire(runner, frames, ble, started, lost);
                concurrent_echo(
                    runner,
                    nodes,
                    &[paths.restored_local(order)],
                    0x50 + cycle as u8,
                );

                restore_second(order, frames, endpoints, ble);
                converge(runner, ble, nodes);
                routes(runner, nodes, frame_id);
                concurrent_echo(runner, nodes, &paths.all(), 0x60 + cycle as u8);
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

#[test]
fn overlapping_partitions_require_both_media_to_recover_frames_first() {
    matrix(RecoveryOrder::FramesFirst);
}

#[test]
fn overlapping_partitions_require_both_media_to_recover_ble_first() {
    matrix(RecoveryOrder::BleFirst);
}

fn matrix(order: RecoveryOrder) {
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
            exercise(order, profile, scheduling);
        }
    }
}
