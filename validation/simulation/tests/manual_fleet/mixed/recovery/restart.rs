use super::*;
use prns_simulation::ManualTaskCancellation;

#[test]
fn mixed_bridge_restart_loses_both_interface_paths_without_restarting_the_endpoints() {
    for profile in [Profile::AppleBridge, Profile::BluezBridge] {
        with_bridge(
            profile,
            prns_simulation::ManualTaskScheduling::Cyclic,
            2 + CYCLES,
            |runner, frames, ble, controls, nodes, frame_id| {
                converge(runner, ble, nodes);
                let client_endpoint = frames
                    .inspect_trace(|trace| {
                        trace.events().find_map(|event| match event {
                            MediumEvent::EndpointAttached {
                                endpoint,
                                channel_tag,
                            } if channel_tag == b"recovery-client" => Some(*endpoint),
                            _ => None,
                        })
                    })
                    .unwrap_or_else(|| unreachable!("client attachment"));
                let survivor_tasks = [nodes[0].task, nodes[BLE_PEER].task];
                let mut origins = [tick(0); 3];
                let mut current = vec![
                    (0, link(runner, nodes, 0, BRIDGE)),
                    (0, link(runner, nodes, 0, BLE_PEER)),
                    (BLE_PEER, link(runner, nodes, BLE_PEER, 0)),
                ];
                for cycle in 0..CYCLES {
                    for &(from, link) in &current {
                        echo(runner, nodes, from, link, 0x30 + cycle as u8);
                    }
                    let old_task = nodes[BRIDGE].task;
                    let old_status = controls.bridge.clone();
                    let started = frames.now();
                    let before = runner
                        .snapshot()
                        .unwrap_or_else(|error| unreachable!("clock: {error}"));
                    assert_eq!(
                        runner.cancel(old_task).ok(),
                        Some(ManualTaskCancellation::Cancelled)
                    );
                    assert!(settle(runner).is_empty());
                    assert_eq!(runner.snapshot().ok(), Some(before));
                    assert_eq!(runner.task_count(), 2);
                    assert_eq!(ble.active_connection_count(), 0);
                    assert!(member_inventory(&nodes[BLE_PEER].control.handle).is_empty());
                    let lost = lost_requests(runner, nodes, &current);
                    assert!(settle(runner).is_empty());
                    expire(runner, frames, ble, started, lost);
                    assert_eq!(runner.task_count(), 2);

                    let interface = frames.attach(b"recovery-bridge").unwrap_or_else(|error| {
                        unreachable!("replaced bridge frame interface: {error}")
                    });
                    let fresh_endpoint = interface.endpoint_id();
                    let fresh_ble = supervisor(
                        ble,
                        BRIDGE,
                        match profile {
                            Profile::AppleBridge => Endpoint::CoreBluetooth(AppleHost::MacOs),
                            Profile::BluezBridge => Endpoint::BlueZ(BlueZHost::Linux),
                        },
                    );
                    controls.bridge = fresh_ble.status();
                    origins[BRIDGE] = frames.now();
                    nodes[BRIDGE] = boot(
                        runner,
                        BRIDGE,
                        Ports::Bridge {
                            frames: interface,
                            ble: fresh_ble,
                        },
                    );
                    assert_ne!(nodes[BRIDGE].task, old_task);
                    assert_eq!(
                        runner.cancel(old_task).ok(),
                        Some(ManualTaskCancellation::NotLive)
                    );
                    assert_eq!(
                        frames.set_reachability(
                            client_endpoint,
                            fresh_endpoint,
                            Reachability::Reachable
                        ),
                        Ok(TopologyMutation::Applied)
                    );
                    assert_eq!(
                        ble.set_reachability(
                            BleAddress::new([BRIDGE as u8; 6]),
                            BleAddress::new([BLE_PEER as u8; 6]),
                            Reachability::Reachable
                        ),
                        Ok(TopologyMutation::Applied)
                    );
                    assert_eq!(runner.task_count(), 3);
                    converge(runner, ble, nodes);
                    routes(runner, nodes, frame_id);
                    let fresh = vec![
                        (0, link(runner, nodes, 0, BRIDGE)),
                        (0, link(runner, nodes, 0, BLE_PEER)),
                        (BLE_PEER, link(runner, nodes, BLE_PEER, 0)),
                    ];
                    for (old, new) in current.iter().zip(&fresh) {
                        assert_ne!(old.1, new.1);
                    }
                    old_status.disable();
                    assert!(settle(runner).is_empty());
                    assert_eq!(ble.active_connection_count(), 1);
                    let started = frames.now();
                    let obsolete = lost_requests(runner, nodes, &current);
                    for &(from, link) in &fresh {
                        echo(runner, nodes, from, link, 0x50 + cycle as u8);
                    }
                    expire(runner, frames, ble, started, obsolete);
                    assert_eq!([nodes[0].task, nodes[BLE_PEER].task], survivor_tasks);
                    for &(from, link) in &fresh {
                        echo(runner, nodes, from, link, 0x70 + cycle as u8);
                    }
                    current = fresh;
                    traffic::clocks(runner, nodes, origins, frames, ble);
                }
            },
        );
    }
}
