use super::adapter::{Node, Runtime};
use super::{Reply, TimedReply, DISCOVERY_BUDGET_MS};
use crate::clock::{ClockLease, CompletionBudget, EmbassyTasks};
use crate::tick;
use personal_rns::engine::RequestResponseTimeout;
use personal_rns::interfaces::bluetooth_auto::BleAddress;
use personal_rns::interfaces::{ConnectionState, InterfaceId, InterfaceKind};
use personal_rns::routing::links::LinkId;
use personal_rns::units::DurationMillis;
use personal_rns::wire::{
    ContextFlag, DestinationType, IfacFlag, PacketType, PropagationType, WireContext,
    WirePacketHeader,
};
use prns_simulation::ble::{BleMediumConfig, BleSimulationEvent, BleWireCapture, VirtualBleLab};
use prns_simulation::{
    ManualMedium, ManualTaskScheduling, ManualTimeDriver, Reachability, TopologyConfig,
    TopologyMutation,
};
use std::num::NonZeroUsize;
use std::time::Duration;

const ADDRESSES: [u8; 2] = [1, 2];
const CAPTURE_CAPACITY: usize = 4096;

pub(super) struct Pair<'scenario, 'driver> {
    tasks: &'scenario mut EmbassyTasks<'driver>,
    lab: &'scenario VirtualBleLab,
    nodes: &'scenario [Node; 2],
    capture: &'scenario BleWireCapture,
    links: [LinkId; 2],
    retirements: Vec<usize>,
}

impl Pair<'_, '_> {
    pub(super) fn now(&self) -> u64 {
        self.tasks.snapshot().tick.get()
    }

    pub(super) fn exchange(&mut self, data: Vec<u8>) -> [Reply; 2] {
        let handles = self.nodes.each_ref().map(Node::handle);
        let links = self.links;
        let replies = self.tasks.complete_ready(async move {
            let (left, right) = tokio::join!(
                handles[0].request(links[0], &data, RequestResponseTimeout::LinkDefault),
                handles[1].request(links[1], &data, RequestResponseTimeout::LinkDefault),
            );
            [left, right]
        });
        for node in self.nodes {
            node.drain_fixture_diagnostics();
        }
        replies
    }

    pub(super) fn expire(&mut self, timeout_ms: u64) -> [TimedReply; 2] {
        for (index, link) in self.links.iter().enumerate() {
            self.nodes[index ^ 1]
                .wire()
                .lose_after(response_header(*link), 0, NonZeroUsize::MIN);
        }
        let handles = self.nodes.each_ref().map(Node::handle);
        let links = self.links;
        let expired = self.tasks.complete_with_budget(
            CompletionBudget {
                deadline: tick(self.now() + timeout_ms),
                polls_per_tick: NonZeroUsize::new(256).unwrap(),
            },
            async move {
                let (left, right) = tokio::join!(
                    expiring_request(&handles[0], links[0], timeout_ms),
                    expiring_request(&handles[1], links[1], timeout_ms),
                );
                [left, right]
            },
        );
        for node in self.nodes {
            assert_eq!(node.wire().stop_loss(), 1);
        }
        expired
    }

    pub(super) fn cancel(&mut self) {
        for _ in 0..crate::node::REQUEST_CAPACITY {
            for (index, link) in self.links.iter().copied().enumerate() {
                let gate = self.nodes[index ^ 1].wire().clone();
                gate.lose_after(response_header(link), 0, NonZeroUsize::MIN);
                let handle = self.nodes[index].handle();
                self.tasks.complete_ready(async move {
                    tokio::select! {
                        biased;
                        reply = handle.request(link, &[42], RequestResponseTimeout::LinkDefault) => unreachable!("dropped response completed: {reply:?}"),
                        _ = gate.first_loss() => {},
                    }
                });
                assert_eq!(self.nodes[index ^ 1].wire().stop_loss(), 1);
            }
        }
    }

    pub(super) fn reconnect(&mut self) -> u64 {
        let boundary = self.capture.snapshot();
        let before = self.now();
        let addresses = ADDRESSES.map(|address| BleAddress::new([address; 6]));
        assert_eq!(
            self.lab
                .set_reachability(addresses[0], addresses[1], Reachability::Isolated),
            Ok(TopologyMutation::Applied)
        );
        self.tasks.settle();
        assert_eq!(self.lab.active_connection_count(), 0);
        assert_eq!(self.nodes.each_ref().map(Node::members), [vec![], vec![]]);
        assert_eq!(self.capture.snapshot(), boundary);
        self.retirements.push(boundary.values.len());
        assert_eq!(
            self.lab
                .set_reachability(addresses[0], addresses[1], Reachability::Reachable),
            Ok(TopologyMutation::Applied)
        );
        converge(self.tasks, self.lab, self.nodes);
        let elapsed = self.now() - before;
        let fresh = establish(self.tasks, self.nodes);
        for link in fresh {
            assert!(!self.links.contains(&link));
        }
        self.links = fresh;
        elapsed
    }
}

async fn expiring_request(
    handle: &super::adapter::Handle,
    link: LinkId,
    timeout_ms: u64,
) -> TimedReply {
    let before = embassy_time::Instant::now().as_millis();
    let reply = handle
        .request(
            link,
            &[42],
            RequestResponseTimeout::Exact(DurationMillis(timeout_ms)),
        )
        .await;
    TimedReply {
        reply,
        elapsed_ms: embassy_time::Instant::now().as_millis() - before,
    }
}

pub(super) fn with_pair<T>(
    runtimes: [Runtime; 2],
    scheduling: ManualTaskScheduling,
    scenario: impl FnOnce(&mut Pair<'_, '_>) -> T,
) -> T {
    let clock = ClockLease::acquire();
    let capture = BleWireCapture::new(NonZeroUsize::new(CAPTURE_CAPACITY).unwrap());
    let lab = VirtualBleLab::with_wire_capture(
        BleMediumConfig::new(
            TopologyConfig::Explicit {
                max_neighbors: NonZeroUsize::MIN,
            },
            2,
            4,
            4,
            CAPTURE_CAPACITY,
        )
        .unwrap(),
        capture.clone(),
    );
    let mut driver =
        ManualTimeDriver::new(ManualMedium::Ble(lab.clone()), Duration::from_millis(1)).unwrap();
    let mut tasks = EmbassyTasks::with_limits(
        &mut driver,
        clock,
        NonZeroUsize::new(8).unwrap(),
        NonZeroUsize::new(256).unwrap(),
        scheduling,
    );
    let nodes = std::array::from_fn(|index| {
        Node::start(runtimes[index], &mut tasks, &lab, ADDRESSES[index])
    });
    let addresses = ADDRESSES.map(|address| BleAddress::new([address; 6]));
    assert_eq!(
        lab.set_reachability(addresses[0], addresses[1], Reachability::Reachable),
        Ok(TopologyMutation::Applied)
    );
    converge(&mut tasks, &lab, &nodes);
    let links = establish(&mut tasks, &nodes);
    let (result, retirements) = {
        let mut pair = Pair {
            tasks: &mut tasks,
            lab: &lab,
            nodes: &nodes,
            capture: &capture,
            links,
            retirements: Vec::new(),
        };
        let result = scenario(&mut pair);
        (result, pair.retirements)
    };
    drop(nodes);
    drop(tasks);
    assert_eq!(lab.active_connection_count(), 0);
    let wire = capture.snapshot();
    assert_eq!(wire.discarded_values, 0);
    for boundary in retirements {
        let (old, new) = wire.values.split_at(boundary);
        assert!(!old.is_empty() && !new.is_empty());
        let mut retired = Vec::new();
        for value in old {
            if !retired.contains(&value.connection) {
                retired.push(value.connection);
            }
        }
        assert!(
            new.iter().all(|value| !retired.contains(&value.connection)),
            "retired connection identities cannot carry recovery traffic: {runtimes:?}"
        );
    }
    let trace = lab.trace();
    assert_eq!(trace.discarded_events, 0);
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
    assert_eq!(attached.len(), 2);
    assert_eq!(attached, detached);
    result
}

fn converge(tasks: &mut EmbassyTasks<'_>, lab: &VirtualBleLab, nodes: &[Node; 2]) {
    let expected = ADDRESSES.map(|address| {
        vec![(
            InterfaceId::from_channel_tag(InterfaceKind::BluetoothPeer, &[address; 16]),
            ConnectionState::Connected,
        )]
    });
    let expected = [expected[1].clone(), expected[0].clone()];
    let horizon = tick(tasks.snapshot().tick.get() + DISCOVERY_BUDGET_MS);
    for _ in 0..32 {
        tasks.settle();
        if nodes.each_ref().map(Node::members) == expected {
            assert_eq!(lab.active_connection_count(), 1);
            return;
        }
        assert!(
            tasks.snapshot().tick < horizon,
            "contract discovery deadline: members={:?}, trace={:?}",
            nodes.each_ref().map(Node::members),
            lab.trace().events.iter().rev().take(12).collect::<Vec<_>>()
        );
        tasks.advance_to_next_wake(horizon).unwrap();
    }
    unreachable!("bounded contract discovery");
}

fn establish(tasks: &mut EmbassyTasks<'_>, nodes: &[Node; 2]) -> [LinkId; 2] {
    let handles = nodes.each_ref().map(Node::handle);
    tasks.complete_ready(async move {
        tokio::join!(
            handles[0].announce(ADDRESSES[0]),
            handles[1].announce(ADDRESSES[1])
        );
    });
    let handles = nodes.each_ref().map(Node::handle);
    let links = tasks.complete_ready(async move {
        let (left, right) = tokio::join!(
            handles[0].establish(ADDRESSES[1]),
            handles[1].establish(ADDRESSES[0])
        );
        [left, right]
    });
    assert_ne!(links[0], links[1]);
    links
}

fn response_header(link: LinkId) -> WirePacketHeader {
    WirePacketHeader {
        ifac_flag: IfacFlag::Open,
        context_flag: ContextFlag::Unset,
        propagation: PropagationType::Broadcast,
        destination_type: DestinationType::Link,
        packet_type: PacketType::Data,
        hops: 0,
        transport_id: None,
        address: link.to_address(),
        context: WireContext::Response,
    }
}
