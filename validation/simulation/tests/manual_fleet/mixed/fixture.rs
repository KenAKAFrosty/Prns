use super::*;

type Supervisor = BluetoothAuto<VirtualBleBackend, 1>;

pub(super) enum Ports {
    Frames(VirtualInterface),
    Bridge {
        frames: VirtualInterface,
        ble: Supervisor,
    },
    Ble(Supervisor),
}

pub(super) struct Node {
    pub task: ManualTaskId,
    pub control: NodeControl,
    pub heard: Rc<RefCell<Vec<personal_rns::wire::DestinationHash>>>,
}

pub(super) fn supervisor(lab: &VirtualBleLab, node: usize, profile: Endpoint) -> Supervisor {
    let backend = lab
        .attach_backend(
            VirtualBleBackendConfig::new(
                BleAddress::new([node as u8; 6]),
                -40,
                BleRoleCapabilities::DualRole,
                SimulationDurationInTicks::from_ticks(20),
                VirtualBleBackendLimits {
                    inbound_links: nonzero(1),
                    connections: nonzero(1),
                    discovered_peers: nonzero(1),
                },
                VirtualBleLinkConfig::new(
                    2,
                    2,
                    BLE_HW_MTU,
                    VirtualGattConfig::new(CONTROL_MAX_LEN, 20)
                        .unwrap_or_else(|error| unreachable!("GATT: {error}")),
                )
                .unwrap_or_else(|error| unreachable!("link: {error}")),
            )
            .unwrap_or_else(|error| unreachable!("backend: {error}")),
        )
        .unwrap_or_else(|error| unreachable!("radio: {error}"));
    BluetoothAuto::new(
        backend,
        BleIdentity::new([node as u8; 16]),
        profile,
        LinkCapabilities {
            l2cap: None,
            link_mtu: BLE_HW_MTU as u16,
        },
    )
}

pub(super) fn boot(
    runner: &mut ManualTaskRunner<'_, Completion>,
    index: usize,
    ports: Ports,
) -> Node {
    let heard = Rc::new(RefCell::new(Vec::new()));
    let role = match &ports {
        Ports::Bridge { .. } => NodeRole::Transport,
        Ports::Frames(_) | Ports::Ble(_) => NodeRole::Endpoint,
    };
    let (task, mut ready) = add_node(
        runner,
        NodeSpec {
            index,
            role,
            heard: heard.clone(),
            heard_capacity: nonzero(2),
            attach_interfaces: move |handle: &PrnsNodeHandle| match ports {
                Ports::Frames(interface) => {
                    let _attached = handle.add_interface(interface);
                }
                Ports::Ble(supervisor) => {
                    let _attached = handle.supervise(supervisor);
                }
                Ports::Bridge { frames, ble } => {
                    let _frame = handle.add_interface(frames);
                    let _ble = handle.supervise(ble);
                }
            },
        },
    );
    assert!(settle(runner).is_empty());
    Node {
        task,
        control: ready
            .try_recv()
            .unwrap_or_else(|error| unreachable!("boot: {error}")),
        heard,
    }
}

pub(super) fn shutdown(
    runner: &mut ManualTaskRunner<'_, Completion>,
    nodes: [Node; 3],
    frames: &VirtualMedium,
    ble: &VirtualBleLab,
    expected_attachments_per_medium: usize,
) {
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
        settle(runner).into_iter().collect::<BTreeMap<_, _>>(),
        expected
    );
    assert_eq!(runner.task_count(), 0);
    assert_eq!(frames.pending_delivery_count(), 0);
    assert_eq!(ble.active_connection_count(), 0);
    let trace = frames.trace();
    assert_eq!(trace.discarded_events, 0);
    let mut attached = Vec::new();
    let mut detached = Vec::new();
    for event in trace.events {
        match event {
            MediumEvent::EndpointAttached { endpoint, .. } => attached.push(endpoint),
            MediumEvent::EndpointDetached { endpoint } => detached.push(endpoint),
            MediumEvent::ReceptionDropped { .. } => unreachable!("no frame queue loss"),
            _ => {}
        }
    }
    attached.sort();
    detached.sort();
    assert_eq!(attached.len(), expected_attachments_per_medium);
    assert_eq!(attached, detached);
    let trace = ble.trace();
    assert_eq!(trace.discarded_events, 0);
    let mut attached = Vec::new();
    let mut detached = Vec::new();
    for event in trace.events {
        match event {
            BleSimulationEvent::RadioAttached { radio } => attached.push(radio),
            BleSimulationEvent::RadioDetached { radio } => detached.push(radio),
            BleSimulationEvent::ObservationDropped { .. } => unreachable!("no BLE discovery loss"),
            _ => {}
        }
    }
    attached.sort();
    detached.sort();
    assert_eq!(attached.len(), expected_attachments_per_medium);
    assert_eq!(attached, detached);
}
