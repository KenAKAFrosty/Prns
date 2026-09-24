use super::*;
use personal_rns::interfaces::bluetooth_auto::{
    AppleHost, BleAddress, BleIdentity, BlueZHost, LinkCapabilities, BLE_HW_MTU,
};
use personal_rns::manifold::tokio::TokioHost;
use personal_rns::remote_control::RemoteControlService;
use personal_rns::runtime::{
    CryptoPoolConfig, InterfaceArbitration, InterfaceEventSource, NoPersistence,
    NoRemoteControlHostControls, PrnsNode, PrnsNodeHandle, PrnsNodeRecipe, TokioHandleEntropy,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::units::InstantMillis;
use prns_core::entropy::{EntropySource, RuntimeEntropy};
use prns_interfaces_tokio::bluetooth_auto::BluetoothAuto;
use prns_simulation::{Reachability, TopologyMutation};
use selection::{FirstEvent, RoundRobinBleEvents};

const EMBASSY_ADDRESS: u8 = interop::EMBASSY_ADDRESS;
const TOKIO_ADDRESS: u8 = interop::TOKIO_ADDRESS;

#[derive(Clone, Copy)]
struct Inputs {
    embassy_seed: u8,
    host_seed: u8,
    handle_seed: u8,
    marker: u8,
    first: FirstEvent,
    interface_first: InterfaceEventSource,
    embedded: Endpoint,
    desktop: Endpoint,
}

fn stream(
    seed: u8,
) -> RuntimeEntropy<impl EntropySource<Error = core::convert::Infallible> + Send> {
    let mut reads = 0;
    RuntimeEntropy::try_new(move |bytes: &mut [u8]| {
        reads += 1;
        assert_eq!((reads, bytes.len()), (1, 32), "bounded replay seed");
        bytes.fill(seed);
        Ok::<(), core::convert::Infallible>(())
    })
    .unwrap()
}

fn desktop(tasks: &mut EmbassyTasks<'_>, lab: &VirtualBleLab, inputs: Inputs) -> PrnsNodeHandle {
    let supervisor = BluetoothAuto::<_, MAX_PEERS>::new(
        backend(lab, TOKIO_ADDRESS),
        BleIdentity::new([TOKIO_ADDRESS; 16]),
        inputs.desktop,
        LinkCapabilities {
            l2cap: None,
            link_mtu: BLE_HW_MTU as u16,
        },
    )
    .with_event_selector(RoundRobinBleEvents::new(inputs.first));
    let (send, mut ready) = tokio::sync::oneshot::channel();
    tasks.insert(async move {
        let node = PrnsNode::new_with_entropy_sources(
            |_| PrnsNodeRecipe {
                transport_identity: None,
                remote_control: RemoteControlService::Unavailable.into(),
                pre_configured_destinations: [echo::destination(TOKIO_ADDRESS)],
                app_state: NoRemoteControlHostControls,
                storage: GrowableHeap,
                request_endpoints: personal_rns::request_endpoints![echo::Echo],
                interfaces: move |handle: &PrnsNodeHandle| {
                    let _attached = handle.supervise(supervisor);
                },
                persistence: NoPersistence,
                on_event: |_, _| {},
            },
            TokioHost::with_runtime_entropy(InstantMillis(0), stream(inputs.host_seed)),
            TokioHandleEntropy::from_sources(
                stream(inputs.handle_seed),
                |_bytes: &mut [u8]| -> Result<(), core::convert::Infallible> {
                    unreachable!("announced direct peers do not request path entropy")
                },
            ),
        )
        .with_crypto_pool(CryptoPoolConfig::Inline)
        .with_interface_arbitration(InterfaceArbitration::RoundRobin {
            first: inputs.interface_first,
        });
        assert!(send.send(node.handle()).is_ok());
        let result = node.run().await;
        unreachable!("desktop actor must remain live: {result:?}");
    });
    tasks.settle();
    ready.try_recv().unwrap()
}

#[derive(Debug, PartialEq, Eq)]
struct MixedTranscript {
    wire: BleWireSnapshot,
    discovery: BleTraceSnapshot,
    responses: [[Vec<u8>; 2]; 2],
    reconnect_boundary: usize,
}

fn run(inputs: Inputs) -> MixedTranscript {
    let lease = ClockLease::acquire();
    let capture = BleWireCapture::new(NonZeroUsize::new(4096).unwrap());
    let lab = VirtualBleLab::with_wire_capture(
        BleMediumConfig::new(
            TopologyConfig::Explicit {
                max_neighbors: NonZeroUsize::new(1).unwrap(),
            },
            2,
            4,
            4,
            4096,
        )
        .unwrap(),
        capture.clone(),
    );
    let mut driver =
        ManualTimeDriver::new(ManualMedium::Ble(lab.clone()), Duration::from_millis(1)).unwrap();
    let mut tasks = EmbassyTasks::new(&mut driver, lease);
    let embassy = node::Node::with_storage_inputs(
        &mut tasks,
        &lab,
        node::NodeInputs {
            address: EMBASSY_ADDRESS,
            endpoint: inputs.embedded,
            entropy_seed: inputs.embassy_seed,
        },
        [echo::destination(EMBASSY_ADDRESS)],
        personal_rns::request_endpoints![echo::Echo],
        GrowableHeap,
    );
    let desktop = desktop(&mut tasks, &lab, inputs);
    assert_eq!(
        lab.set_reachability(
            BleAddress::new([EMBASSY_ADDRESS; 6]),
            BleAddress::new([TOKIO_ADDRESS; 6]),
            Reachability::Reachable
        ),
        Ok(TopologyMutation::Applied)
    );
    interop::converge(&mut tasks, &lab, &embassy, &desktop);
    let original_links = interop::establish_pair(&mut tasks, &embassy, &desktop);
    let before = exchange(
        &mut tasks,
        &embassy,
        &desktop,
        original_links,
        inputs.marker,
    );
    let boundary = capture.snapshot();
    let addresses = [
        BleAddress::new([EMBASSY_ADDRESS; 6]),
        BleAddress::new([TOKIO_ADDRESS; 6]),
    ];
    assert_eq!(
        lab.set_reachability(addresses[0], addresses[1], Reachability::Isolated),
        Ok(TopologyMutation::Applied)
    );
    tasks.settle();
    assert_eq!(lab.active_connection_count(), 0);
    assert_eq!(embassy.status.members().count(), 0);
    assert_eq!(
        capture.snapshot(),
        boundary,
        "retired link must accept no more bytes"
    );
    assert_eq!(
        lab.set_reachability(addresses[0], addresses[1], Reachability::Reachable),
        Ok(TopologyMutation::Applied)
    );
    interop::converge(&mut tasks, &lab, &embassy, &desktop);
    let fresh_links = interop::establish_pair(&mut tasks, &embassy, &desktop);
    for link in fresh_links {
        assert!(!original_links.contains(&link));
    }
    let after = exchange(&mut tasks, &embassy, &desktop, fresh_links, inputs.marker);
    drop(desktop);
    drop(embassy);
    drop(tasks);
    assert_eq!(lab.active_connection_count(), 0);
    let transcript = MixedTranscript {
        wire: capture.snapshot(),
        discovery: lab.trace(),
        responses: [before, after],
        reconnect_boundary: boundary.values.len(),
    };
    assert_eq!(transcript.wire.discarded_values, 0);
    assert_eq!(transcript.discovery.discarded_events, 0);
    let (old, new) = transcript
        .wire
        .values
        .split_at(transcript.reconnect_boundary);
    assert!(!old.is_empty() && !new.is_empty());
    assert_ne!(old[0].connection, new[0].connection);
    for phase in [old, new] {
        assert!(phase
            .iter()
            .all(|value| value.connection == phase[0].connection));
    }
    transcript
}

fn exchange(
    tasks: &mut EmbassyTasks<'_>,
    embassy: &node::Node,
    desktop: &PrnsNodeHandle,
    links: [personal_rns::routing::links::LinkId; 2],
    marker: u8,
) -> [Vec<u8>; 2] {
    let embedded = embassy.handle;
    let handle = desktop.clone();
    let responses = tasks.complete_ready(async move {
        let payload = [marker; 256];
        let (embedded, desktop) = tokio::join!(
            embedded.request(links[0], RequestPathHash::of(echo::QUERY_PATH), &payload),
            handle.request(links[1], RequestPathHash::of(echo::QUERY_PATH), &payload),
        );
        [embedded.unwrap().0.as_slice().to_vec(), desktop.unwrap().0]
    });
    assert_eq!(responses, [vec![marker; 256], vec![marker; 256]]);
    responses
}

#[test]
fn mixed_nodes_repeat_complete_bidirectional_reconnect_transcripts() {
    for (embedded, desktop) in [
        (
            Endpoint::Esp32(Esp32Host::Esp32),
            Endpoint::BlueZ(BlueZHost::Linux),
        ),
        (
            Endpoint::Nrf52(Nrf52Host::Nrf52),
            Endpoint::CoreBluetooth(AppleHost::MacOs),
        ),
    ] {
        for first in FirstEvent::ALL {
            for interface_first in [
                InterfaceEventSource::Message,
                InterfaceEventSource::Completion,
            ] {
                let inputs = Inputs {
                    embassy_seed: 1,
                    host_seed: 2,
                    handle_seed: 3,
                    marker: 42,
                    first,
                    interface_first,
                    embedded,
                    desktop,
                };
                let expected = run(inputs);
                for _ in 0..3 {
                    assert_eq!(run(inputs), expected);
                }
                let payload = run(Inputs {
                    marker: 43,
                    ..inputs
                });
                assert_ne!(payload.responses, expected.responses);
                assert_ne!(payload.wire, expected.wire);
                for changed in [
                    Inputs {
                        embassy_seed: 11,
                        ..inputs
                    },
                    Inputs {
                        host_seed: 12,
                        ..inputs
                    },
                ] {
                    let changed = run(changed);
                    assert_eq!(changed.responses, expected.responses);
                    assert_ne!(changed.wire, expected.wire);
                }
            }
        }
    }
}
