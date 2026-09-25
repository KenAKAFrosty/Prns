#![cfg(feature = "controlled-time")]

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use personal_rns::engine::{RequestResponseTimeout, SendRequestFailure};
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::runtime::SendError;
use personal_rns::units::DurationMillis;
use prns_simulation::{
    FaultPlan, ManualMedium, ManualTaskRunner, ManualTimeDriver, ManualTimeSnapshot, MediumEvent,
    Reachability, SimulationDurationInTicks, SimulationTick, TopologyConfig, TopologyMutation,
    TransmissionOrdinal, TransmissionRule, VirtualMedium, VirtualMediumConfig,
};

mod ble;
mod routing;
mod scenario;
use scenario::{
    add_node, announce, destination, nonzero, request, settle, Completion, NodeRole, NodeSpec,
    NODE_COUNT, QUERY_PATH,
};

const REQUEST_TIMEOUT_MILLIS: u64 = 50;
const RECEIVE_QUEUE_CAPACITY: usize = 16;
const TRACE_CAPACITY: usize = NODE_COUNT * 64;

fn tick(value: u64) -> SimulationTick {
    SimulationTick::from_ticks(value)
}

#[test]
fn full_nodes_exchange_in_a_sparse_ring_then_survive_a_partition_and_shut_down() {
    let faults = FaultPlan::new(
        (0..NODE_COUNT as u64)
            .map(|ordinal| {
                TransmissionRule::delay(
                    TransmissionOrdinal::new(ordinal),
                    SimulationDurationInTicks::from_ticks(1),
                )
            })
            .collect(),
    )
    .unwrap_or_else(|error| unreachable!("ordered announce delays: {error}"));
    let config = VirtualMediumConfig::new(
        TopologyConfig::Explicit {
            max_neighbors: nonzero(2),
        },
        NODE_COUNT,
        RECEIVE_QUEUE_CAPACITY,
        NODE_COUNT * 2,
        TRACE_CAPACITY,
        faults,
    )
    .unwrap_or_else(|error| unreachable!("bounded ring medium: {error}"));
    let medium = VirtualMedium::new(config);
    let interfaces: Vec<_> = (0..NODE_COUNT)
        .map(|index| {
            medium
                .attach(&(index as u64).to_be_bytes())
                .unwrap_or_else(|error| unreachable!("unique interface: {error}"))
        })
        .collect();
    let endpoints: Vec<_> = interfaces
        .iter()
        .map(|interface| interface.endpoint_id())
        .collect();
    for index in 0..NODE_COUNT {
        assert_eq!(
            medium.set_reachability(
                endpoints[index],
                endpoints[(index + 1) % NODE_COUNT],
                Reachability::Reachable
            ),
            Ok(TopologyMutation::Applied)
        );
    }
    let destinations: Vec<_> = (0..NODE_COUNT)
        .map(|index| {
            destination(index)
                .destination_hash()
                .unwrap_or_else(|error| unreachable!("valid destination: {error:?}"))
        })
        .collect();
    let heard: Vec<_> = (0..NODE_COUNT)
        .map(|_| Rc::new(RefCell::new(Vec::new())))
        .collect();
    let mut driver = ManualTimeDriver::new(
        ManualMedium::Frames(medium.clone()),
        Duration::from_millis(1),
    )
    .unwrap_or_else(|error| unreachable!("manual fleet clock: {error}"));
    let mut runner = ManualTaskRunner::new(&mut driver, nonzero(NODE_COUNT * 2));
    let (node_tasks, pending_controls): (Vec<_>, Vec<_>) = interfaces
        .into_iter()
        .enumerate()
        .map(|(index, interface)| {
            add_node(
                &mut runner,
                NodeSpec {
                    index,
                    role: NodeRole::Endpoint,
                    attach_interfaces: move |handle: &personal_rns::runtime::PrnsNodeHandle| {
                        let _attached = handle.add_interface(interface);
                    },
                    heard: heard[index].clone(),
                    heard_capacity: nonzero(2),
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
                .unwrap_or_else(|error| unreachable!("node has initialized: {error}"))
        })
        .collect();

    for (control, &destination) in controls.iter().zip(&destinations) {
        announce(control, destination);
    }
    assert!(settle(&mut runner).is_empty());
    assert!(heard.iter().all(|peers| peers.borrow().is_empty()));
    assert_eq!(medium.pending_delivery_count(), NODE_COUNT * 2);
    assert!(runner.advance_to_next_event(tick(1)).is_ok());
    assert!(settle(&mut runner).is_empty());
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
                    .unwrap_or_else(|error| unreachable!("neighbor link: {error:?}"));
                Completion::Linked { node: index, link }
            })
            .is_ok());
    }
    let mut links = BTreeMap::new();
    for (_, output) in settle(&mut runner) {
        let Completion::Linked { node, link } = output else {
            unreachable!("only link operations may complete")
        };
        assert!(links.insert(node, link).is_none());
    }
    assert_eq!(links.len(), NODE_COUNT);

    for (index, control) in controls.iter().enumerate() {
        request(
            &mut runner,
            index,
            control.handle.clone(),
            links[&index],
            (index as u64).to_be_bytes().to_vec(),
        );
    }
    let responses: BTreeMap<_, _> = settle(&mut runner)
        .into_iter()
        .map(|(_, output)| {
            let Completion::Response { node, bytes } = output else {
                unreachable!("only responses may complete")
            };
            (node, bytes)
        })
        .collect();
    assert_eq!(
        responses,
        (0..NODE_COUNT)
            .map(|node| (node, (node as u64).to_be_bytes().to_vec()))
            .collect()
    );

    assert_eq!(
        medium.set_reachability(endpoints[0], endpoints[1], Reachability::Isolated),
        Ok(TopologyMutation::Applied)
    );
    let failed = controls[0].handle.clone();
    let link = links[&0];
    let timeout_task = runner
        .insert(async move {
            assert_eq!(
                failed
                    .request_with_response_timeout(
                        link,
                        RequestPathHash::of(QUERY_PATH),
                        b"partitioned",
                        RequestResponseTimeout::Exact(DurationMillis(REQUEST_TIMEOUT_MILLIS))
                    )
                    .await,
                Err(SendError::Failed(SendRequestFailure::Timeout))
            );
            Completion::TimedOut { node: 0 }
        })
        .unwrap_or_else(|error| unreachable!("bounded request admission: {error}"));
    let unaffected_tasks: BTreeMap<_, _> = controls
        .iter()
        .enumerate()
        .skip(1)
        .map(|(index, control)| {
            let task = request(
                &mut runner,
                index,
                control.handle.clone(),
                links[&index],
                b"unaffected".to_vec(),
            );
            (
                task,
                Completion::Response {
                    node: index,
                    bytes: b"unaffected".to_vec(),
                },
            )
        })
        .collect();
    assert_eq!(
        settle(&mut runner).into_iter().collect::<BTreeMap<_, _>>(),
        unaffected_tasks
    );
    for elapsed in 1..REQUEST_TIMEOUT_MILLIS {
        assert!(runner.advance_to_next_event(tick(1 + elapsed)).is_ok());
        assert!(
            settle(&mut runner).is_empty(),
            "request cannot time out early"
        );
    }
    assert!(runner
        .advance_to_next_event(tick(1 + REQUEST_TIMEOUT_MILLIS))
        .is_ok());
    assert_eq!(
        settle(&mut runner),
        vec![(timeout_task, Completion::TimedOut { node: 0 })]
    );

    assert_eq!(
        medium.set_reachability(endpoints[0], endpoints[1], Reachability::Reachable),
        Ok(TopologyMutation::Applied)
    );
    let recovery_task = request(
        &mut runner,
        0,
        controls[0].handle.clone(),
        link,
        b"recovered".to_vec(),
    );
    assert_eq!(
        settle(&mut runner),
        vec![(
            recovery_task,
            Completion::Response {
                node: 0,
                bytes: b"recovered".to_vec()
            }
        )]
    );
    let elapsed = 1 + REQUEST_TIMEOUT_MILLIS;
    assert_eq!(
        runner.snapshot().ok(),
        Some(ManualTimeSnapshot {
            tick: tick(elapsed),
            runtime_elapsed: Duration::from_millis(elapsed)
        })
    );
    // Each node may have a different wall-clock origin; elapsed monotonic time must agree exactly.
    for (node, control) in controls.iter().enumerate() {
        let clock = control.clock.clone();
        let origin = control.origin;
        assert!(runner
            .insert(async move {
                Completion::Clock {
                    node,
                    elapsed: clock.now().duration_since(origin),
                }
            })
            .is_ok());
    }
    let observations: Vec<_> = settle(&mut runner)
        .into_iter()
        .map(|(_, output)| output)
        .collect();
    assert_eq!(
        observations,
        (0..NODE_COUNT)
            .map(|node| Completion::Clock {
                node,
                elapsed: DurationMillis(elapsed)
            })
            .collect::<Vec<_>>()
    );
    for control in controls {
        assert_eq!(control.shutdown.send(()), Ok(()));
    }
    let stopped: BTreeMap<_, _> = settle(&mut runner).into_iter().collect();
    assert_eq!(
        stopped,
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
    assert_eq!(medium.pending_delivery_count(), 0);
    let trace = medium.trace();
    assert_eq!(trace.discarded_events, 0);
    assert!(!trace
        .events
        .iter()
        .any(|event| matches!(event, MediumEvent::ReceptionDropped { .. })));
    let mut detached: Vec<_> = trace
        .events
        .iter()
        .filter_map(|event| match event {
            MediumEvent::EndpointDetached { endpoint } => Some(*endpoint),
            _ => None,
        })
        .collect();
    detached.sort();
    assert_eq!(detached, endpoints);
}
