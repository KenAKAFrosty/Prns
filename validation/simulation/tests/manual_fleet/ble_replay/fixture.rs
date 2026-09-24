use super::*;
use personal_rns::wire::DestinationHash;
use prns_simulation::ManualTaskId;
use scenario::NodeControl;

pub(super) struct LiveNode {
    pub task: ManualTaskId,
    pub control: NodeControl,
    pub heard: Rc<RefCell<Vec<DestinationHash>>>,
}

pub(super) fn start_node(
    runner: &mut ManualTaskRunner<'_, Completion>,
    lab: &VirtualBleLab,
    index: usize,
    seed: u8,
    first: selection::FirstEvent,
    driver_first: personal_rns::runtime::InterfaceEventSource,
) -> LiveNode {
    start_node_with_peers::<1>(runner, lab, index, seed, first, driver_first)
}

pub(super) fn start_node_with_peers<const MAX_PEERS: usize>(
    runner: &mut ManualTaskRunner<'_, Completion>,
    lab: &VirtualBleLab,
    index: usize,
    seed: u8,
    first: selection::FirstEvent,
    driver_first: personal_rns::runtime::InterfaceEventSource,
) -> LiveNode {
    let gatt = VirtualGattConfig::new(CONTROL_MAX_LEN, 20)
        .unwrap_or_else(|error| unreachable!("GATT: {error}"));
    let config = VirtualBleBackendConfig::new(
        BleAddress::new([index as u8; 6]),
        -40,
        BleRoleCapabilities::DualRole,
        SimulationDurationInTicks::from_ticks(20),
        VirtualBleBackendLimits {
            inbound_links: nonzero(MAX_PEERS),
            connections: nonzero(MAX_PEERS),
            discovered_peers: nonzero(MAX_PEERS),
        },
        VirtualBleLinkConfig::new(2, 2, BLE_HW_MTU, gatt)
            .unwrap_or_else(|error| unreachable!("link: {error}")),
    )
    .unwrap_or_else(|error| unreachable!("backend: {error}"));
    let backend = lab
        .attach_backend(config)
        .unwrap_or_else(|error| unreachable!("attach: {error}"));
    let supervisor = BluetoothAuto::<_, MAX_PEERS>::new(
        backend,
        BleIdentity::new([index as u8; 16]),
        if index == 0 {
            Endpoint::CoreBluetooth(AppleHost::MacOs)
        } else {
            Endpoint::BlueZ(BlueZHost::Linux)
        },
        LinkCapabilities {
            l2cap: None,
            link_mtu: BLE_HW_MTU as u16,
        },
    )
    .with_event_selector(selection::RoundRobinBleEvents::new(first));
    let heard = Rc::new(RefCell::new(Vec::new()));
    let (task, mut ready) = add_node_with_sources_and_arbitration(
        runner,
        NodeSpec {
            index,
            role: NodeRole::Endpoint,
            attach_interfaces: move |handle: &PrnsNodeHandle| {
                let _attached = handle.supervise(supervisor);
            },
            heard: heard.clone(),
            heard_capacity: nonzero(MAX_PEERS),
        },
        move |origin| {
            (
                TokioHost::with_runtime_entropy(origin, stream(seed + index as u8)),
                TokioHandleEntropy::from_sources(
                    stream(80 + index as u8),
                    |_output: &mut [u8]| -> Result<(), core::convert::Infallible> {
                        unreachable!("this routed BLE scenario must not request path entropy")
                    },
                ),
            )
        },
        personal_rns::runtime::InterfaceArbitration::RoundRobin {
            first: driver_first,
        },
    );
    assert!(settle(runner).is_empty());
    let control = ready
        .try_recv()
        .unwrap_or_else(|error| unreachable!("ready: {error}"));
    LiveNode {
        task,
        control,
        heard,
    }
}
