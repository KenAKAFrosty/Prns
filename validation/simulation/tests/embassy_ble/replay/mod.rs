use super::*;
mod fleet;
mod mixed;
mod restart;
use personal_rns::engine::{AnnounceAppData, AnnounceNow, AnnounceTarget};
use personal_rns::routing::request_handlers::RequestPathHash;
use prns_simulation::ble::{
    BleMediumConfig, BleTraceSnapshot, BleWireCapture, BleWireSnapshot, VirtualBleLab,
};
use prns_simulation::TopologyConfig;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Transcript {
    wire: BleWireSnapshot,
    discovery: BleTraceSnapshot,
    response: Vec<u8>,
}

pub(super) fn run(marker: u8) -> Transcript {
    run_seeded(marker, 1)
}

fn run_seeded(marker: u8, entropy_seed: u8) -> Transcript {
    let lease = ClockLease::acquire();
    let capture = BleWireCapture::new(NonZeroUsize::new(4096).unwrap());
    let lab = VirtualBleLab::with_wire_capture(
        BleMediumConfig::new(TopologyConfig::FullyConnected, 2, 4, 4, 4096).unwrap(),
        capture.clone(),
    );
    let mut driver =
        ManualTimeDriver::new(ManualMedium::Ble(lab.clone()), Duration::from_millis(1)).unwrap();
    let mut tasks = EmbassyTasks::new(&mut driver, lease);
    let sender = node::Node::with_storage_inputs(
        &mut tasks,
        &lab,
        node::NodeInputs {
            address: 1,
            endpoint: Endpoint::Esp32(Esp32Host::Esp32),
            entropy_seed,
        },
        [echo::destination(1)],
        personal_rns::request_endpoints![echo::Echo],
        personal_rns::storage::GrowableHeap,
    );
    let receiver = node::Node::start_echo(&mut tasks, &lab, 2, Endpoint::Nrf52(Nrf52Host::Nrf52));
    for step in 0..16 {
        tasks.settle();
        if sender.status.members().count() == 1 && receiver.status.members().count() == 1 {
            break;
        }
        assert!(step < 15, "bounded discovery");
        tasks.advance_to_next_wake(tick(60_000)).unwrap();
    }
    assert_eq!(lab.active_connection_count(), 1);
    let remote = echo::destination(2).destination_hash().unwrap();
    let handle = receiver.handle;
    tasks.complete_ready(async move {
        handle
            .announce_now(AnnounceNow {
                destination: remote,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            })
            .await
            .unwrap();
    });
    let handle = sender.handle;
    let link = tasks.complete_ready(async move { handle.establish_link(remote).await.unwrap() });
    let handle = sender.handle;
    let response = tasks.complete_ready(async move {
        handle
            .request(link, RequestPathHash::of(echo::QUERY_PATH), &[marker; 256])
            .await
            .unwrap()
            .0
            .as_slice()
            .to_vec()
    });
    assert_eq!(response, vec![marker; 256]);
    drop(sender);
    drop(receiver);
    drop(tasks);
    assert_eq!(lab.active_connection_count(), 0);
    let wire = capture.snapshot();
    let discovery = lab.trace();
    assert_eq!(wire.discarded_values, 0);
    assert_eq!(discovery.discarded_events, 0);
    assert_eq!(super::static_storage::footprint().blocks, 16);
    Transcript {
        wire,
        discovery,
        response,
    }
}

#[test]
fn embassy_nodes_repeat_complete_ble_wire_transcripts() {
    let expected = run(42);
    for _ in 0..3 {
        assert_eq!(run(42), expected);
    }
    let changed = run(43);
    assert_ne!(changed.response, expected.response);
    assert_ne!(changed.wire, expected.wire);
    let entropy = run_seeded(42, 21);
    assert_eq!(entropy.response, expected.response);
    assert_ne!(entropy.wire, expected.wire);
}
