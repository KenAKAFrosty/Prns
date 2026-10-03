use super::*;
use personal_rns::interfaces::bluetooth_auto::{
    BleIdentity, BleRoleCapabilities, LinkCapabilities, BLE_HW_MTU, CONTROL_MAX_LEN,
};
use personal_rns::wire::DestinationHash;
use prns_interfaces_tokio::bluetooth_auto::{BluetoothAuto, BluetoothAutoStatus};
use prns_simulation::ble::{
    BleMediumConfig, BleRadioId, VirtualBleBackendConfig, VirtualBleBackendLimits,
    VirtualBleLinkConfig, VirtualGattConfig,
};
use prns_simulation::{
    ManualMedium, ManualTaskId, ManualTimeDriver, SimulationDurationInTicks, TopologyConfig,
};

pub(super) const NODE_COUNT: usize = 4;
const MAX_PEERS: usize = 1;
const ADVERTISEMENT_INTERVAL_MS: u64 = 20;
const TRACE_CAPACITY: usize = 262_144;
const GATT_VALUE_BYTES: usize = 20;

pub(super) struct LiveNode {
    pub task: ManualTaskId,
    pub radio: BleRadioId,
    pub control: NodeControl,
    pub status: BluetoothAutoStatus,
    pub heard: Rc<RefCell<Vec<DestinationHash>>>,
}

pub(super) fn address(index: usize) -> BleAddress {
    BleAddress::new([index as u8; 6])
}

pub(super) fn profile(index: usize) -> Endpoint {
    if index.is_multiple_of(2) {
        Endpoint::CoreBluetooth(AppleHost::MacOs)
    } else {
        Endpoint::BlueZ(BlueZHost::Linux)
    }
}

pub(super) fn expected_member(peer: usize) -> Vec<(InterfaceId, ConnectionState)> {
    vec![(
        InterfaceId::from_channel_tag(InterfaceKind::BluetoothPeer, &[peer as u8; 16]),
        ConnectionState::Connected,
    )]
}

pub(super) fn destination_hash(index: usize) -> DestinationHash {
    destination(index)
        .destination_hash()
        .unwrap_or_else(|error| unreachable!("destination: {error:?}"))
}

pub(super) fn start(
    runner: &mut ManualTaskRunner<'_, Completion>,
    lab: &VirtualBleLab,
    index: usize,
) -> LiveNode {
    let gatt = VirtualGattConfig::new(CONTROL_MAX_LEN, GATT_VALUE_BYTES)
        .unwrap_or_else(|error| unreachable!("fragmented GATT: {error}"));
    let link = VirtualBleLinkConfig::new(2, 2, BLE_HW_MTU, gatt)
        .unwrap_or_else(|error| unreachable!("bounded link: {error}"));
    let backend = lab
        .attach_backend(
            VirtualBleBackendConfig::new(
                address(index),
                -40,
                BleRoleCapabilities::DualRole,
                SimulationDurationInTicks::from_ticks(ADVERTISEMENT_INTERVAL_MS),
                VirtualBleBackendLimits {
                    inbound_links: nonzero(1),
                    connections: nonzero(1),
                    discovered_peers: nonzero(1),
                },
                link,
            )
            .unwrap_or_else(|error| unreachable!("backend config: {error}")),
        )
        .unwrap_or_else(|error| unreachable!("fresh radio: {error}"));
    let Some(BleSimulationEvent::RadioAttached { radio }) = lab.trace().events.last().cloned()
    else {
        unreachable!("attachment must identify its new incarnation")
    };
    let supervisor = BluetoothAuto::<_, MAX_PEERS>::new(
        backend,
        BleIdentity::new([index as u8; 16]),
        profile(index),
        LinkCapabilities {
            l2cap: None,
            link_mtu: BLE_HW_MTU as u16,
        },
    );
    let status = supervisor.status();
    let heard = Rc::new(RefCell::new(Vec::new()));
    let (task, mut ready) = add_node(
        runner,
        NodeSpec {
            index,
            role: NodeRole::Endpoint,
            attach_interfaces: move |handle: &PrnsNodeHandle| {
                let _attached = handle.supervise(supervisor);
            },
            heard: heard.clone(),
            heard_capacity: nonzero(1),
        },
    );
    assert!(settle(runner).is_empty());
    let control = ready
        .try_recv()
        .unwrap_or_else(|error| unreachable!("node ready: {error}"));
    LiveNode {
        task,
        radio,
        control,
        status,
        heard,
    }
}

pub(super) fn connect(lab: &VirtualBleLab, first: usize, second: usize) {
    assert_eq!(
        lab.set_reachability(address(first), address(second), Reachability::Reachable),
        Ok(TopologyMutation::Applied)
    );
}

pub(super) fn converge(
    runner: &mut ManualTaskRunner<'_, Completion>,
    lab: &VirtualBleLab,
    nodes: &[LiveNode],
) {
    advance_until(runner, || {
        lab.active_connection_count() == NODE_COUNT / 2
            && nodes.iter().enumerate().all(|(index, live)| {
                member_inventory(&live.control.handle) == expected_member(index ^ 1)
            })
    });
    for (index, live) in nodes.iter().enumerate() {
        announce(&live.control, destination_hash(index));
    }
    advance_until(runner, || {
        nodes
            .iter()
            .enumerate()
            .all(|(index, live)| *live.heard.borrow() == [destination_hash(index ^ 1)])
    });
}

pub(super) fn establish(
    runner: &mut ManualTaskRunner<'_, Completion>,
    nodes: &[LiveNode],
    first: usize,
    second: usize,
) -> LinkId {
    let handle = nodes[first].control.handle.clone();
    let task = runner
        .insert(async move {
            let link = handle
                .establish_link(destination_hash(second))
                .await
                .unwrap_or_else(|error| unreachable!("BLE Reticulum link: {error:?}"));
            Completion::Linked { node: first, link }
        })
        .unwrap_or_else(|error| unreachable!("link actor: {error}"));
    let results = settle(runner);
    let [(observed, Completion::Linked { node, link })] = results.as_slice() else {
        unreachable!("one BLE link result: {results:?}")
    };
    assert_eq!((*observed, *node), (task, first));
    *link
}

pub(super) fn echo(
    runner: &mut ManualTaskRunner<'_, Completion>,
    nodes: &[LiveNode],
    from: usize,
    link: LinkId,
    marker: u8,
) {
    let bytes = vec![marker; PAYLOAD_BYTES];
    let task = request(
        runner,
        from,
        nodes[from].control.handle.clone(),
        link,
        bytes.clone(),
    );
    assert_eq!(
        settle(runner),
        [(task, Completion::Response { node: from, bytes })]
    );
}

pub(super) fn with_fleet(
    run: impl FnOnce(&mut ManualTaskRunner<'_, Completion>, &VirtualBleLab, &mut [LiveNode]),
) {
    let lab = VirtualBleLab::new(
        BleMediumConfig::new(
            TopologyConfig::Explicit {
                max_neighbors: nonzero(1),
            },
            NODE_COUNT,
            4,
            4,
            TRACE_CAPACITY,
        )
        .unwrap_or_else(|error| unreachable!("paired BLE lab: {error}")),
    );
    let mut driver =
        ManualTimeDriver::new(ManualMedium::Ble(lab.clone()), Duration::from_millis(1))
            .unwrap_or_else(|error| unreachable!("BLE clock: {error}"));
    let mut runner = ManualTaskRunner::new(&mut driver, nonzero(NODE_COUNT + 4));
    let mut nodes: Vec<_> = (0..NODE_COUNT)
        .map(|index| start(&mut runner, &lab, index))
        .collect();
    connect(&lab, 0, 1);
    connect(&lab, 2, 3);
    converge(&mut runner, &lab, &nodes);
    run(&mut runner, &lab, &mut nodes);
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
    assert_eq!(lab.active_connection_count(), 0);
    let trace = lab.trace();
    assert_eq!(trace.discarded_events, 0);
    let mut attached = Vec::new();
    let mut detached = Vec::new();
    for event in trace.events {
        match event {
            BleSimulationEvent::RadioAttached { radio } => attached.push(radio),
            BleSimulationEvent::RadioDetached { radio } => detached.push(radio),
            BleSimulationEvent::ObservationDropped { .. } => {
                unreachable!("discovery trace must not drop observations")
            }
            _ => {}
        }
    }
    attached.sort();
    detached.sort();
    assert_eq!(attached.len(), NODE_COUNT + RESTARTS);
    assert_eq!(detached, attached);
}
