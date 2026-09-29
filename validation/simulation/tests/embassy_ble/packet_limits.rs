use std::time::Duration;

use personal_rns::engine::{SendRequestFailure, Settlement};
use personal_rns::interfaces::bluetooth_auto::{
    AppleHost, BlueZHost, Endpoint, Esp32Host, Nrf52Host,
};
use personal_rns::routing::links::LinkId;
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::runtime::{PrnsNodeHandle, RequestOptions, SendError};
use personal_rns::units::{ByteLimit, RttMillis};
use prns_simulation::{ManualMedium, ManualTimeDriver};

use super::clock::{ClockLease, EmbassyTasks};
use super::echo::QUERY_PATH;
use super::fixture::lab;
use super::interop::{converge, establish_pair, EMBASSY_ADDRESS, TOKIO_ADDRESS};
use super::node::{Node, PAYLOAD_BYTES};
use super::tokio_node;

fn exchange(
    tasks: &mut EmbassyTasks<'_>,
    embassy: &Node,
    desktop: &PrnsNodeHandle,
    links: [LinkId; 2],
    body: Vec<u8>,
) {
    let embedded = embassy.handle;
    let desktop = desktop.clone();
    let expected = if body.len() <= PAYLOAD_BYTES {
        Ok((body.clone(), RttMillis::new(0)))
    } else {
        Err(SendError::Failed(SendRequestFailure::ResponseTooLarge))
    };
    let results = tasks.complete_ready(async move {
        tokio::join!(
            embedded.request(links[0], RequestPathHash::of(QUERY_PATH), &body),
            desktop.request_with_options(
                links[1],
                RequestPathHash::of(QUERY_PATH),
                &body,
                RequestOptions {
                    maximum_response_bytes: ByteLimit::Maximum(PAYLOAD_BYTES as u64),
                    ..RequestOptions::default()
                },
            ),
        )
    });
    assert_eq!(
        [results.0.map(|(data, rtt)| (data.to_vec(), rtt)), results.1],
        [expected.clone(), expected],
    );
    assert_eq!(
        embassy
            .take_settled()
            .into_iter()
            .map(|(_, result)| result)
            .collect::<Vec<_>>(),
        vec![Settlement::Respond(Ok(()))],
    );
    assert!(embassy.take_received().is_empty());
    assert!(embassy.take_closed().is_empty());
}

fn scenario(embedded_endpoint: Endpoint, desktop_endpoint: Endpoint) {
    let clock = ClockLease::acquire();
    let lab = lab();
    let mut driver =
        ManualTimeDriver::new(ManualMedium::Ble(lab.clone()), Duration::from_millis(1)).unwrap();
    let mut tasks = EmbassyTasks::new(&mut driver, clock);
    let embassy = Node::start_echo(&mut tasks, &lab, EMBASSY_ADDRESS, embedded_endpoint);
    let mut desktop = tokio_node::start(&mut tasks, &lab, TOKIO_ADDRESS, desktop_endpoint);
    assert!(tasks.settle() > 0);
    let desktop = desktop.try_recv().unwrap();
    converge(&mut tasks, &lab, &embassy, &desktop.handle);
    let links = establish_pair(&mut tasks, &embassy, &desktop.handle);

    // The same raw packet values exercise Tokio's core limit and Embassy's
    // exact completion capacity. Each refusal must leave both links usable.
    for size in [
        PAYLOAD_BYTES - 1,
        PAYLOAD_BYTES,
        PAYLOAD_BYTES + 1,
        PAYLOAD_BYTES + 2,
        PAYLOAD_BYTES,
    ] {
        exchange(
            &mut tasks,
            &embassy,
            &desktop.handle,
            links,
            vec![0xA7; size],
        );
    }
    // Binary value headers are retained, not decoded by either Rust runtime.
    // These three representations fill exactly the same completion capacity.
    for prefix in [&[0xC4, 254][..], &[0xC5, 0, 253], &[0xC6, 0, 0, 0, 251]] {
        let mut body = prefix.to_vec();
        body.resize(PAYLOAD_BYTES, 0xB8);
        exchange(&mut tasks, &embassy, &desktop.handle, links, body);
    }
    assert!(desktop.take_closed().is_empty());
    assert_eq!(embassy.status.setup_failure_events(), 0);
    assert_eq!(lab.active_connection_count(), 1);
    drop(tasks);
    assert_eq!(lab.active_connection_count(), 0);
}

#[test]
fn esp32_and_apple_nodes_enforce_exact_packet_response_limits() {
    scenario(
        Endpoint::Esp32(Esp32Host::Esp32),
        Endpoint::CoreBluetooth(AppleHost::MacOs),
    );
}

#[test]
fn nrf52_and_bluez_nodes_enforce_exact_packet_response_limits() {
    scenario(
        Endpoint::Nrf52(Nrf52Host::Nrf52),
        Endpoint::BlueZ(BlueZHost::Linux),
    );
}
