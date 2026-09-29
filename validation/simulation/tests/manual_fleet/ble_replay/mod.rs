use super::*;
use personal_rns::interfaces::bluetooth_auto::{
    AppleHost, BleAddress, BleIdentity, BleRoleCapabilities, BlueZHost, Endpoint, LinkCapabilities,
    BLE_HW_MTU, CONTROL_MAX_LEN,
};
use personal_rns::interfaces::{ConnectionState, InterfaceId, InterfaceKind};
use personal_rns::manifold::tokio::TokioHost;
use personal_rns::runtime::{PrnsNodeHandle, TokioHandleEntropy};
use prns_core::entropy::{EntropySource, RuntimeEntropy};
use prns_interfaces_tokio::bluetooth_auto::BluetoothAuto;
use prns_simulation::ble::{
    BleMediumConfig, BleSimulationEvent, BleTraceSnapshot, BleWireCapture, BleWireChannel,
    BleWireSnapshot, VirtualBleBackendConfig, VirtualBleBackendLimits, VirtualBleLab,
    VirtualBleLinkConfig, VirtualGattConfig,
};
use scenario::add_node_with_sources_and_arbitration;

mod fixture;
mod fleet;
mod recovery;
#[path = "../../support/ble_selection.rs"]
mod selection;
mod star;
mod traffic;

#[derive(Clone, Copy)]
enum Lifecycle {
    Fresh,
    Reconnect,
}

// Validation-only inputs. There is no production fixed-seed mode.
fn stream(
    seed: u8,
) -> RuntimeEntropy<impl EntropySource<Error = core::convert::Infallible> + Send> {
    let mut reads = 0;
    RuntimeEntropy::try_new(move |output: &mut [u8]| {
        reads += 1;
        assert_eq!(reads, 1, "short BLE replay must not reseed");
        assert_eq!(output.len(), 32);
        output.fill(seed);
        Ok::<(), core::convert::Infallible>(())
    })
    .unwrap_or_else(|never| match never {})
}

#[derive(Debug, PartialEq, Eq)]
struct Transcript {
    discovery: BleTraceSnapshot,
    wire: BleWireSnapshot,
    response: Vec<u8>,
    connection_boundaries: Vec<usize>,
}

impl Transcript {
    fn assert_replays(&self, expected: &Self) {
        for (index, (actual, expected)) in self
            .wire
            .values
            .iter()
            .zip(&expected.wire.values)
            .enumerate()
        {
            assert_eq!(actual, expected, "wire value {index}");
        }
        for (index, (actual, expected)) in self
            .discovery
            .events
            .iter()
            .zip(&expected.discovery.events)
            .enumerate()
        {
            assert_eq!(actual, expected, "discovery event {index}");
        }
        assert_eq!(self, expected);
    }
}

fn replay(seed: u8, marker: u8, lifecycle: Lifecycle) -> Transcript {
    replay_with_selection(
        seed,
        marker,
        lifecycle,
        selection::FirstEvent::Backend,
        personal_rns::runtime::InterfaceEventSource::Message,
    )
}

fn replay_with_selection(
    seed: u8,
    marker: u8,
    lifecycle: Lifecycle,
    first: selection::FirstEvent,
    driver_first: personal_rns::runtime::InterfaceEventSource,
) -> Transcript {
    let capture = BleWireCapture::new(nonzero(4096));
    let lab = VirtualBleLab::with_wire_capture(
        BleMediumConfig::new(
            TopologyConfig::Explicit {
                max_neighbors: nonzero(1),
            },
            2,
            4,
            4,
            262_144,
        )
        .unwrap_or_else(|error| unreachable!("lab: {error}")),
        capture.clone(),
    );
    let mut driver =
        ManualTimeDriver::new(ManualMedium::Ble(lab.clone()), Duration::from_millis(1))
            .unwrap_or_else(|error| unreachable!("clock: {error}"));
    let mut runner = ManualTaskRunner::new(&mut driver, nonzero(4));
    let mut nodes = Vec::new();
    let destinations: Vec<_> = (0..2)
        .map(|index| {
            destination(index)
                .destination_hash()
                .unwrap_or_else(|error| unreachable!("destination: {error:?}"))
        })
        .collect();
    for index in 0..2 {
        nodes.push(fixture::start_node(
            &mut runner,
            &lab,
            index,
            seed,
            first,
            driver_first,
        ));
    }
    assert_eq!(
        lab.set_reachability(
            BleAddress::new([0; 6]),
            BleAddress::new([1; 6]),
            Reachability::Reachable
        ),
        Ok(TopologyMutation::Applied)
    );
    ble::advance_until(&mut runner, || {
        lab.active_connection_count() == 1
            && nodes
                .iter()
                .enumerate()
                .all(|(index, fixture::LiveNode { control, .. })| {
                    ble::member_inventory(&control.handle)
                        == [(
                            InterfaceId::from_channel_tag(
                                InterfaceKind::BluetoothPeer,
                                &[(index ^ 1) as u8; 16],
                            ),
                            ConnectionState::Connected,
                        )]
                })
    });
    for (index, fixture::LiveNode { control, .. }) in nodes.iter().enumerate() {
        announce(control, destinations[index]);
    }
    ble::advance_until(&mut runner, || {
        nodes
            .iter()
            .enumerate()
            .all(|(index, fixture::LiveNode { heard, .. })| {
                *heard.borrow() == [destinations[index ^ 1]]
            })
    });
    let response = vec![marker; 256];
    exchange(
        &mut runner,
        nodes[0].control.handle.clone(),
        destinations[1],
        &response,
    );
    let mut connection_boundaries = Vec::new();
    match lifecycle {
        Lifecycle::Fresh => {}
        Lifecycle::Reconnect => {
            let boundary = capture.snapshot().values.len();
            connection_boundaries.push(boundary);
            recovery::reconnect(
                &mut runner,
                &lab,
                &capture,
                &nodes
                    .iter()
                    .map(|fixture::LiveNode { control, .. }| control.handle.clone())
                    .collect::<Vec<_>>(),
            );
            exchange(
                &mut runner,
                nodes[0].control.handle.clone(),
                destinations[1],
                &response,
            );
            recovery::assert_distinct_incarnations(&capture.snapshot(), boundary);
        }
    }
    let expected: BTreeMap<_, _> = nodes
        .into_iter()
        .enumerate()
        .map(|(node, fixture::LiveNode { task, control, .. })| {
            assert_eq!(control.shutdown.send(()), Ok(()));
            (
                task,
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
    let discovery = lab.trace();
    assert_eq!(discovery.discarded_events, 0);
    let mut attached = Vec::new();
    let mut detached = Vec::new();
    for event in &discovery.events {
        match event {
            BleSimulationEvent::RadioAttached { radio } => attached.push(*radio),
            BleSimulationEvent::RadioDetached { radio } => detached.push(*radio),
            BleSimulationEvent::ObservationDropped { .. } => unreachable!("discovery drop"),
            _ => {}
        }
    }
    attached.sort();
    detached.sort();
    assert_eq!(attached.len(), 2);
    assert_eq!(attached, detached);
    let wire = capture.snapshot();
    assert_eq!(wire.discarded_values, 0);
    for channel in [BleWireChannel::Control, BleWireChannel::Data] {
        assert!(wire.values.iter().any(|value| value.channel == channel));
    }
    assert!(
        wire.values
            .iter()
            .filter(|value| value.channel == BleWireChannel::Data)
            .count()
            > 16
    );
    Transcript {
        discovery,
        wire,
        response,
        connection_boundaries,
    }
}

fn exchange(
    runner: &mut ManualTaskRunner<'_, Completion>,
    handle: PrnsNodeHandle,
    remote: personal_rns::wire::DestinationHash,
    response: &[u8],
) {
    let link_handle = handle.clone();
    let link_task = runner
        .insert(async move {
            let link = link_handle
                .establish_link(remote)
                .await
                .unwrap_or_else(|error| unreachable!("link: {error:?}"));
            Completion::Linked { node: 0, link }
        })
        .unwrap_or_else(|error| unreachable!("actor: {error}"));
    let results = settle(runner);
    let [(task, Completion::Linked { node: 0, link })] = results.as_slice() else {
        unreachable!("link settlement: {results:?}")
    };
    assert_eq!(*task, link_task);
    let request_task = request(runner, 0, handle, *link, response.to_vec());
    assert_eq!(
        settle(runner),
        [(
            request_task,
            Completion::Response {
                node: 0,
                bytes: response.to_vec()
            }
        )]
    );
}

#[test]
fn fresh_ble_nodes_replay_every_control_value_and_fragment() {
    let expected = replay(11, 42, Lifecycle::Fresh);
    assert_eq!(replay(11, 42, Lifecycle::Fresh), expected);
    assert_eq!(replay(11, 42, Lifecycle::Fresh), expected);
}

#[test]
fn ble_replay_detects_changed_entropy_and_equal_length_payloads() {
    let baseline = replay(11, 42, Lifecycle::Fresh);
    let seed = replay(21, 42, Lifecycle::Fresh);
    assert_eq!(baseline.response, seed.response);
    assert_ne!(baseline.wire, seed.wire);
    let payload = replay(11, 43, Lifecycle::Fresh);
    assert_eq!(baseline.response.len(), payload.response.len());
    assert_ne!(baseline.response, payload.response);
    assert_ne!(baseline.wire, payload.wire);
}
