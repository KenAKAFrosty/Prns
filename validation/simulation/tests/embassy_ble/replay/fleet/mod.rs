use super::*;
mod deadlines;
#[cfg(feature = "heap-profile")]
mod heap;
use personal_rns::interfaces::bluetooth_auto::BleAddress;
use personal_rns::interfaces::{ConnectionState, InterfaceId, InterfaceKind};
use personal_rns::storage::GrowableHeap;
use prns_simulation::{ManualTaskScheduling, Reachability, SimulationSeed, TopologyMutation};
use std::future::{poll_fn, Future};
use std::task::Poll;

const TRACE_EVENTS_PER_NODE: usize = 64;
const WIRE_VALUES_PER_NODE: usize = 256;
const PAYLOAD_BYTES: usize = 256;

#[derive(Debug, PartialEq, Eq)]
struct FleetTranscript {
    wire: BleWireSnapshot,
    discovery: BleTraceSnapshot,
    responses: Vec<Vec<u8>>,
    expirations: Vec<u64>,
    timers: crate::clock::QueueStats,
}

enum Workload {
    Echo,
    ExpiredReplies,
}

async fn together<T>(futures: Vec<impl Future<Output = T>>) -> Vec<T> {
    let mut pending: Vec<_> = futures
        .into_iter()
        .map(|future| Some(Box::pin(future)))
        .collect();
    let mut results: Vec<_> = (0..pending.len()).map(|_| None).collect();
    poll_fn(|cx| {
        for (future, result) in pending.iter_mut().zip(&mut results) {
            if let Some(active) = future {
                if let Poll::Ready(value) = active.as_mut().poll(cx) {
                    *result = Some(value);
                    *future = None;
                }
            }
        }
        if pending.iter().all(Option::is_none) {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await;
    results.into_iter().map(Option::unwrap).collect()
}

fn address(index: usize) -> BleAddress {
    BleAddress::new([u8::try_from(index).unwrap(); 6])
}

fn run<const NODES: usize>(scheduling: ManualTaskScheduling, marker: u8) -> FleetTranscript {
    run_workload::<NODES>(scheduling, marker, Workload::Echo)
}

fn run_workload<const NODES: usize>(
    scheduling: ManualTaskScheduling,
    marker: u8,
    workload: Workload,
) -> FleetTranscript {
    const {
        assert!(NODES >= 4 && NODES <= 16 && NODES.is_multiple_of(2));
    }
    let lease = ClockLease::acquire();
    let capture = BleWireCapture::new(NonZeroUsize::new(NODES * WIRE_VALUES_PER_NODE).unwrap());
    let lab = VirtualBleLab::with_wire_capture(
        BleMediumConfig::new(
            TopologyConfig::Explicit {
                max_neighbors: NonZeroUsize::new(1).unwrap(),
            },
            NODES,
            4,
            NODES,
            NODES * TRACE_EVENTS_PER_NODE,
        )
        .unwrap(),
        capture.clone(),
    );
    let mut driver =
        ManualTimeDriver::new(ManualMedium::Ble(lab.clone()), Duration::from_millis(1)).unwrap();
    let mut tasks = EmbassyTasks::with_limits(
        &mut driver,
        lease,
        NonZeroUsize::new(NODES + 1).unwrap(),
        NonZeroUsize::new(NODES * 128).unwrap(),
        scheduling,
    );
    let nodes: Vec<_> = (0..NODES)
        .map(|index| {
            let address = u8::try_from(index).unwrap();
            node::Node::with_storage_inputs(
                &mut tasks,
                &lab,
                node::NodeInputs {
                    address,
                    endpoint: if index.is_multiple_of(2) {
                        Endpoint::Esp32(Esp32Host::Esp32)
                    } else {
                        Endpoint::Nrf52(Nrf52Host::Nrf52)
                    },
                    entropy_seed: address + 1,
                },
                [echo::destination(address)],
                personal_rns::request_endpoints![echo::Echo],
                GrowableHeap,
            )
        })
        .collect();
    for index in (0..NODES).step_by(2) {
        assert_eq!(
            lab.set_reachability(address(index), address(index + 1), Reachability::Reachable),
            Ok(TopologyMutation::Applied)
        );
    }
    for step in 0..32 {
        tasks.settle();
        if nodes.iter().all(|node| node.status.members().count() == 1) {
            break;
        }
        assert!(step < 31, "bounded fleet discovery");
        tasks.advance_to_next_wake(tick(60_000)).unwrap();
    }
    assert_eq!(lab.active_connection_count(), NODES / 2);
    for (index, node) in nodes.iter().enumerate() {
        assert_eq!(
            node.status
                .members()
                .map(|member| (member.id(), member.connection()))
                .collect::<Vec<_>>(),
            vec![(
                InterfaceId::from_channel_tag(
                    InterfaceKind::BluetoothPeer,
                    &[u8::try_from(index ^ 1).unwrap(); 16]
                ),
                ConnectionState::Connected
            )]
        );
    }
    let announce = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            let handle = node.handle;
            async move {
                handle
                    .announce_now(AnnounceNow {
                        destination: echo::destination(index as u8).destination_hash().unwrap(),
                        target: AnnounceTarget::AllInterfaces,
                        app_data: AnnounceAppData::Registered,
                    })
                    .await
                    .unwrap();
            }
        })
        .collect();
    tasks.complete_ready(together(announce));
    let connect = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            let handle = node.handle;
            async move {
                handle
                    .establish_link(
                        echo::destination((index ^ 1) as u8)
                            .destination_hash()
                            .unwrap(),
                    )
                    .await
                    .unwrap()
            }
        })
        .collect();
    let links = tasks.complete_ready(together(connect));
    let expirations = match workload {
        Workload::Echo => Vec::new(),
        Workload::ExpiredReplies => deadlines::expire(&mut tasks, &nodes, &links),
    };
    let requests = nodes
        .iter()
        .zip(links)
        .enumerate()
        .map(|(index, (node, link))| {
            let handle = node.handle;
            async move {
                let mut payload = [marker; PAYLOAD_BYTES];
                payload[0] = index as u8;
                let bytes = handle
                    .request(link, RequestPathHash::of(echo::QUERY_PATH), &payload)
                    .await
                    .unwrap()
                    .0
                    .as_slice()
                    .to_vec();
                assert_eq!(bytes, payload);
                bytes
            }
        })
        .collect();
    let responses = tasks.complete_ready(together(requests));
    let timers = tasks.timer_stats();
    for node in nodes {
        node.stop(&mut tasks);
    }
    drop(tasks);
    assert_eq!(lab.active_connection_count(), 0);
    assert_eq!(crate::static_storage::footprint().blocks, NODES * 8);
    let transcript = FleetTranscript {
        wire: capture.snapshot(),
        discovery: lab.trace(),
        responses,
        expirations,
        timers,
    };
    assert_eq!(transcript.wire.discarded_values, 0);
    assert_eq!(transcript.discovery.discarded_events, 0);
    let mut attached = Vec::new();
    let mut detached = Vec::new();
    for event in &transcript.discovery.events {
        match event {
            prns_simulation::ble::BleSimulationEvent::RadioAttached { radio } => {
                attached.push(*radio)
            }
            prns_simulation::ble::BleSimulationEvent::RadioDetached { radio } => {
                detached.push(*radio)
            }
            _ => {}
        }
    }
    attached.sort();
    detached.sort();
    assert_eq!(attached.len(), NODES);
    assert_eq!(attached, detached);
    transcript
}

#[test]
fn embassy_paired_fleets_replay_concurrent_bidirectional_requests() {
    for scheduling in [
        ManualTaskScheduling::Cyclic,
        ManualTaskScheduling::Seeded {
            seed: SimulationSeed::new(7),
        },
        ManualTaskScheduling::Seeded {
            seed: SimulationSeed::new(41),
        },
    ] {
        let expected = run::<8>(scheduling, 42);
        for _ in 0..3 {
            assert_eq!(run::<8>(scheduling, 42), expected);
        }
        let changed = run::<8>(scheduling, 43);
        assert_ne!(changed.responses, expected.responses);
        assert_ne!(changed.wire, expected.wire);
        let large = run::<16>(scheduling, 42);
        assert_eq!(run::<16>(scheduling, 42), large);
    }
}
