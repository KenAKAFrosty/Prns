use super::*;
use crate::ble::{advance_until, member_inventory};
use crate::scenario::NodeControl;
use personal_rns::interfaces::bluetooth_auto::{
    AppleHost, BleAddress, BleIdentity, BleRoleCapabilities, BlueZHost, Endpoint, LinkCapabilities,
    BLE_HW_MTU, CONTROL_MAX_LEN,
};
use personal_rns::interfaces::{ConnectionState, InterfaceId, InterfaceKind};
use personal_rns::manifold::interface_seam::Interface;
use personal_rns::node_introspection::NodeIntrospection;
use personal_rns::routing::links::LinkId;
use personal_rns::runtime::PrnsNodeHandle;
use prns_interfaces_tokio::bluetooth_auto::BluetoothAuto;
use prns_simulation::ble::{
    BleMediumConfig, BleSimulationEvent, VirtualBleBackend, VirtualBleBackendConfig,
    VirtualBleBackendLimits, VirtualBleLab, VirtualBleLinkConfig, VirtualGattConfig,
};
use prns_simulation::{ManualTaskId, VirtualInterface};

mod fixture;
mod recovery;
use fixture::{boot, supervisor, Ports};

const BRIDGE: usize = 1;
const BLE_PEER: usize = 2;
const REQUEST_TIMEOUT: u64 = 50;

fn hash(node: usize) -> personal_rns::wire::DestinationHash {
    destination(node)
        .destination_hash()
        .unwrap_or_else(|error| unreachable!("destination: {error:?}"))
}

fn link(
    runner: &mut ManualTaskRunner<'_, Completion>,
    nodes: &[fixture::Node],
    from: usize,
    to: usize,
) -> LinkId {
    let handle = nodes[from].control.handle.clone();
    let task = runner
        .insert(async move {
            let link = handle
                .establish_link(hash(to))
                .await
                .unwrap_or_else(|error| unreachable!("mixed link: {error:?}"));
            Completion::Linked { node: from, link }
        })
        .unwrap_or_else(|error| unreachable!("link actor: {error}"));
    let completed = settle(runner);
    let [(observed, Completion::Linked { node, link })] = completed.as_slice() else {
        unreachable!("one completed link: {completed:?}")
    };
    assert_eq!((*observed, *node), (task, from));
    *link
}

fn echo(
    runner: &mut ManualTaskRunner<'_, Completion>,
    nodes: &[fixture::Node],
    from: usize,
    link: LinkId,
    marker: u8,
) {
    let bytes = vec![marker; 256];
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

#[test]
fn a_real_transport_bridges_frames_and_ble_on_one_clock_through_frame_partition() {
    let frames = VirtualMedium::new(
        VirtualMediumConfig::new(
            TopologyConfig::Explicit {
                max_neighbors: nonzero(1),
            },
            2,
            8,
            8,
            8192,
            FaultPlan::none(),
        )
        .unwrap_or_else(|error| unreachable!("frame medium: {error}")),
    );
    let client = frames
        .attach(b"mixed-client")
        .unwrap_or_else(|error| unreachable!("client: {error}"));
    let bridge = frames
        .attach(b"mixed-bridge")
        .unwrap_or_else(|error| unreachable!("bridge: {error}"));
    let endpoints = [client.endpoint_id(), bridge.endpoint_id()];
    let client_interface = client.descriptor().id;
    assert_eq!(
        frames.set_reachability(endpoints[0], endpoints[1], Reachability::Reachable),
        Ok(TopologyMutation::Applied)
    );
    let ble = VirtualBleLab::new(
        BleMediumConfig::new(TopologyConfig::FullyConnected, 2, 8, 2, 32768)
            .unwrap_or_else(|error| unreachable!("BLE lab: {error}")),
    );
    let bridge_ble = supervisor(&ble, BRIDGE, Endpoint::CoreBluetooth(AppleHost::MacOs));
    let peer_ble = supervisor(&ble, BLE_PEER, Endpoint::BlueZ(BlueZHost::Linux));
    let mut driver = ManualTimeDriver::new(
        ManualMedium::FramesAndBle {
            frames: frames.clone(),
            ble: ble.clone(),
        },
        Duration::from_millis(1),
    )
    .unwrap_or_else(|error| unreachable!("mixed driver: {error}"));
    let mut runner = ManualTaskRunner::new(&mut driver, nonzero(8));
    let nodes = [
        boot(&mut runner, 0, Ports::Frames(client)),
        boot(
            &mut runner,
            BRIDGE,
            Ports::Bridge {
                frames: bridge,
                ble: bridge_ble,
            },
        ),
        boot(&mut runner, BLE_PEER, Ports::Ble(peer_ble)),
    ];
    advance_until(&mut runner, || {
        ble.active_connection_count() == 1
            && [BRIDGE, BLE_PEER].into_iter().all(|node| {
                member_inventory(&nodes[node].control.handle)
                    == vec![(
                        InterfaceId::from_channel_tag(
                            InterfaceKind::BluetoothPeer,
                            &[(3 - node) as u8; 16],
                        ),
                        ConnectionState::Connected,
                    )]
            })
    });
    announce(&nodes[0].control, hash(0));
    announce(&nodes[BLE_PEER].control, hash(BLE_PEER));
    advance_until(&mut runner, || {
        *nodes[0].heard.borrow() == [hash(BLE_PEER)] && *nodes[BLE_PEER].heard.borrow() == [hash(0)]
    });
    let mut expected = BTreeMap::new();
    for (node, destination, interface) in [
        (0, BLE_PEER, client_interface),
        (
            BLE_PEER,
            0,
            InterfaceId::from_channel_tag(InterfaceKind::BluetoothPeer, &[BRIDGE as u8; 16]),
        ),
    ] {
        let handle = nodes[node].control.handle.clone();
        let task = runner
            .insert(async move {
                let route = handle
                    .route(hash(destination))
                    .await
                    .unwrap_or_else(|| unreachable!("bridged route"));
                Completion::Route {
                    node,
                    hops: route.hops,
                    interface: route.interface,
                }
            })
            .unwrap_or_else(|error| unreachable!("route actor: {error}"));
        expected.insert(
            task,
            Completion::Route {
                node,
                hops: 2,
                interface,
            },
        );
    }
    assert_eq!(
        settle(&mut runner).into_iter().collect::<BTreeMap<_, _>>(),
        expected
    );
    let crossing = [
        (0, link(&mut runner, &nodes, 0, BLE_PEER)),
        (BLE_PEER, link(&mut runner, &nodes, BLE_PEER, 0)),
    ];
    let local = link(&mut runner, &nodes, BRIDGE, BLE_PEER);
    for &(node, link) in &crossing {
        echo(&mut runner, &nodes, node, link, 0x51);
    }
    assert_eq!(
        frames.set_reachability(endpoints[0], endpoints[1], Reachability::Isolated),
        Ok(TopologyMutation::Applied)
    );
    let started = runner
        .snapshot()
        .unwrap_or_else(|error| unreachable!("clock: {error}"))
        .tick
        .get();
    let pending: BTreeMap<_, _> = crossing
        .iter()
        .map(|&(node, link)| {
            let handle = nodes[node].control.handle.clone();
            let task = runner
                .insert(async move {
                    assert_eq!(
                        handle
                            .request_with_response_timeout(
                                link,
                                RequestPathHash::of(QUERY_PATH),
                                b"partitioned",
                                RequestResponseTimeout::Exact(DurationMillis(REQUEST_TIMEOUT))
                            )
                            .await,
                        Err(SendError::Failed(SendRequestFailure::Timeout))
                    );
                    Completion::TimedOut { node }
                })
                .unwrap_or_else(|error| unreachable!("request actor: {error}"));
            (task, Completion::TimedOut { node })
        })
        .collect();
    echo(&mut runner, &nodes, BRIDGE, local, 0x62);
    let deadline = started + REQUEST_TIMEOUT;
    let mut finished = false;
    for _ in 0..REQUEST_TIMEOUT * 3 {
        let now = runner
            .snapshot()
            .unwrap_or_else(|error| unreachable!("clock: {error}"))
            .tick
            .get();
        assert!(runner
            .advance_to_next_event(tick((now + 1).min(deadline)))
            .is_ok());
        assert_eq!(frames.now(), ble.now());
        let completed = settle(&mut runner);
        if frames.now() == tick(deadline) {
            assert_eq!(completed.into_iter().collect::<BTreeMap<_, _>>(), pending);
            finished = true;
            break;
        }
        assert!(completed.is_empty());
    }
    assert!(
        finished,
        "same-tick emissions must not stall timeout progress"
    );
    assert_eq!(ble.active_connection_count(), 1);
    assert_eq!(
        frames.set_reachability(endpoints[0], endpoints[1], Reachability::Reachable),
        Ok(TopologyMutation::Applied)
    );
    for &(node, link) in &crossing {
        echo(&mut runner, &nodes, node, link, 0x73);
    }
    echo(&mut runner, &nodes, BRIDGE, local, 0x84);
    fixture::shutdown(&mut runner, nodes, &frames, &ble, 2);
}
