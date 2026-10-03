use super::*;
use personal_rns::engine::{PrnsCommand, RequestResponseTimeout, SendRequestFailure};
use personal_rns::routing::links::LinkId;
use personal_rns::runtime::SendError;
use personal_rns::storage::GrowableHeap;
use personal_rns::units::DurationMillis;
use prns_simulation::ble::BleSimulationEvent;

const SENDER: u8 = 1;
const RECEIVER: u8 = 2;
const RESTART_AT_MS: u64 = 7;
const OLD_REQUEST_TIMEOUT_MS: u64 = 50;

#[derive(Debug, PartialEq, Eq)]
struct RestartTranscript {
    wire: BleWireSnapshot,
    discovery: BleTraceSnapshot,
    responses: [Vec<u8>; 2],
    restart_boundary: usize,
}

fn receiver(
    tasks: &mut EmbassyTasks<'_>,
    lab: &VirtualBleLab,
    endpoint: Endpoint,
    seed: u8,
) -> node::Node {
    node::Node::with_storage_inputs(
        tasks,
        lab,
        node::NodeInputs {
            address: RECEIVER,
            endpoint,
            entropy_seed: seed,
        },
        [echo::destination(RECEIVER)],
        personal_rns::request_endpoints![echo::Echo],
        GrowableHeap,
    )
}

fn connect(
    tasks: &mut EmbassyTasks<'_>,
    lab: &VirtualBleLab,
    sender: &node::Node,
    receiver: &node::Node,
) -> LinkId {
    let horizon = tick(tasks.snapshot().tick.get() + 60_000);
    for step in 0..16 {
        tasks.settle();
        if sender.status.members().count() == 1 && receiver.status.members().count() == 1 {
            break;
        }
        assert!(step < 15, "bounded discovery after boot");
        tasks.advance_to_next_wake(horizon).unwrap();
    }
    assert_eq!(lab.active_connection_count(), 1);
    let remote = echo::destination(RECEIVER).destination_hash().unwrap();
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
    tasks.complete_ready(async move { handle.establish_link(remote).await.unwrap() })
}

fn exchange(
    tasks: &mut EmbassyTasks<'_>,
    sender: &node::Node,
    link: LinkId,
    marker: u8,
) -> Vec<u8> {
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
    response
}

fn run(endpoint: Endpoint, replacement_seed: u8, marker: u8) -> RestartTranscript {
    let lease = ClockLease::acquire();
    let capture = BleWireCapture::new(NonZeroUsize::new(4096).unwrap());
    let lab = VirtualBleLab::with_wire_capture(
        BleMediumConfig::new(TopologyConfig::FullyConnected, 2, 4, 4, 4096).unwrap(),
        capture.clone(),
    );
    let mut driver =
        ManualTimeDriver::new(ManualMedium::Ble(lab.clone()), Duration::from_millis(1)).unwrap();
    let mut tasks = EmbassyTasks::new(&mut driver, lease);
    let sender =
        node::Node::start_echo(&mut tasks, &lab, SENDER, Endpoint::Esp32(Esp32Host::Esp32));
    let original = receiver(&mut tasks, &lab, endpoint, 2);
    let old_link = connect(&mut tasks, &lab, &sender, &original);
    let before = exchange(&mut tasks, &sender, old_link, marker);
    for step in 0..16 {
        tasks.settle();
        if tasks.snapshot().tick == tick(RESTART_AT_MS) {
            break;
        }
        assert!(step < 15, "bounded timer settlement before restart");
        tasks.advance_to_next_wake(tick(RESTART_AT_MS)).unwrap();
    }
    assert_eq!(tasks.snapshot().tick, tick(RESTART_AT_MS));
    let stale_handle = original.handle;
    let boundary = capture.snapshot();
    original.stop(&mut tasks);
    tasks.settle();
    assert_eq!(lab.active_connection_count(), 0);
    assert_eq!(sender.status.members().count(), 0);
    assert_eq!(capture.snapshot(), boundary);
    let replacement = receiver(&mut tasks, &lab, endpoint, replacement_seed);
    let fresh_link = connect(&mut tasks, &lab, &sender, &replacement);
    assert_ne!(fresh_link, old_link);

    let current = capture.snapshot();
    stale_handle
        .issue(PrnsCommand::AnnounceNow(AnnounceNow {
            destination: echo::destination(RECEIVER).destination_hash().unwrap(),
            target: AnnounceTarget::AllInterfaces,
            app_data: AnnounceAppData::Registered,
        }))
        .unwrap();
    tasks.settle();
    assert_eq!(
        capture.snapshot(),
        current,
        "stale static handle cannot address the replacement actor"
    );
    let handle = sender.handle;
    let request_start = tasks.snapshot().tick.get();
    let _announced = replacement.take_settled();
    assert_eq!(
        tasks.complete_with_budget(
            CompletionBudget {
                deadline: tick(request_start + OLD_REQUEST_TIMEOUT_MS),
                polls_per_tick: NonZeroUsize::new(128).unwrap()
            },
            async move {
                handle
                    .request_with_response_timeout(
                        old_link,
                        RequestPathHash::of(echo::QUERY_PATH),
                        &[42],
                        RequestResponseTimeout::Exact(DurationMillis(OLD_REQUEST_TIMEOUT_MS)),
                    )
                    .await
                    .map(|_| ())
            }
        ),
        Err(SendError::Failed(SendRequestFailure::Timeout))
    );
    assert_eq!(
        tasks.snapshot().tick,
        tick(request_start + OLD_REQUEST_TIMEOUT_MS)
    );
    assert!(
        replacement.take_settled().is_empty(),
        "replacement must not respond on the retired protocol link"
    );
    let after = exchange(&mut tasks, &sender, fresh_link, marker);
    replacement.stop(&mut tasks);
    sender.stop(&mut tasks);
    drop(tasks);
    assert_eq!(lab.active_connection_count(), 0);
    let transcript = RestartTranscript {
        wire: capture.snapshot(),
        discovery: lab.trace(),
        responses: [before, after],
        restart_boundary: boundary.values.len(),
    };
    assert_eq!(transcript.wire.discarded_values, 0);
    assert_eq!(transcript.discovery.discarded_events, 0);
    let (old, new) = transcript.wire.values.split_at(transcript.restart_boundary);
    assert!(!old.is_empty() && !new.is_empty());
    assert_ne!(old[0].connection, new[0].connection);
    for phase in [old, new] {
        assert!(phase
            .iter()
            .all(|value| value.connection == phase[0].connection));
    }
    let mut attached = Vec::new();
    let mut detached = Vec::new();
    for event in &transcript.discovery.events {
        match event {
            BleSimulationEvent::RadioAttached { radio } => attached.push(*radio),
            BleSimulationEvent::RadioDetached { radio } => detached.push(*radio),
            _ => {}
        }
    }
    attached.sort();
    detached.sort();
    assert_eq!(attached.len(), 3);
    assert_eq!(attached, detached);
    transcript
}

#[test]
fn embassy_node_restart_replays_without_reviving_old_handles_or_links() {
    for endpoint in [
        Endpoint::Esp32(Esp32Host::Esp32),
        Endpoint::Nrf52(Nrf52Host::Nrf52),
    ] {
        let expected = run(endpoint, 21, 42);
        for _ in 0..3 {
            assert_eq!(run(endpoint, 21, 42), expected);
        }
        let changed = run(endpoint, 31, 42);
        assert_eq!(changed.responses, expected.responses);
        assert_eq!(changed.restart_boundary, expected.restart_boundary);
        assert_eq!(
            changed.wire.values[..changed.restart_boundary],
            expected.wire.values[..expected.restart_boundary]
        );
        assert_ne!(changed.wire, expected.wire);
        let payload = run(endpoint, 21, 43);
        assert_ne!(payload.responses, expected.responses);
        assert_ne!(payload.wire, expected.wire);
    }
}
