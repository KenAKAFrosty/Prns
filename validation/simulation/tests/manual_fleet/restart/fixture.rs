use super::*;
use personal_rns::interfaces::InterfaceId;
use personal_rns::manifold::interface_seam::Interface;
use personal_rns::routing::links::LinkId;
use personal_rns::runtime::PrnsNodeHandle;
use personal_rns::wire::DestinationHash;
use prns_simulation::{EndpointId, ManualTaskId, VirtualInterface};

pub(super) const NODE_COUNT: usize = 4;
const ACTOR_CAPACITY: usize = NODE_COUNT + 4;
const TRACE_CAPACITY: usize = 4096;

pub(super) struct LiveNode {
    pub task: ManualTaskId,
    pub control: NodeControl,
    pub endpoint: EndpointId,
    pub interface: InterfaceId,
    pub heard: Rc<RefCell<Vec<DestinationHash>>>,
}

pub(super) fn destination_hash(index: usize) -> DestinationHash {
    destination(index)
        .destination_hash()
        .unwrap_or_else(|error| unreachable!("destination: {error:?}"))
}

fn start(
    runner: &mut ManualTaskRunner<'_, Completion>,
    medium: &VirtualMedium,
    index: usize,
) -> LiveNode {
    let interface: VirtualInterface = medium
        .attach(&(index as u64).to_be_bytes())
        .unwrap_or_else(|error| unreachable!("unique live interface: {error}"));
    let endpoint = interface.endpoint_id();
    let id = interface.descriptor().id;
    let heard = Rc::new(RefCell::new(Vec::new()));
    let (task, mut ready) = add_node(
        runner,
        NodeSpec {
            index,
            role: NodeRole::Endpoint,
            attach_interfaces: move |handle: &PrnsNodeHandle| {
                let _attached = handle.add_interface(interface);
            },
            heard: heard.clone(),
            heard_capacity: nonzero(1),
        },
    );
    assert!(settle(runner).is_empty());
    let control = ready
        .try_recv()
        .unwrap_or_else(|error| unreachable!("initialized node: {error}"));
    LiveNode {
        task,
        control,
        endpoint,
        interface: id,
        heard,
    }
}

pub(super) fn connect(medium: &VirtualMedium, nodes: &[LiveNode], first: usize, second: usize) {
    assert_eq!(
        medium.set_reachability(
            nodes[first].endpoint,
            nodes[second].endpoint,
            Reachability::Reachable
        ),
        Ok(TopologyMutation::Applied)
    );
}

pub(super) fn restart(
    runner: &mut ManualTaskRunner<'_, Completion>,
    medium: &VirtualMedium,
    nodes: &mut [LiveNode],
    index: usize,
) {
    assert_eq!(
        runner.cancel(nodes[index].task).ok(),
        Some(ManualTaskCancellation::Cancelled)
    );
    replace_cancelled(runner, medium, nodes, index);
}

pub(super) fn replace_cancelled(
    runner: &mut ManualTaskRunner<'_, Completion>,
    medium: &VirtualMedium,
    nodes: &mut [LiveNode],
    index: usize,
) {
    let before = runner.snapshot().ok();
    let old = &nodes[index];
    assert_eq!(runner.task_count(), NODE_COUNT - 1);
    assert_eq!(
        runner.cancel(old.task).ok(),
        Some(ManualTaskCancellation::NotLive)
    );
    let replacement = start(runner, medium, index);
    assert_ne!(replacement.task, old.task);
    assert_ne!(replacement.endpoint, old.endpoint);
    assert_eq!(replacement.interface, old.interface);
    assert!(replacement.heard.borrow().is_empty());
    assert_eq!(runner.snapshot().ok(), before);
    assert_eq!(runner.task_count(), NODE_COUNT);
    nodes[index] = replacement;
}

pub(super) fn link(
    runner: &mut ManualTaskRunner<'_, Completion>,
    nodes: &[LiveNode],
    first: usize,
    second: usize,
) -> LinkId {
    announce(&nodes[second].control, destination_hash(second));
    assert!(settle(runner).is_empty());
    let handle = nodes[first].control.handle.clone();
    let task = runner
        .insert(async move {
            let link = handle
                .establish_link(destination_hash(second))
                .await
                .unwrap_or_else(|error| unreachable!("fresh link: {error:?}"));
            Completion::Linked { node: first, link }
        })
        .unwrap_or_else(|error| unreachable!("link actor: {error}"));
    let completed = settle(runner);
    let [(observed, Completion::Linked { node, link })] = completed.as_slice() else {
        unreachable!("one link completes: {completed:?}")
    };
    assert_eq!((*observed, *node), (task, first));
    *link
}

pub(super) fn echo(
    runner: &mut ManualTaskRunner<'_, Completion>,
    nodes: &[LiveNode],
    node: usize,
    link: LinkId,
    bytes: &[u8],
) {
    let task = request(
        runner,
        node,
        nodes[node].control.handle.clone(),
        link,
        bytes.to_vec(),
    );
    assert_eq!(
        settle(runner),
        [(
            task,
            Completion::Response {
                node,
                bytes: bytes.to_vec()
            }
        )]
    );
}

pub(super) fn with_fleet(
    faults: FaultPlan,
    run: impl FnOnce(&mut ManualTaskRunner<'_, Completion>, &VirtualMedium, &mut [LiveNode]),
) {
    let medium = VirtualMedium::new(
        VirtualMediumConfig::new(
            TopologyConfig::Explicit {
                max_neighbors: nonzero(1),
            },
            NODE_COUNT,
            8,
            8,
            TRACE_CAPACITY,
            faults,
        )
        .unwrap_or_else(|error| unreachable!("paired medium: {error}")),
    );
    let mut driver = ManualTimeDriver::new(
        ManualMedium::Frames(medium.clone()),
        Duration::from_millis(1),
    )
    .unwrap_or_else(|error| unreachable!("fleet clock: {error}"));
    let mut runner = ManualTaskRunner::new(&mut driver, nonzero(ACTOR_CAPACITY));
    let mut nodes: Vec<_> = (0..NODE_COUNT)
        .map(|index| start(&mut runner, &medium, index))
        .collect();
    connect(&medium, &nodes, 0, 1);
    connect(&medium, &nodes, 2, 3);
    run(&mut runner, &medium, &mut nodes);
    assert_eq!(runner.task_count(), NODE_COUNT);
    assert_eq!(medium.pending_delivery_count(), 0);
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
    let trace = medium.trace();
    assert_eq!(trace.discarded_events, 0);
    let mut attached = Vec::new();
    let mut detached = Vec::new();
    for event in trace.events {
        match event {
            MediumEvent::EndpointAttached { endpoint, .. } => attached.push(endpoint),
            MediumEvent::EndpointDetached { endpoint } => detached.push(endpoint),
            _ => {}
        }
    }
    attached.sort();
    detached.sort();
    assert_eq!(attached, detached);
}
