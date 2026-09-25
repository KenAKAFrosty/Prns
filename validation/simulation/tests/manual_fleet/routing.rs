use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use personal_rns::engine::{RequestResponseTimeout, SendRequestFailure};
use personal_rns::manifold::interface_seam::Interface;
use personal_rns::node_introspection::NodeIntrospection;
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::runtime::SendError;
use personal_rns::units::DurationMillis;
use prns_simulation::{
    FaultPlan, ManualMedium, ManualTaskRunner, ManualTimeDriver, MediumEvent, Reachability,
    SimulationTick, TopologyConfig, TopologyMutation, VirtualMedium, VirtualMediumConfig,
};

use super::scenario::{
    add_node, announce, destination, nonzero, request, settle, Completion, NodeRole, NodeSpec,
    QUERY_PATH,
};

const CLIENT_COUNT: usize = 16;
const SERVER_COUNT: usize = 2;
const LEFT_TRANSPORT: usize = CLIENT_COUNT + SERVER_COUNT;
const RIGHT_TRANSPORT: usize = LEFT_TRANSPORT + 1;
const NODE_COUNT: usize = RIGHT_TRANSPORT + 1;
const SEGMENT_COUNT: usize = CLIENT_COUNT + SERVER_COUNT + 1;
const DISCOVERY_DEADLINE_MILLIS: u64 = 10_000;
const REQUEST_TIMEOUT_MILLIS: u64 = 50;
const RECEIVE_QUEUE_CAPACITY: usize = 64;
const TRACE_CAPACITY: usize = 65_536;

#[test]
fn routed_requests_cross_two_transports_and_recover_after_bridge_loss() {
    let config = VirtualMediumConfig::new(
        TopologyConfig::Explicit {
            max_neighbors: nonzero(1),
        },
        SEGMENT_COUNT * 2,
        RECEIVE_QUEUE_CAPACITY,
        SEGMENT_COUNT * 2,
        TRACE_CAPACITY,
        FaultPlan::none(),
    )
    .unwrap_or_else(|error| unreachable!("isolated paired segments: {error}"));
    let medium = VirtualMedium::new(config);
    let mut wiring: Vec<Vec<_>> = (0..NODE_COUNT).map(|_| Vec::new()).collect();
    let mut endpoints = Vec::new();
    for (segment, (first, second)) in (0..CLIENT_COUNT)
        .map(|leaf| (leaf, LEFT_TRANSPORT))
        .chain([(LEFT_TRANSPORT, RIGHT_TRANSPORT)])
        .chain((CLIENT_COUNT..LEFT_TRANSPORT).map(|leaf| (RIGHT_TRANSPORT, leaf)))
        .enumerate()
    {
        let first_interface = medium
            .attach(&(segment as u64 * 2).to_be_bytes())
            .unwrap_or_else(|error| unreachable!("unique endpoint: {error}"));
        let second_interface = medium
            .attach(&(segment as u64 * 2 + 1).to_be_bytes())
            .unwrap_or_else(|error| unreachable!("unique endpoint: {error}"));
        let pair = (
            first_interface.endpoint_id(),
            second_interface.endpoint_id(),
        );
        assert_eq!(
            medium.set_reachability(pair.0, pair.1, Reachability::Reachable),
            Ok(TopologyMutation::Applied)
        );
        endpoints.push(pair);
        wiring[first].push(first_interface);
        wiring[second].push(second_interface);
    }
    let ingress: Vec<_> = wiring[..CLIENT_COUNT]
        .iter()
        .map(|interfaces| interfaces[0].descriptor().id)
        .collect();
    let destinations: Vec<_> = (CLIENT_COUNT..LEFT_TRANSPORT)
        .map(|index| {
            destination(index)
                .destination_hash()
                .unwrap_or_else(|error| unreachable!("valid server: {error:?}"))
        })
        .collect();
    let heard: Vec<_> = (0..NODE_COUNT)
        .map(|_| Rc::new(RefCell::new(Vec::new())))
        .collect();
    let mut driver = ManualTimeDriver::new(
        ManualMedium::Frames(medium.clone()),
        Duration::from_millis(1),
    )
    .unwrap_or_else(|error| unreachable!("manual routed clock: {error}"));
    let mut runner = ManualTaskRunner::new(&mut driver, nonzero(NODE_COUNT + CLIENT_COUNT * 2));
    let (node_tasks, pending_controls): (Vec<_>, Vec<_>) = wiring
        .into_iter()
        .enumerate()
        .map(|(index, interfaces)| {
            let role = if index == LEFT_TRANSPORT || index == RIGHT_TRANSPORT {
                NodeRole::Transport
            } else {
                NodeRole::Endpoint
            };
            add_node(
                &mut runner,
                NodeSpec {
                    index,
                    role,
                    attach_interfaces: move |handle: &personal_rns::runtime::PrnsNodeHandle| {
                        for interface in interfaces {
                            let _attached = handle.add_interface(interface);
                        }
                    },
                    heard: heard[index].clone(),
                    heard_capacity: nonzero(SERVER_COUNT),
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
                .unwrap_or_else(|error| unreachable!("node boot completed: {error}"))
        })
        .collect();
    for (server, &destination) in controls[CLIENT_COUNT..LEFT_TRANSPORT]
        .iter()
        .zip(&destinations)
    {
        announce(server, destination);
    }
    let mut now = 0;
    loop {
        assert!(settle(&mut runner).is_empty());
        if heard[..CLIENT_COUNT]
            .iter()
            .all(|peers| peers.borrow().len() == SERVER_COUNT)
        {
            break;
        }
        assert!(
            now < DISCOVERY_DEADLINE_MILLIS,
            "routed announcements must reach every client; heard counts: {:?}",
            heard
                .iter()
                .map(|peers| peers.borrow().len())
                .collect::<Vec<_>>()
        );
        now += 1;
        assert!(runner
            .advance_to_next_event(SimulationTick::from_ticks(now))
            .is_ok());
    }
    let mut expected_destinations = destinations.clone();
    expected_destinations.sort_by_key(|destination| *destination.as_bytes());
    for peers in &heard[..CLIENT_COUNT] {
        assert_eq!(*peers.borrow(), expected_destinations);
    }
    let expected_routes: BTreeMap<_, _> = controls[..CLIENT_COUNT]
        .iter()
        .enumerate()
        .map(|(index, client)| {
            let handle = client.handle.clone();
            let remote = destinations[index % SERVER_COUNT];
            let task = runner
                .insert(async move {
                    let route = handle
                        .route(remote)
                        .await
                        .unwrap_or_else(|| unreachable!("client route exists"));
                    Completion::Route {
                        node: index,
                        hops: route.hops,
                        interface: route.interface,
                    }
                })
                .unwrap_or_else(|error| unreachable!("route inventory admission: {error}"));
            (
                task,
                Completion::Route {
                    node: index,
                    hops: 3,
                    interface: ingress[index],
                },
            )
        })
        .collect();
    assert_eq!(
        settle(&mut runner).into_iter().collect::<BTreeMap<_, _>>(),
        expected_routes
    );
    for (index, client) in controls[..CLIENT_COUNT].iter().enumerate() {
        let handle = client.handle.clone();
        let remote = destinations[index % SERVER_COUNT];
        assert!(runner
            .insert(async move {
                let link = handle
                    .establish_link(remote)
                    .await
                    .unwrap_or_else(|error| unreachable!("three-hop link: {error:?}"));
                Completion::Linked { node: index, link }
            })
            .is_ok());
    }
    let mut links = BTreeMap::new();
    for (_, output) in settle(&mut runner) {
        let Completion::Linked { node, link } = output else {
            unreachable!("only routed links may complete")
        };
        assert!(links.insert(node, link).is_none());
    }
    assert_eq!(links.len(), CLIENT_COUNT);
    let responses: BTreeMap<_, _> = controls[..CLIENT_COUNT]
        .iter()
        .enumerate()
        .map(|(index, client)| {
            let payload = (index as u64).to_be_bytes().to_vec();
            let task = request(
                &mut runner,
                index,
                client.handle.clone(),
                links[&index],
                payload.clone(),
            );
            (
                task,
                Completion::Response {
                    node: index,
                    bytes: payload,
                },
            )
        })
        .collect();
    assert_eq!(
        settle(&mut runner).into_iter().collect::<BTreeMap<_, _>>(),
        responses
    );

    let bridge = endpoints[CLIENT_COUNT];
    assert_eq!(
        medium.set_reachability(bridge.0, bridge.1, Reachability::Isolated),
        Ok(TopologyMutation::Applied)
    );
    let timeouts: BTreeMap<_, _> = controls[..CLIENT_COUNT]
        .iter()
        .enumerate()
        .map(|(index, client)| {
            let handle = client.handle.clone();
            let link = links[&index];
            let task = runner
                .insert(async move {
                    assert_eq!(
                        handle
                            .request_with_response_timeout(
                                link,
                                RequestPathHash::of(QUERY_PATH),
                                b"cut",
                                RequestResponseTimeout::Exact(DurationMillis(
                                    REQUEST_TIMEOUT_MILLIS
                                ))
                            )
                            .await,
                        Err(SendError::Failed(SendRequestFailure::Timeout))
                    );
                    Completion::TimedOut { node: index }
                })
                .unwrap_or_else(|error| unreachable!("bounded timeout admission: {error}"));
            (task, Completion::TimedOut { node: index })
        })
        .collect();
    let local_responses: BTreeMap<_, _> = controls[CLIENT_COUNT..LEFT_TRANSPORT]
        .iter()
        .enumerate()
        .map(|(index, server)| {
            let handle = server.handle.clone();
            let remote = destinations[(index + 1) % SERVER_COUNT];
            let task = runner
                .insert(async move {
                    let link = handle
                        .establish_link(remote)
                        .await
                        .unwrap_or_else(|error| unreachable!("same-side link: {error:?}"));
                    let (bytes, _) = handle
                        .request(link, RequestPathHash::of(QUERY_PATH), b"same-side")
                        .await
                        .unwrap_or_else(|error| unreachable!("same-side echo: {error:?}"));
                    Completion::Response {
                        node: CLIENT_COUNT + index,
                        bytes,
                    }
                })
                .unwrap_or_else(|error| unreachable!("bounded same-side admission: {error}"));
            (
                task,
                Completion::Response {
                    node: CLIENT_COUNT + index,
                    bytes: b"same-side".to_vec(),
                },
            )
        })
        .collect();
    assert_eq!(
        settle(&mut runner).into_iter().collect::<BTreeMap<_, _>>(),
        local_responses
    );
    for _ in 1..REQUEST_TIMEOUT_MILLIS {
        now += 1;
        assert!(runner
            .advance_to_next_event(SimulationTick::from_ticks(now))
            .is_ok());
        assert!(
            settle(&mut runner).is_empty(),
            "routed requests cannot expire early"
        );
    }
    now += 1;
    assert!(runner
        .advance_to_next_event(SimulationTick::from_ticks(now))
        .is_ok());
    assert_eq!(
        settle(&mut runner).into_iter().collect::<BTreeMap<_, _>>(),
        timeouts
    );
    assert_eq!(
        medium.set_reachability(bridge.0, bridge.1, Reachability::Reachable),
        Ok(TopologyMutation::Applied)
    );
    let recovered: BTreeMap<_, _> = controls[..CLIENT_COUNT]
        .iter()
        .enumerate()
        .map(|(index, client)| {
            let task = request(
                &mut runner,
                index,
                client.handle.clone(),
                links[&index],
                b"recovered".to_vec(),
            );
            (
                task,
                Completion::Response {
                    node: index,
                    bytes: b"recovered".to_vec(),
                },
            )
        })
        .collect();
    assert_eq!(
        settle(&mut runner).into_iter().collect::<BTreeMap<_, _>>(),
        recovered
    );
    for control in controls {
        assert_eq!(control.shutdown.send(()), Ok(()));
    }
    assert_eq!(
        settle(&mut runner).into_iter().collect::<BTreeMap<_, _>>(),
        node_tasks
            .into_iter()
            .enumerate()
            .map(|(node, task)| {
                (
                    task,
                    Completion::Stopped {
                        node,
                        result: Ok(()),
                    },
                )
            })
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
    assert_eq!(
        detached,
        endpoints
            .into_iter()
            .flat_map(|(first, second)| [first, second])
            .collect::<Vec<_>>()
    );
}
