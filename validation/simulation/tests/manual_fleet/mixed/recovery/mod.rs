use super::*;
use prns_interfaces_tokio::bluetooth_auto::BluetoothAutoStatus;

mod fragmented;
mod inflight;
mod lab;
mod overlapping;
mod restart;
mod traffic;
use lab::{converge, with_bridge, Controls, Profile};
use traffic::{expire, lost_requests, routes};

const CYCLES: usize = 2;

#[derive(Clone, Copy)]
enum Interruption {
    Partition,
    BridgeRadio,
    PeerRadio,
}

impl Interruption {
    fn disconnect(self, ble: &VirtualBleLab, controls: &Controls) {
        match self {
            Self::Partition => assert_eq!(
                ble.set_reachability(
                    BleAddress::new([BRIDGE as u8; 6]),
                    BleAddress::new([BLE_PEER as u8; 6]),
                    Reachability::Isolated,
                ),
                Ok(TopologyMutation::Applied)
            ),
            Self::BridgeRadio => controls.bridge.disable(),
            Self::PeerRadio => controls.peer.disable(),
        }
    }

    fn reconnect(self, ble: &VirtualBleLab, controls: &Controls) {
        match self {
            Self::Partition => assert_eq!(
                ble.set_reachability(
                    BleAddress::new([BRIDGE as u8; 6]),
                    BleAddress::new([BLE_PEER as u8; 6]),
                    Reachability::Reachable,
                ),
                Ok(TopologyMutation::Applied)
            ),
            Self::BridgeRadio => controls.bridge.enable(),
            Self::PeerRadio => controls.peer.enable(),
        }
    }

    fn repeat_disconnected(self, ble: &VirtualBleLab, controls: &Controls) {
        match self {
            Self::Partition => assert_eq!(
                ble.set_reachability(
                    BleAddress::new([BRIDGE as u8; 6]),
                    BleAddress::new([BLE_PEER as u8; 6]),
                    Reachability::Isolated,
                ),
                Ok(TopologyMutation::Unchanged)
            ),
            Self::BridgeRadio => controls.bridge.disable(),
            Self::PeerRadio => controls.peer.disable(),
        }
    }
}

fn exercise(interruption: Interruption) {
    for profile in [Profile::AppleBridge, Profile::BluezBridge] {
        with_bridge(
            profile,
            prns_simulation::ManualTaskScheduling::Cyclic,
            2,
            |runner, frames, ble, controls, nodes, frame_id| {
                converge(runner, ble, nodes);
                routes(runner, nodes, frame_id);
                let local = link(runner, nodes, 0, BRIDGE);
                let mut crossing = vec![
                    (0, link(runner, nodes, 0, BLE_PEER)),
                    (BLE_PEER, link(runner, nodes, BLE_PEER, 0)),
                ];
                let task_ids: Vec<_> = nodes.iter().map(|node| node.task).collect();
                for cycle in 0..CYCLES {
                    for &(from, link) in &crossing {
                        echo(runner, nodes, from, link, 0x20 + cycle as u8);
                    }
                    let before = runner
                        .snapshot()
                        .unwrap_or_else(|error| unreachable!("clock: {error}"));
                    interruption.disconnect(ble, controls);
                    assert!(settle(runner).is_empty());
                    assert_eq!(runner.snapshot().ok(), Some(before));
                    assert_eq!(ble.active_connection_count(), 0);
                    for node in [BRIDGE, BLE_PEER] {
                        assert!(member_inventory(&nodes[node].control.handle).is_empty());
                    }
                    assert_eq!(runner.task_count(), 3);
                    let before = ble.trace();
                    interruption.repeat_disconnected(ble, controls);
                    assert!(settle(runner).is_empty());
                    assert_eq!(ble.trace(), before);

                    let started = frames.now();
                    let expected = lost_requests(runner, nodes, &crossing);
                    echo(runner, nodes, 0, local, 0x40 + cycle as u8);
                    expire(runner, frames, ble, started, expected);
                    assert_eq!(ble.active_connection_count(), 0);
                    echo(runner, nodes, 0, local, 0x50 + cycle as u8);

                    interruption.reconnect(ble, controls);
                    converge(runner, ble, nodes);
                    routes(runner, nodes, frame_id);
                    let fresh = [
                        (0, link(runner, nodes, 0, BLE_PEER)),
                        (BLE_PEER, link(runner, nodes, BLE_PEER, 0)),
                    ];
                    for (old, new) in crossing.iter().zip(&fresh) {
                        assert_ne!(old.1, new.1);
                    }
                    for &(from, link) in &crossing {
                        echo(runner, nodes, from, link, 0x59 + cycle as u8);
                    }
                    for &(from, link) in &fresh {
                        echo(runner, nodes, from, link, 0x60 + cycle as u8);
                    }
                    echo(runner, nodes, 0, local, 0x70 + cycle as u8);
                    assert_eq!(ble.active_connection_count(), 1);
                    assert_eq!(
                        nodes.iter().map(|node| node.task).collect::<Vec<_>>(),
                        task_ids
                    );
                    let before = ble.trace();
                    controls.bridge.enable();
                    controls.peer.enable();
                    assert!(settle(runner).is_empty());
                    assert_eq!(ble.trace(), before);
                    for &(from, link) in &fresh {
                        echo(runner, nodes, from, link, 0x80 + cycle as u8);
                    }
                    crossing.extend(fresh);
                    traffic::clocks(runner, nodes, [tick(0); 3], frames, ble);
                }
            },
        );
    }
}

#[test]
fn mixed_ble_partition_preserves_frame_local_links_and_recovers_crossing_traffic() {
    exercise(Interruption::Partition);
}

#[test]
fn mixed_bridge_radio_disable_preserves_frame_local_links_and_recovers_crossing_traffic() {
    exercise(Interruption::BridgeRadio);
}

#[test]
fn mixed_peer_radio_disable_preserves_frame_local_links_and_recovers_crossing_traffic() {
    exercise(Interruption::PeerRadio);
}
