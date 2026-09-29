use super::*;

pub(super) fn medium() -> VirtualMedium {
    VirtualMedium::new(
        VirtualMediumConfig::new(
            TopologyConfig::Explicit {
                max_neighbors: nonzero(1),
            },
            NODES,
            8,
            NODES,
            TRACE_CAPACITY,
            FaultPlan::none(),
        )
        .unwrap_or_else(|error| unreachable!("bounded paired fleet: {error}")),
    )
}

pub(super) struct LiveNode {
    pub task: ManualTaskId,
    pub control: NodeControl,
    pub endpoint: EndpointId,
    pub interface: InterfaceId,
    pub heard: Rc<RefCell<Vec<DestinationHash>>>,
    pub born_at: u64,
}

pub(super) fn hash(index: usize) -> DestinationHash {
    destination(index)
        .destination_hash()
        .unwrap_or_else(|error| unreachable!("identity: {error:?}"))
}

pub(super) fn start(
    runner: &mut ManualTaskRunner<'_, Completion>,
    medium: &VirtualMedium,
    index: usize,
) -> LiveNode {
    let born_at = now(runner);
    let interface = medium
        .attach(&(index as u64).to_be_bytes())
        .unwrap_or_else(|error| unreachable!("fleet attachment: {error}"));
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
    assert_eq!(now(runner), born_at);
    LiveNode {
        task,
        control: ready
            .try_recv()
            .unwrap_or_else(|error| unreachable!("boot: {error}")),
        endpoint,
        interface: id,
        heard,
        born_at,
    }
}

pub(super) fn connect(medium: &VirtualMedium, nodes: &[LiveNode], pair: usize) {
    assert_eq!(
        medium.set_reachability(
            nodes[pair * 2].endpoint,
            nodes[pair * 2 + 1].endpoint,
            Reachability::Reachable,
        ),
        Ok(TopologyMutation::Applied)
    );
}
