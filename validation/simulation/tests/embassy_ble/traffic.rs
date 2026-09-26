use std::num::NonZeroUsize;
use std::time::Duration;

use personal_rns::engine::{EgressTarget, InstantMillis, Settlement};
use personal_rns::interfaces::bluetooth_auto::{BleAddress, Endpoint, Esp32Host, Nrf52Host};
use personal_rns::interfaces::{ConnectionState, InterfaceId, InterfaceKind, InterfaceStatus};
use personal_rns::wire::{WireContext, HEADER_MIN_LEN};
use prns_simulation::ble::{BleMediumConfig, VirtualBleLab};
use prns_simulation::{
    ManualMedium, ManualTimeDriver, Reachability, SimulationTick, TopologyConfig, TopologyMutation,
};

use super::clock::{ClockLease, EmbassyTasks};
use super::fixture::{GATT_QUEUE_DEPTH, GATT_VALUE_BYTES};
use super::node::{destination, Node, Received, PAYLOAD_BYTES};
use super::tick;

const NODE_COUNT: usize = 3;
const DISCOVERY_BUDGET_MS: u64 = 60_000;
const TRACE_CAPACITY: usize = 256;

const _: () = assert!(PAYLOAD_BYTES + HEADER_MIN_LEN > GATT_VALUE_BYTES * GATT_QUEUE_DEPTH);

fn peer(address: u8) -> InterfaceId {
    InterfaceId::from_channel_tag(InterfaceKind::BluetoothPeer, &[address; 16])
}

fn members(node: &Node) -> Vec<(InterfaceId, ConnectionState)> {
    let mut result: Vec<_> = node
        .status
        .members()
        .map(|member| (member.id(), member.connection()))
        .collect();
    result.sort_by_key(|(id, _)| *id);
    result
}

fn converge(tasks: &mut EmbassyTasks<'_>, lab: &VirtualBleLab, nodes: &[Node; NODE_COUNT]) {
    let mut expected_hub = vec![
        (peer(2), ConnectionState::Connected),
        (peer(3), ConnectionState::Connected),
    ];
    expected_hub.sort_by_key(|(id, _)| *id);
    let expected = [
        expected_hub,
        vec![(peer(1), ConnectionState::Connected)],
        vec![(peer(1), ConnectionState::Connected)],
    ];
    let deadline = tasks.snapshot().tick.get() + DISCOVERY_BUDGET_MS;
    for _ in 0..=DISCOVERY_BUDGET_MS * 2 {
        let _ = tasks.settle();
        if lab.active_connection_count() == 2 && nodes.each_ref().map(members) == expected {
            return;
        }
        let now = tasks.snapshot().tick.get();
        assert!(now < deadline, "Embassy BLE discovery must converge");
        let _ = tasks.advance(tick(now + 1)).unwrap();
    }
    unreachable!("bounded discovery step count")
}

fn exchange(
    tasks: &mut EmbassyTasks<'_>,
    nodes: &[Node; NODE_COUNT],
    round: u8,
    hub_target: EgressTarget,
) {
    let before = tasks.snapshot();
    let counters = |node: &Node| {
        let mut counters: Vec<_> = node
            .status
            .members()
            .map(|member| (member.id(), member.rx_bytes(), member.tx_bytes()))
            .collect();
        counters.sort_by_key(|(id, _, _)| *id);
        counters
    };
    let before_counters = nodes.each_ref().map(counters);
    let payloads: [_; NODE_COUNT] = std::array::from_fn(|index| {
        let mut payload = vec![index as u8; PAYLOAD_BYTES];
        payload[0] = round;
        payload
    });
    let ids = [
        nodes[0].send(hub_target, &payloads[0]),
        nodes[1].send(EgressTarget::AllInterfaces, &payloads[1]),
        nodes[2].send(EgressTarget::AllInterfaces, &payloads[2]),
    ];
    assert!(tasks.settle() > 0);
    assert_eq!(
        tasks.snapshot(),
        before,
        "traffic settles without advancing time"
    );
    for (node, id) in nodes.iter().zip(ids) {
        assert_eq!(
            node.take_settled(),
            vec![(id, Settlement::SendPlainPacket(Ok(())))]
        );
    }
    let expected = |sender: usize| Received {
        destination: destination().destination_hash().unwrap(),
        source: peer((sender + 1) as u8),
        context: WireContext::None,
        arrived_at: InstantMillis(before.runtime_elapsed.as_millis().try_into().unwrap()),
        payload: payloads[sender].clone(),
    };
    let mut hub_received = vec![expected(1), expected(2)];
    hub_received.sort_by_key(|event| event.source);
    assert_eq!(
        nodes[0].take_received(),
        hub_received,
        "member counters: {:?}",
        nodes.each_ref().map(counters)
    );
    let selected = |id| match hub_target {
        EgressTarget::AllInterfaces => true,
        EgressTarget::Interface(target) => target == id,
    };
    for (index, node) in nodes.iter().enumerate().skip(1) {
        assert_eq!(
            node.take_received(),
            if selected(peer((index + 1) as u8)) {
                vec![expected(0)]
            } else {
                vec![]
            }
        );
    }

    let frame_bytes = (PAYLOAD_BYTES + HEADER_MIN_LEN) as u64;
    let expected_counters: [_; NODE_COUNT] = std::array::from_fn(|index| {
        before_counters[index]
            .iter()
            .map(|&(id, rx, tx)| {
                if index == 0 {
                    (
                        id,
                        rx + frame_bytes,
                        tx + u64::from(selected(id)) * frame_bytes,
                    )
                } else {
                    (
                        id,
                        rx + u64::from(selected(peer((index + 1) as u8))) * frame_bytes,
                        tx + frame_bytes,
                    )
                }
            })
            .collect::<Vec<_>>()
    });
    assert_eq!(nodes.each_ref().map(counters), expected_counters);
}

#[test]
fn embassy_nodes_keep_fragmented_fanout_duplex_and_recover_a_disabled_hub() {
    let clock = ClockLease::acquire();
    let lab = VirtualBleLab::new(
        BleMediumConfig::new(
            TopologyConfig::Explicit {
                max_neighbors: NonZeroUsize::new(2).unwrap(),
            },
            NODE_COUNT,
            4,
            8,
            TRACE_CAPACITY,
        )
        .unwrap(),
    );
    let mut driver =
        ManualTimeDriver::new(ManualMedium::Ble(lab.clone()), Duration::from_millis(1)).unwrap();
    let mut tasks = EmbassyTasks::new(&mut driver, clock);
    let nodes = [
        Node::start(&mut tasks, &lab, 1, Endpoint::Esp32(Esp32Host::Esp32)),
        Node::start(&mut tasks, &lab, 2, Endpoint::Nrf52(Nrf52Host::Nrf52)),
        Node::start(&mut tasks, &lab, 3, Endpoint::Esp32(Esp32Host::Esp32)),
    ];
    for address in [2, 3] {
        assert_eq!(
            lab.set_reachability(
                BleAddress::new([1; 6]),
                BleAddress::new([address; 6]),
                Reachability::Reachable,
            ),
            Ok(TopologyMutation::Applied)
        );
    }
    converge(&mut tasks, &lab, &nodes);
    assert_eq!(tasks.snapshot().tick, SimulationTick::ZERO);
    for round in 0..4 {
        exchange(&mut tasks, &nodes, round, EgressTarget::AllInterfaces);
        exchange(
            &mut tasks,
            &nodes,
            round + 4,
            EgressTarget::Interface(peer(2)),
        );
    }
    for node in &nodes {
        assert_eq!(
            (
                node.status.setup_failure_events(),
                node.status.transport_closure_events()
            ),
            (0, 0)
        );
    }

    nodes[0].status.disable();
    assert!(tasks.settle() > 0);
    assert_eq!(lab.active_connection_count(), 0);
    assert_eq!(nodes.each_ref().map(members), [vec![], vec![], vec![]]);
    nodes[0].status.enable();
    let _ = tasks.settle();
    converge(&mut tasks, &lab, &nodes);
    exchange(&mut tasks, &nodes, 8, EgressTarget::AllInterfaces);
    exchange(&mut tasks, &nodes, 9, EgressTarget::Interface(peer(3)));
    for node in &nodes {
        assert_eq!(node.status.setup_failure_events(), 0);
        assert!(node.take_received().is_empty());
        assert!(node.take_settled().is_empty());
    }
    drop(tasks);
    assert_eq!(lab.active_connection_count(), 0);
}
