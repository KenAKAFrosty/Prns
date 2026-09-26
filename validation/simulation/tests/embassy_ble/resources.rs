use std::future::Future;
use std::num::NonZeroUsize;
use std::time::Duration;

use personal_rns::engine::{
    RequestResponseTimeout, RespondFailure, SendRequestFailure, SendResourceFailure, Settlement,
    MAX_RESPOND_DATA_LEN, MAX_SEND_REQUEST_DATA_LEN,
};
use personal_rns::interfaces::bluetooth_auto::{
    AppleHost, BlueZHost, Endpoint, Esp32Host, Nrf52Host,
};
use personal_rns::routing::links::request::{write_response_plaintext, RESPONSE_WIRE_OVERHEAD};
use personal_rns::routing::links::LinkId;
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::runtime::request_endpoints::{
    Decline, RequestContext, RequestEndpoint, RequestEndpointPolicy,
};
use personal_rns::runtime::{
    NoRemoteControlHostControls, PrnsNodeApi, PrnsNodeHandle, RequestOptions, SendError,
};
use personal_rns::units::{ByteLimit, RttMillis};
use prns_runtime::resource_compression::{compression_preflight, CompressionPreflight};
use prns_simulation::ble::{BleMediumConfig, BleSimulationEvent, VirtualBleLab};
use prns_simulation::{ManualMedium, ManualTimeDriver, TopologyConfig};

use super::clock::{ClockLease, CompletionBudget, EmbassyTasks};
use super::echo::{destination_with_limit, Echo};
use super::interop::{converge, establish_pair, EMBASSY_ADDRESS, TOKIO_ADDRESS};
use super::node::NodeFixture;
use super::{tick, tokio_node};

const REQUEST_BYTES: usize = 2048;
const RESPONSE_BYTES: usize = 2048;
const TRANSFER_BYTES: usize = 1200;
const RESPONSE_ENVELOPE_BYTES: usize = RESPONSE_WIRE_OVERHEAD + TRANSFER_BYTES;
const TRANSFER_BUDGET_MS: u64 = 10_000;
const TRANSFER_POLLS_PER_TICK: usize = 256;
const TRACE_CAPACITY: usize = 4096;
const ECHO_PATH: &str = "/simulation/resource-echo";
const REPLY_PATH: &str = "/simulation/resource-reply";
type ResourceNode = NodeFixture<RESPONSE_BYTES, REQUEST_BYTES>;

// Application data only, not an entropy source. Dense bytes exercise the existing
// uncompressed path without invoking workers outside the manually polled actors.
const fn dense_payload() -> [u8; TRANSFER_BYTES] {
    let mut bytes = [0; TRANSFER_BYTES];
    let mut state = 0x914C_70A3u32;
    let mut index = 0;
    while index < bytes.len() {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        bytes[index] = state as u8;
        index += 1;
    }
    bytes
}

const PAYLOAD: [u8; TRANSFER_BYTES] = dense_payload();
const _: () = {
    assert!(TRANSFER_BYTES > MAX_SEND_REQUEST_DATA_LEN);
    assert!(TRANSFER_BYTES > MAX_RESPOND_DATA_LEN);
};

struct ResourceEcho;

impl RequestEndpoint<NoRemoteControlHostControls> for ResourceEcho {
    const ENDPOINT_ID: &'static str = ECHO_PATH;
    const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;

    async fn handle(
        mut context: RequestContext<'_, NoRemoteControlHostControls>,
        _node: &impl PrnsNodeApi,
    ) -> Result<(), Decline> {
        context.respond_resource(context.data)
    }
}

struct ResourceReply;

impl RequestEndpoint<NoRemoteControlHostControls> for ResourceReply {
    const ENDPOINT_ID: &'static str = REPLY_PATH;
    const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;

    async fn handle(
        mut context: RequestContext<'_, NoRemoteControlHostControls>,
        _node: &impl PrnsNodeApi,
    ) -> Result<(), Decline> {
        let encoded: [u8; 2] = context.data.try_into().map_err(|_| Decline::Ignore)?;
        let requested = usize::from(u16::from_be_bytes(encoded));
        let bytes = PAYLOAD.get(..requested).ok_or(Decline::Ignore)?;
        let mut packed = vec![0; RESPONSE_WIRE_OVERHEAD + requested];
        let len = write_response_plaintext(&context.respond_token().request_id, bytes, &mut packed)
            .unwrap();
        assert!(matches!(
            compression_preflight(&packed[..len]),
            CompressionPreflight::ShipUncompressed
        ));
        context.respond_resource(bytes)
    }
}

fn expected(
    size: usize,
    elapsed: RttMillis,
) -> Result<(Vec<u8>, RttMillis), SendError<SendRequestFailure>> {
    Ok((PAYLOAD[..size].to_vec(), elapsed))
}

async fn measured<T>(future: impl Future<Output = T>) -> (T, RttMillis) {
    let started = embassy_time::Instant::now();
    let result = future.await;
    (
        result,
        RttMillis::new((embassy_time::Instant::now() - started).as_millis()),
    )
}

fn complete<T: 'static>(
    tasks: &mut EmbassyTasks<'_>,
    future: impl Future<Output = T> + 'static,
) -> T {
    let deadline = tick(tasks.snapshot().tick.get() + TRANSFER_BUDGET_MS);
    tasks.complete_with_budget(
        CompletionBudget {
            deadline,
            polls_per_tick: NonZeroUsize::new(TRANSFER_POLLS_PER_TICK).unwrap(),
        },
        future,
    )
}

fn response_settlements(node: &ResourceNode) -> Vec<Settlement> {
    node.take_settled()
        .into_iter()
        .map(|(_, settlement)| settlement)
        .collect()
}

fn upload_echo(
    tasks: &mut EmbassyTasks<'_>,
    node: &ResourceNode,
    desktop: &PrnsNodeHandle,
    link: LinkId,
) {
    let desktop = desktop.clone();
    let (result, elapsed) = complete(tasks, async move {
        measured(desktop.request(
            link,
            RequestPathHash::of(ECHO_PATH),
            &PAYLOAD[..TRANSFER_BYTES],
        ))
        .await
    });
    assert_eq!(result, expected(TRANSFER_BYTES, elapsed));
    assert_eq!(response_settlements(node), [Settlement::Respond(Ok(()))]);
}

fn simultaneous_replies(
    tasks: &mut EmbassyTasks<'_>,
    node: &ResourceNode,
    desktop: &PrnsNodeHandle,
    links: [LinkId; 2],
) {
    let embedded = node.handle;
    let desktop = desktop.clone();
    let ((embedded, embedded_elapsed), (desktop, desktop_elapsed)) = complete(tasks, async move {
        let size = (TRANSFER_BYTES as u16).to_be_bytes();
        tokio::join!(
            measured(embedded.request(links[0], RequestPathHash::of(REPLY_PATH), &size)),
            measured(desktop.request(links[1], RequestPathHash::of(REPLY_PATH), &size)),
        )
    });
    assert_eq!(
        embedded.map(|(bytes, rtt)| (bytes.as_slice().to_vec(), rtt)),
        expected(TRANSFER_BYTES, embedded_elapsed)
    );
    assert_eq!(desktop, expected(TRANSFER_BYTES, desktop_elapsed));
    assert_eq!(response_settlements(node), [Settlement::Respond(Ok(()))]);
}

fn desktop_resource_envelope_limit(
    tasks: &mut EmbassyTasks<'_>,
    node: &ResourceNode,
    desktop: &PrnsNodeHandle,
    link: LinkId,
) {
    for maximum in [RESPONSE_ENVELOPE_BYTES - 1, RESPONSE_ENVELOPE_BYTES] {
        let desktop = desktop.clone();
        let (result, elapsed) = complete(tasks, async move {
            measured(desktop.request_with_options(
                link,
                RequestPathHash::of(REPLY_PATH),
                &(TRANSFER_BYTES as u16).to_be_bytes(),
                RequestOptions {
                    response_timeout: RequestResponseTimeout::LinkDefault,
                    maximum_response_bytes: ByteLimit::Maximum(maximum as u64),
                },
            ))
            .await
        });
        let (response, settlement) = if maximum == RESPONSE_ENVELOPE_BYTES {
            (
                expected(TRANSFER_BYTES, elapsed),
                Settlement::Respond(Ok(())),
            )
        } else {
            (
                Err(SendError::Failed(SendRequestFailure::ResponseTooLarge)),
                Settlement::Respond(Err(RespondFailure::Resource(
                    SendResourceFailure::RejectedByPeer,
                ))),
            )
        };
        assert_eq!(result, response);
        assert_eq!(response_settlements(node), [settlement]);
    }
}

fn scenario(embedded_endpoint: Endpoint, desktop_endpoint: Endpoint) {
    let clock = ClockLease::acquire();
    let lab = VirtualBleLab::new(
        BleMediumConfig::new(TopologyConfig::FullyConnected, 2, 4, 4, TRACE_CAPACITY).unwrap(),
    );
    let mut driver =
        ManualTimeDriver::new(ManualMedium::Ble(lab.clone()), Duration::from_millis(1)).unwrap();
    let mut tasks = EmbassyTasks::new(&mut driver, clock);
    let destination =
        |address| destination_with_limit(address, ByteLimit::Maximum(REQUEST_BYTES as u64));
    let embedded = ResourceNode::with_endpoints(
        &mut tasks,
        &lab,
        EMBASSY_ADDRESS,
        embedded_endpoint,
        [destination(EMBASSY_ADDRESS)],
        personal_rns::request_endpoints![Echo, ResourceEcho, ResourceReply],
    );
    let mut desktop = tokio_node::with_endpoints(
        &mut tasks,
        &lab,
        TOKIO_ADDRESS,
        desktop_endpoint,
        [destination(TOKIO_ADDRESS)],
        personal_rns::request_endpoints![Echo, ResourceEcho, ResourceReply],
    );
    assert!(tasks.settle() > 0);
    let desktop = desktop.try_recv().unwrap();
    converge(&mut tasks, &lab, &embedded, &desktop.handle);
    let links = establish_pair(&mut tasks, &embedded, &desktop.handle);
    upload_echo(&mut tasks, &embedded, &desktop.handle, links[1]);
    simultaneous_replies(&mut tasks, &embedded, &desktop.handle, links);
    desktop_resource_envelope_limit(&mut tasks, &embedded, &desktop.handle, links[1]);
    simultaneous_replies(&mut tasks, &embedded, &desktop.handle, links);
    upload_echo(&mut tasks, &embedded, &desktop.handle, links[1]);
    assert!(embedded.take_received().is_empty());
    assert!(embedded.take_closed().is_empty());
    assert!(desktop.take_closed().is_empty());
    drop(tasks);
    assert_eq!(lab.active_connection_count(), 0);
    let trace = lab.trace();
    assert_eq!(trace.discarded_events, 0);
    let mut attached = Vec::new();
    let mut detached = Vec::new();
    for event in trace.events {
        match event {
            BleSimulationEvent::RadioAttached { radio } => attached.push(radio),
            BleSimulationEvent::RadioDetached { radio } => detached.push(radio),
            _ => {}
        }
    }
    attached.sort();
    detached.sort();
    assert_eq!(attached.len(), 2);
    assert_eq!(detached, attached);
}

#[test]
fn esp32_and_apple_nodes_transfer_resources_and_enforce_response_limits() {
    scenario(
        Endpoint::Esp32(Esp32Host::Esp32),
        Endpoint::CoreBluetooth(AppleHost::MacOs),
    );
}

#[test]
fn nrf52_and_bluez_nodes_transfer_resources_and_enforce_response_limits() {
    scenario(
        Endpoint::Nrf52(Nrf52Host::Nrf52),
        Endpoint::BlueZ(BlueZHost::Linux),
    );
}
