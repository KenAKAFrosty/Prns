use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use personal_rns::interfaces::bluetooth_auto::{
    AppleHost, BleAddress, BleIdentity, BleRoleCapabilities, BlueZHost, Endpoint, LinkCapabilities,
    BLE_HW_MTU, CONTROL_MAX_LEN,
};
use personal_rns::interfaces::{
    ConnectionState, InterfaceId, InterfaceKind, InterfaceStatus, Membership,
};
use personal_rns::routing::links::LinkId;
use personal_rns::runtime::PrnsNodeHandle;
use prns_interfaces_tokio::bluetooth_auto::BluetoothAuto;
use prns_simulation::ble::{
    BleMediumConfig, BleSimulationEvent, VirtualBleBackendConfig, VirtualBleBackendLimits,
    VirtualBleLab, VirtualBleLinkConfig, VirtualGattConfig,
};
use prns_simulation::{
    ManualMedium, ManualTaskRunner, ManualTimeDriver, Reachability, SimulationDurationInTicks,
    SimulationTick, TopologyConfig, TopologyMutation,
};

use super::scenario::{
    add_node, announce, destination, nonzero, request, settle, Completion, NodeControl, NodeRole,
    NodeSpec,
};

const NODE_COUNT: usize = 16;
const MAX_PEERS: usize = 4;
const NEIGHBOR_COUNT: usize = 2;
const DISCOVERY_BUDGET_MILLIS: u64 = 60_000;
const ADVERTISEMENT_INTERVAL_MILLIS: u64 = 20;
const GATT_VALUE_BYTES: usize = 20;
const PAYLOAD_BYTES: usize = 256;
const TRACE_CAPACITY: usize = 262_144;

#[track_caller]
fn advance_until(runner: &mut ManualTaskRunner<'_, Completion>, ready: impl Fn() -> bool) {
    let deadline = runner
        .snapshot()
        .unwrap_or_else(|error| unreachable!("coordinated BLE time: {error}"))
        .tick
        .get()
        + DISCOVERY_BUDGET_MILLIS;
    for _ in 0..=DISCOVERY_BUDGET_MILLIS * 2 {
        assert!(settle(runner).is_empty());
        if ready() {
            return;
        }
        let now = runner
            .snapshot()
            .unwrap_or_else(|error| unreachable!("coordinated BLE time: {error}"))
            .tick;
        assert!(
            now.get() < deadline,
            "BLE fleet discovery deadline at {deadline} ms"
        );
        assert!(runner
            .advance_to_next_event(SimulationTick::from_ticks(now.get() + 1))
            .is_ok());
    }
    unreachable!(
        "BLE fleet must converge within its discovery budget: {:?}",
        runner.snapshot()
    )
}

fn member_inventory(handle: &PrnsNodeHandle) -> Vec<(InterfaceId, ConnectionState)> {
    let mut members: Vec<_> = handle
        .interfaces()
        .into_iter()
        .filter_map(|interface| match interface.membership {
            Membership::Independent => None,
            Membership::FleetMember { .. } => Some((interface.id, interface.connection)),
        })
        .collect();
    members.sort_by_key(|(id, _)| *id);
    members
}

fn expected_neighbors(index: usize) -> Vec<(InterfaceId, ConnectionState)> {
    let mut members: Vec<_> = [
        (index + NODE_COUNT - 1) % NODE_COUNT,
        (index + 1) % NODE_COUNT,
    ]
    .into_iter()
    .map(|peer| {
        (
            InterfaceId::from_channel_tag(InterfaceKind::BluetoothPeer, &[peer as u8; 16]),
            ConnectionState::Connected,
        )
    })
    .collect();
    members.sort_by_key(|(id, _)| *id);
    members
}

fn exchange(
    runner: &mut ManualTaskRunner<'_, Completion>,
    controls: &[NodeControl],
    links: &BTreeMap<usize, LinkId>,
    nodes: std::ops::Range<usize>,
) {
    let expected: BTreeMap<_, _> = nodes
        .map(|node| {
            let bytes = vec![node as u8; PAYLOAD_BYTES];
            let task = request(
                runner,
                node,
                controls[node].handle.clone(),
                links[&node],
                bytes.clone(),
            );
            (task, Completion::Response { node, bytes })
        })
        .collect();
    assert_eq!(
        settle(runner).into_iter().collect::<BTreeMap<_, _>>(),
        expected
    );
}

#[test]
fn full_nodes_exchange_fragmented_ble_requests_and_recover_a_disabled_radio() {
    let lab = VirtualBleLab::new(
        BleMediumConfig::new(
            TopologyConfig::Explicit {
                max_neighbors: nonzero(NEIGHBOR_COUNT),
            },
            NODE_COUNT,
            NEIGHBOR_COUNT * 2,
            NODE_COUNT,
            TRACE_CAPACITY,
        )
        .unwrap_or_else(|error| unreachable!("bounded BLE ring: {error}")),
    );
    let addresses: Vec<_> = (0..NODE_COUNT)
        .map(|index| BleAddress::new([index as u8; 6]))
        .collect();
    let supervisors: Vec<_> = addresses
        .iter()
        .enumerate()
        .map(|(index, &address)| {
            let gatt = VirtualGattConfig::new(CONTROL_MAX_LEN, GATT_VALUE_BYTES)
                .unwrap_or_else(|error| unreachable!("fragmented GATT configuration: {error}"));
            let link = VirtualBleLinkConfig::new(4, 4, BLE_HW_MTU, gatt)
                .unwrap_or_else(|error| unreachable!("bounded GATT queues: {error}"));
            let backend = lab
                .attach_backend(
                    VirtualBleBackendConfig::new(
                        address,
                        -40,
                        BleRoleCapabilities::DualRole,
                        SimulationDurationInTicks::from_ticks(ADVERTISEMENT_INTERVAL_MILLIS),
                        VirtualBleBackendLimits {
                            inbound_links: nonzero(MAX_PEERS),
                            connections: nonzero(MAX_PEERS),
                            discovered_peers: nonzero(NEIGHBOR_COUNT),
                        },
                        link,
                    )
                    .unwrap_or_else(|error| unreachable!("bounded backend: {error}")),
                )
                .unwrap_or_else(|error| unreachable!("unique radio: {error}"));
            BluetoothAuto::<_, MAX_PEERS>::new(
                backend,
                BleIdentity::new([index as u8; 16]),
                if index % 2 == 0 {
                    Endpoint::CoreBluetooth(AppleHost::MacOs)
                } else {
                    Endpoint::BlueZ(BlueZHost::Linux)
                },
                LinkCapabilities {
                    l2cap: None,
                    link_mtu: BLE_HW_MTU as u16,
                },
            )
        })
        .collect();
    for index in 0..NODE_COUNT {
        assert_eq!(
            lab.set_reachability(
                addresses[index],
                addresses[(index + 1) % NODE_COUNT],
                Reachability::Reachable,
            ),
            Ok(TopologyMutation::Applied)
        );
    }
    let statuses: Vec<_> = supervisors.iter().map(BluetoothAuto::status).collect();
    let destinations: Vec<_> = (0..NODE_COUNT)
        .map(|index| {
            destination(index)
                .destination_hash()
                .unwrap_or_else(|error| unreachable!("valid BLE destination: {error:?}"))
        })
        .collect();
    let heard: Vec<_> = (0..NODE_COUNT)
        .map(|_| Rc::new(RefCell::new(Vec::new())))
        .collect();
    let mut driver =
        ManualTimeDriver::new(ManualMedium::Ble(lab.clone()), Duration::from_millis(1))
            .unwrap_or_else(|error| unreachable!("manual BLE fleet clock: {error}"));
    let mut runner = ManualTaskRunner::new(&mut driver, nonzero(NODE_COUNT * 2));
    let (node_tasks, pending_controls): (Vec<_>, Vec<_>) = supervisors
        .into_iter()
        .enumerate()
        .map(|(index, supervisor)| {
            add_node(
                &mut runner,
                NodeSpec {
                    index,
                    role: NodeRole::Endpoint,
                    attach_interfaces: move |handle: &PrnsNodeHandle| {
                        let _attached = handle.supervise(supervisor);
                    },
                    heard: heard[index].clone(),
                    heard_capacity: nonzero(NEIGHBOR_COUNT),
                },
            )
        })
        .unzip();
    assert!(settle(&mut runner).is_empty());
    let controls: Vec<_> = pending_controls
        .into_iter()
        .map(|mut receiver| {
            receiver
                .try_recv()
                .unwrap_or_else(|error| unreachable!("BLE node boot completed: {error}"))
        })
        .collect();
    advance_until(&mut runner, || {
        lab.active_connection_count() == NODE_COUNT
            && controls.iter().enumerate().all(|(index, control)| {
                member_inventory(&control.handle) == expected_neighbors(index)
            })
    });
    for (control, &destination) in controls.iter().zip(&destinations) {
        announce(control, destination);
    }
    advance_until(&mut runner, || {
        heard
            .iter()
            .all(|peers| peers.borrow().len() == NEIGHBOR_COUNT)
    });
    for index in 0..NODE_COUNT {
        let mut expected = vec![
            destinations[(index + NODE_COUNT - 1) % NODE_COUNT],
            destinations[(index + 1) % NODE_COUNT],
        ];
        expected.sort_by_key(|destination| *destination.as_bytes());
        assert_eq!(*heard[index].borrow(), expected);
    }
    for (index, control) in controls.iter().enumerate() {
        let handle = control.handle.clone();
        let remote = destinations[(index + 1) % NODE_COUNT];
        assert!(runner
            .insert(async move {
                let link = handle
                    .establish_link(remote)
                    .await
                    .unwrap_or_else(|error| unreachable!("BLE neighbor link: {error:?}"));
                Completion::Linked { node: index, link }
            })
            .is_ok());
    }
    let mut links = BTreeMap::new();
    for (_, output) in settle(&mut runner) {
        let Completion::Linked { node, link } = output else {
            unreachable!("only BLE links may complete")
        };
        assert!(links.insert(node, link).is_none());
    }
    assert_eq!(links.len(), NODE_COUNT);
    exchange(&mut runner, &controls, &links, 0..NODE_COUNT);

    statuses[0].disable();
    assert!(settle(&mut runner).is_empty());
    assert_eq!(statuses[0].connection(), ConnectionState::Disabled);
    assert_eq!(lab.active_connection_count(), NODE_COUNT - NEIGHBOR_COUNT);
    exchange(&mut runner, &controls, &links, 1..NODE_COUNT - 1);
    statuses[0].enable();
    advance_until(&mut runner, || {
        lab.active_connection_count() == NODE_COUNT
            && controls.iter().enumerate().all(|(index, control)| {
                member_inventory(&control.handle) == expected_neighbors(index)
            })
    });
    exchange(&mut runner, &controls, &links, 0..NODE_COUNT);
    for control in controls {
        assert_eq!(control.shutdown.send(()), Ok(()));
    }
    assert_eq!(
        settle(&mut runner).into_iter().collect::<BTreeMap<_, _>>(),
        node_tasks
            .into_iter()
            .enumerate()
            .map(|(node, task)| (
                task,
                Completion::Stopped {
                    node,
                    result: Ok(())
                }
            ))
            .collect()
    );
    assert_eq!(runner.task_count(), 0);
    assert_eq!(lab.active_connection_count(), 0);
    let trace = lab.trace();
    assert_eq!(trace.discarded_events, 0);
    assert!(!trace
        .events
        .iter()
        .any(|event| matches!(event, BleSimulationEvent::ObservationDropped { .. })));
    let mut attached = Vec::new();
    let mut detached = Vec::new();
    for event in trace.events {
        match event {
            BleSimulationEvent::RadioAttached { radio } => attached.push(radio),
            BleSimulationEvent::RadioDetached { radio } => detached.push(radio),
            _ => {}
        }
    }
    attached.sort();
    detached.sort();
    assert_eq!(attached.len(), NODE_COUNT);
    assert_eq!(detached, attached);
}
