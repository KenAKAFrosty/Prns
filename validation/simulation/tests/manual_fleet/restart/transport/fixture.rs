use super::*;
use prns_simulation::{ManualMedium, ManualTimeDriver, TopologyConfig, VirtualMediumConfig};

const TRACE_CAPACITY: usize = 65_536;
const ACTOR_CAPACITY: usize = NODE_COUNT + LEAVES + 2;

pub(super) struct LiveNode {
    pub task: ManualTaskId,
    pub control: NodeControl,
    pub ports: Vec<topology::BoundPort>,
    pub heard: Rc<RefCell<Vec<DestinationHash>>>,
}

pub(super) fn hash(index: usize) -> DestinationHash {
    destination(index)
        .destination_hash()
        .unwrap_or_else(|error| unreachable!("destination: {error:?}"))
}

fn start(
    runner: &mut ManualTaskRunner<'_, Completion>,
    medium: &VirtualMedium,
    index: usize,
) -> LiveNode {
    let (ports, interfaces) = topology::attach(medium, index);
    let heard = Rc::new(RefCell::new(Vec::new()));
    let (task, mut ready) = add_node(
        runner,
        NodeSpec {
            index,
            role: if index < LEAVES {
                NodeRole::Endpoint
            } else {
                NodeRole::Transport
            },
            attach_interfaces: move |handle: &PrnsNodeHandle| {
                for interface in interfaces {
                    let _attached = handle.add_interface(interface);
                }
            },
            heard: heard.clone(),
            heard_capacity: nonzero(LEAVES),
        },
    );
    assert!(settle(runner).is_empty());
    let control = ready
        .try_recv()
        .unwrap_or_else(|error| unreachable!("transport scenario boot: {error}"));
    LiveNode {
        task,
        control,
        ports,
        heard,
    }
}

pub(super) fn rebuild(
    runner: &mut ManualTaskRunner<'_, Completion>,
    medium: &VirtualMedium,
    nodes: &mut [LiveNode],
    side: Side,
) {
    let index = side.transport();
    let old = &nodes[index];
    let before = runner.snapshot().ok();
    let fresh = start(runner, medium, index);
    assert_eq!(
        runner.cancel(old.task).ok(),
        Some(ManualTaskCancellation::NotLive)
    );
    assert_ne!(fresh.task, old.task);
    assert_eq!(fresh.ports.len(), 3);
    let before_identity: Vec<_> = old
        .ports
        .iter()
        .map(|port| (port.port, port.interface))
        .collect();
    let after_identity: Vec<_> = fresh
        .ports
        .iter()
        .map(|port| (port.port, port.interface))
        .collect();
    assert_eq!(after_identity, before_identity);
    for (old, fresh) in old.ports.iter().zip(&fresh.ports) {
        assert_ne!(old.endpoint, fresh.endpoint);
    }
    assert!(fresh.heard.borrow().is_empty());
    assert_eq!(runner.snapshot().ok(), before);
    nodes[index] = fresh;
    assert_eq!(runner.task_count(), NODE_COUNT);
}

pub(super) fn assert_empty_routes(
    runner: &mut ManualTaskRunner<'_, Completion>,
    node: &LiveNode,
    index: usize,
) {
    let handle = node.control.handle.clone();
    let task = runner
        .insert(async move {
            for destination in (0..LEAVES).map(hash) {
                assert_eq!(
                    handle.route(destination).await,
                    None,
                    "new transport must not inherit volatile paths"
                );
            }
            Completion::RoutesChecked { node: index }
        })
        .unwrap_or_else(|error| unreachable!("route observation: {error}"));
    assert_eq!(
        settle(runner),
        [(task, Completion::RoutesChecked { node: index })]
    );
}

pub(super) fn with_fleet(
    run: impl FnOnce(&mut ManualTaskRunner<'_, Completion>, &VirtualMedium, &mut [LiveNode]),
) {
    let medium = VirtualMedium::new(
        VirtualMediumConfig::new(
            TopologyConfig::Explicit {
                max_neighbors: nonzero(1),
            },
            PORT_COUNT,
            32,
            PORT_COUNT,
            TRACE_CAPACITY,
            FaultPlan::none(),
        )
        .unwrap_or_else(|error| unreachable!("bounded transport topology: {error}")),
    );
    let mut driver = ManualTimeDriver::new(
        ManualMedium::Frames(medium.clone()),
        Duration::from_millis(1),
    )
    .unwrap_or_else(|error| unreachable!("transport clock: {error}"));
    let mut runner = ManualTaskRunner::new(&mut driver, nonzero(ACTOR_CAPACITY));
    let mut nodes: Vec<_> = (0..NODE_COUNT)
        .map(|index| start(&mut runner, &medium, index))
        .collect();
    topology::connect_all(&medium, &nodes);
    run(&mut runner, &medium, &mut nodes);
    assert_eq!(runner.task_count(), NODE_COUNT);
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
    assert_eq!(medium.pending_delivery_count(), 0);
    let trace = medium.trace();
    assert_eq!(trace.discarded_events, 0);
    let mut attached = Vec::new();
    let mut detached = Vec::new();
    for event in trace.events {
        match event {
            MediumEvent::EndpointAttached { endpoint, .. } => attached.push(endpoint),
            MediumEvent::EndpointDetached { endpoint } => detached.push(endpoint),
            MediumEvent::ReceptionDropped { .. } => {
                unreachable!("transport scenario must not drop queued deliveries")
            }
            _ => {}
        }
    }
    attached.sort();
    detached.sort();
    assert_eq!(attached.len(), PORT_COUNT + RESTARTS * 3);
    assert_eq!(detached, attached);
}
