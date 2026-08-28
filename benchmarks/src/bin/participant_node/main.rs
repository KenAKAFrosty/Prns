#![allow(
    clippy::default_constructed_unit_structs,
    clippy::single_match,
    clippy::too_many_arguments,
    clippy::unit_arg
)]

mod link_channel;
mod request;
mod resource;

use personal_rns::runtime::NoPersistence;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{sync::Arc, time::Duration};

use benchmarks::{
    deterministic_payload, ScenarioId, ScenarioManifest as Manifest, SizeSequence,
    WorkloadProfile as Profile,
};
use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, EstablishLink, LinkClosedReason,
    PrnsCommand, RatchetPolicy, RequestResponseTimeout, SendSinglePacket, SendSinglePacketPayload,
    SendToLink, SendToLinkFailure, SendToLinkPayload, Settlement,
};
use personal_rns::interfaces::{
    tcp, BitrateBps, ConfiguredInterfacePolicy, EffectiveInterfacePolicy, InterfaceDescriptor,
    InterfaceId, InterfaceKind, MtuPolicy, ReportsStatus,
};
use personal_rns::manifold::interface_seam::{Interface, InterfaceSeam};
use personal_rns::manifold::reconnect::ReconnectPolicy;
use personal_rns::request_endpoints;
use personal_rns::routing::delivery::Delivery;
use personal_rns::routing::links::resources::{ResourceStrategy, MAX_EFFICIENT_SIZE};
use personal_rns::routing::links::LinkId;
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::request_endpoints::{
    Decline, RequestContext, RequestEndpoint, RequestEndpointPolicy, RequestEndpointSet,
};
use personal_rns::runtime::{
    generate_identity_secret, Diagnostic, Message, PreConfiguredDestination, PrnsEvent, PrnsNode,
    PrnsNodeHandle, PrnsNodeRecipe, SegmentCompression, ServeMyRequestEndpoints,
};
#[cfg(feature = "fixed-storage")]
type NodeStorage = personal_rns::storage::Esp32S3<allocator_api2::alloc::Global>;
#[cfg(not(feature = "fixed-storage"))]
use personal_rns::storage::GrowableHeap as NodeStorage;
use personal_rns::tcp::{tune, TcpClientInterface, TcpServerConnection};
use personal_rns::units::DurationMillis;
use personal_rns::wire::DestinationHash;
use tokio::io::AsyncRead;
use tokio::sync::mpsc;

const TCP_INTERFACE_ID: InterfaceId = InterfaceId::new([0xBE; 8]);
const RELAY_SECOND_INTERFACE_ID: InterfaceId = InterfaceId::new([0xBF; 8]);

/// The optimization profile this binary was built under, tagged onto every measuring `RESULT` line
/// so a perf consumer can refuse a debug build: unoptimized crypto runs ~10x slower, so a debug
/// run's throughput and latency are meaningless while its conformance counts stay valid.
const BUILD_PROFILE: &str = if cfg!(debug_assertions) {
    "debug"
} else {
    "release"
};

struct BenchTcpListener {
    id: InterfaceId,
    listener: tokio::net::TcpListener,
    policy: EffectiveInterfacePolicy,
}

impl BenchTcpListener {
    async fn bind_with_id(
        id: InterfaceId,
        addr: impl tokio::net::ToSocketAddrs,
        policy: EffectiveInterfacePolicy,
    ) -> std::io::Result<Self> {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        Ok(Self {
            id,
            listener,
            policy,
        })
    }

    fn local_addr(&self) -> std::io::Result<std::net::SocketAddr> {
        self.listener.local_addr()
    }
}

impl Interface for BenchTcpListener {
    const HW_MTU: usize = tcp::TCP_HW_MTU_CAP;
    const KIND: InterfaceKind = InterfaceKind::TcpServerPeer;

    fn descriptor(&self) -> InterfaceDescriptor {
        tcp::descriptor(self.id, self.policy)
    }

    fn channel_tag(&self) -> &[u8] {
        self.id.as_bytes()
    }

    async fn run<Seam: InterfaceSeam>(self, seam: Seam) {
        let Ok((stream, peer)) = self.listener.accept().await else {
            return;
        };
        tune(&stream);
        TcpServerConnection::with_policy(peer.to_string().into_bytes(), stream, self.policy)
            .run(seam)
            .await;
    }
}

impl ReportsStatus for BenchTcpListener {}

fn fanin_listener_id(index: usize) -> InterfaceId {
    let mut id = [0xC0u8; 8];
    id[7] = index as u8;
    InterfaceId::new(id)
}
const DRAIN_GRACE: Duration = Duration::from_secs(5);

fn event_channel(profile: &Profile) -> (mpsc::Sender<Event>, mpsc::Receiver<Event>) {
    let capacity = profile
        .window
        .saturating_mul(2)
        .saturating_add(profile.initiator_count.saturating_mul(4))
        .saturating_add(32);
    mpsc::channel(capacity)
}

fn send_event(sender: &mpsc::Sender<Event>, event: Event) {
    match sender.try_send(event) {
        Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => {}
        Err(mpsc::error::TrySendError::Full(_)) => {
            panic!("benchmark event queue overflow");
        }
    }
}

fn benchmark_tcp_policy(profile: &Profile) -> EffectiveInterfacePolicy {
    tcp::configured_policy(ConfiguredInterfacePolicy {
        bitrate: profile.tcp_bitrate_bps.map(BitrateBps::guess),
        mtu: (profile.link_mtu > 0).then(|| MtuPolicy::fixed(profile.link_mtu)),
        ..ConfiguredInterfacePolicy::default()
    })
}

/// The manifest's compression posture for resource sends. `"off"` is the matrix's
/// transport-only baseline, matching the reference harness's `auto_compress=False`;
/// `"auto"` is both stacks' shipping default posture.
fn segment_compression(profile: &Profile) -> SegmentCompression {
    match profile.compression.as_str() {
        "off" => SegmentCompression::Never,
        "auto" => SegmentCompression::AUTO,
        other => panic!("unknown compression posture {other:?} (expected \"off\" or \"auto\")"),
    }
}

fn responder_resource_strategy(profile: &Profile) -> ResourceStrategy {
    ResourceStrategy::Accept {
        max_uncompressed_bytes: 128 * 1024 * 1024,
        accept_compressed: matches!(
            segment_compression(profile),
            SegmentCompression::Attempt { .. }
        ),
    }
}

enum Event {
    Heard(DestinationHash),
    Settled(CommandId, Settlement),
    FirstDelivered,
    LinkUp,
    ResourceIn { link_id: LinkId, bytes: usize },
    ResourceAck(u64),
    Closed,
}

#[derive(Default)]
struct DeliveryCounters {
    delivered: AtomicU64,
    payload_bytes: AtomicU64,
}

impl DeliveryCounters {
    fn record(&self, bytes: usize) -> bool {
        self.payload_bytes
            .fetch_add(bytes as u64, Ordering::Relaxed);
        self.delivered.fetch_add(1, Ordering::Release) == 0
    }

    fn snapshot(&self) -> (u64, u64) {
        (
            self.delivered.load(Ordering::Acquire),
            self.payload_bytes.load(Ordering::Relaxed),
        )
    }
}

const REQUEST_PATH: &str = "/bench/query";
const RESOURCE_ACK_PREFIX: &[u8; 8] = b"PRNSRACK";

fn resource_ack_payload(sequence: u64) -> [u8; 16] {
    let mut payload = [0u8; 16];
    payload[..8].copy_from_slice(RESOURCE_ACK_PREFIX);
    payload[8..].copy_from_slice(&sequence.to_be_bytes());
    payload
}

fn parse_resource_ack(payload: &[u8]) -> Option<u64> {
    if payload.len() != 16 || &payload[..8] != RESOURCE_ACK_PREFIX {
        return None;
    }
    Some(u64::from_be_bytes(payload[8..].try_into().ok()?))
}

/// The engine's request/response codec carries the app's data as RAW msgpack value bytes. The
/// reference packs and unpacks its side natively, so this bench frames every payload as a
/// msgpack bin value to speak across.
fn begin_msgpack_bin(payload_len: usize, framed: &mut Vec<u8>) {
    framed.clear();
    framed.reserve(payload_len + 3);
    if payload_len <= 0xFF {
        framed.push(0xC4);
        framed.push(payload_len as u8);
    } else {
        framed.push(0xC5);
        framed.extend_from_slice(&(payload_len as u16).to_be_bytes());
    }
}

fn msgpack_bin_payload(framed: &[u8]) -> &[u8] {
    match framed.first() {
        Some(0xC4) => &framed[2..],
        Some(0xC5) => &framed[3..],
        _ => framed,
    }
}

fn incompressible_payload(len: usize) -> Vec<u8> {
    deterministic_payload(len)
}

fn drain_grace(profile: &Profile) -> Duration {
    Duration::from_millis(profile.drain_timeout_ms)
}

/// Lowercase-hex text over the same stream: four bits of entropy per byte, so every
/// segment's compression attempt keeps (~2:1) and the wire carries bz2.
fn compressible_payload(len: usize) -> Vec<u8> {
    const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
    incompressible_payload(len.div_ceil(2))
        .into_iter()
        .flat_map(|byte| {
            [
                HEX_DIGITS[usize::from(byte >> 4)],
                HEX_DIGITS[usize::from(byte & 0x0F)],
            ]
        })
        .take(len)
        .collect()
}

fn scenario_payload(profile: &Profile, len: usize) -> Vec<u8> {
    match profile.payload_shape.as_str() {
        "dense" => incompressible_payload(len),
        "compressible" => compressible_payload(len),
        other => panic!("unknown payload shape {other:?} (expected \"dense\" or \"compressible\")"),
    }
}

async fn await_measurement_start() {
    tokio::task::spawn_blocking(|| {
        use std::io::BufRead as _;

        println!("MEASURE_READY");
        let mut command = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut command)
            .expect("read measurement command");
        assert_eq!(
            command.trim(),
            "START",
            "expected START measurement command"
        );
    })
    .await
    .expect("measurement gate task");
}

async fn await_startup_go() {
    tokio::task::spawn_blocking(|| {
        use std::io::BufRead as _;

        let mut command = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut command)
            .expect("read startup command");
        assert_eq!(
            command.trim(),
            "STARTUP",
            "expected STARTUP command after both participants report ready"
        );
    })
    .await
    .expect("startup gate task");
}

async fn await_stop() {
    tokio::task::spawn_blocking(|| {
        use std::io::BufRead as _;

        let mut command = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut command)
            .expect("read relay stop command");
        assert_eq!(command.trim(), "STOP", "expected STOP relay command");
    })
    .await
    .expect("relay stop task");
}

fn collection_target_receiver() -> tokio::sync::oneshot::Receiver<(u64, u64)> {
    let (send, receive) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        use std::io::BufRead as _;

        let mut command = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut command)
            .expect("read collection target");
        let mut fields = command.split_whitespace();
        assert_eq!(fields.next(), Some("COLLECT"), "collection command");
        let transfers = fields
            .next()
            .expect("collection transfer target")
            .parse::<u64>()
            .expect("numeric collection transfer target");
        let bytes = fields
            .next()
            .expect("collection byte target")
            .parse::<u64>()
            .expect("numeric collection byte target");
        assert!(
            fields.next().is_none(),
            "collection command has two targets"
        );
        let _ = send.send((transfers, bytes));
    });
    receive
}

fn await_collection_release() {
    use std::io::BufRead as _;

    let mut command = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut command)
        .expect("read collection release");
    assert_eq!(command.trim(), "COLLECTED", "collection release command");
}

fn percentile(sorted: &[u64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let rank = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[rank.min(sorted.len() - 1)] as f64
}

struct ExactMillisHistogram {
    bins: Vec<u64>,
    samples: u64,
}

impl ExactMillisHistogram {
    fn new(max_millis: u64) -> Self {
        Self {
            bins: vec![0; max_millis.saturating_add(1) as usize],
            samples: 0,
        }
    }

    fn record(&mut self, millis: u64) {
        let bin = self
            .bins
            .get_mut(millis as usize)
            .unwrap_or_else(|| panic!("benchmark RTT {millis} ms exceeds histogram bound"));
        *bin += 1;
        self.samples += 1;
    }

    fn percentile(&self, p: f64) -> f64 {
        if self.samples == 0 {
            return f64::NAN;
        }
        let rank = ((self.samples as f64 - 1.0) * p).round() as u64;
        let mut seen = 0u64;
        for (millis, count) in self.bins.iter().copied().enumerate() {
            seen += count;
            if seen > rank {
                return millis as f64;
            }
        }
        unreachable!("histogram sample count matches its bins")
    }
}

fn percentile_f64(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let rank = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

const SCENARIO_STACK_BYTES: usize = 64 * 1024 * 1024;

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static DHAT_ALLOC: dhat::Alloc = dhat::Alloc;

fn main() {
    if cfg!(debug_assertions) {
        eprintln!("================================================================");
        eprintln!("participant_node is a DEBUG build: crypto runs ~10x slower than release.");
        eprintln!("Throughput and latency numbers are INVALID and must not be recorded as");
        eprintln!("performance. Conformance counts (sent/delivered/timeouts) stay valid.");
        eprintln!("Rebuild with --release before any performance measurement.");
        eprintln!("================================================================");
    }
    std::thread::Builder::new()
        .stack_size(SCENARIO_STACK_BYTES)
        .spawn(run_scenario)
        .expect("spawns the scenario thread")
        .join()
        .expect("the scenario thread runs to completion");
}

fn run_scenario() {
    #[cfg(feature = "dhat-heap")]
    let dhat = dhat::Profiler::new_heap();
    let worker_threads = std::env::var("SCENARIO_WORKERS")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(2);
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(worker_threads)
        .thread_stack_size(SCENARIO_STACK_BYTES)
        .enable_all()
        .build()
        .expect("builds the scenario runtime")
        .block_on(scenario_main());
    #[cfg(feature = "dhat-heap")]
    {
        let stats = dhat::HeapStats::get();
        eprintln!(
            "DHAT total_blocks={} total_bytes={} max_blocks={} max_bytes={}",
            stats.total_blocks, stats.total_bytes, stats.max_blocks, stats.max_bytes
        );
        drop(dhat);
    }
}

async fn scenario_main() {
    let mut args = std::env::args().skip(1);
    let usage =
        "usage: participant_node <manifest.json> <responder|initiator> <addr> [duration-ms]";
    let manifest_path = args.next().expect(usage);
    let role = args.next().expect(usage);
    let addr = args.next().expect(usage);
    let duration_override: Option<u64> = args.next().map(|s| s.parse().expect("duration-ms"));

    let manifest: Manifest =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).expect("read manifest"))
            .expect("parse manifest");
    let duration = Duration::from_millis(duration_override.unwrap_or(manifest.profile.duration_ms));

    if role == "relay" {
        run_relay(&manifest, &addr).await;
        return;
    }

    match manifest.name {
        ScenarioId::SinglePacketThroughput | ScenarioId::LinkMessageThroughput => {
            run_runtime_endpoint(&manifest, &role, &addr, duration).await;
        }
        ScenarioId::RequestResponse => {
            run_request_endpoint(&manifest, &role, &addr, duration).await;
        }
        ScenarioId::ResourceMaxSegment
        | ScenarioId::ResourceMaxSegmentUnleashed
        | ScenarioId::Resource64mibStream
        | ScenarioId::Resource64mibStreamUnleashed => {
            run_resource_endpoint(&manifest, &role, &addr, duration).await;
        }
        ScenarioId::RawTransportThroughput
        | ScenarioId::TransportResourceThroughput
        | ScenarioId::TransportResourceThroughputUnleashed => {
            panic!("raw transport only supports the relay role")
        }
    }
}

use link_channel::run_runtime_endpoint;
use request::run_request_endpoint;
use resource::run_resource_endpoint;

async fn run_relay(manifest: &Manifest, addr: &str) {
    assert!(
        manifest.name.is_transport(),
        "the relay role belongs to raw transport"
    );
    let policy = benchmark_tcp_policy(&manifest.profile);
    let bitrate_bps = policy.bitrate.get();
    let mtu_bytes = policy
        .mtu
        .resolve(policy.bitrate)
        .expect("TCP benchmark policy selects an MTU tier");
    let side_a = BenchTcpListener::bind_with_id(TCP_INTERFACE_ID, addr, policy)
        .await
        .expect("binds relay side A");
    let side_b = BenchTcpListener::bind_with_id(RELAY_SECOND_INTERFACE_ID, "127.0.0.1:0", policy)
        .await
        .expect("binds relay side B");
    let addr_a = side_a.local_addr().expect("relay side A address");
    let addr_b = side_b.local_addr().expect("relay side B address");
    let node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: Some(generate_identity_secret()),
        pre_configured_destinations: std::iter::empty::<PreConfiguredDestination<'static>>(),
        app_state: (),
        storage: NodeStorage::default(),
        request_endpoints: request_endpoints![],
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        on_event: |_: PrnsEvent<'_>, _: &()| {},
        interfaces: |node: &PrnsNodeHandle| {
            node.add_interface(side_a);
            node.add_interface(side_b);
        },
        persistence: NoPersistence,
    });

    println!(
        "READY role=relay addr={addr_a}>{addr_b} bitrate_bps={bitrate_bps} mtu_bytes={mtu_bytes}"
    );
    tokio::select! {
        result = node.run() => {
            result.expect("relay node remains healthy");
        }
        () = await_stop() => {}
    }
}

async fn build_responder_node<St, R, F>(
    single: PreConfiguredDestination<'static>,
    app_state: St,
    request_endpoints: R,
    on_event: F,
    manifest: &Manifest,
    addr: &str,
) -> (PrnsNode<St, R, F, NodeStorage>, String)
where
    R: RequestEndpointSet<St>,
    F: FnMut(PrnsEvent<'_>, &St),
{
    let tcp_policy = benchmark_tcp_policy(&manifest.profile);
    let primary = BenchTcpListener::bind_with_id(TCP_INTERFACE_ID, addr, tcp_policy)
        .await
        .expect("binds the scenario port");
    let mut addresses = primary.local_addr().expect("bound address").to_string();
    let mut servers = vec![primary];
    for index in 0..manifest.profile.initiator_count.saturating_sub(1) {
        let extra =
            BenchTcpListener::bind_with_id(fanin_listener_id(index), "127.0.0.1:0", tcp_policy)
                .await
                .expect("binds an extra listener");
        addresses.push('+');
        addresses.push_str(&extra.local_addr().expect("bound address").to_string());
        servers.push(extra);
    }
    let node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [single],
        app_state,
        storage: NodeStorage::default(),
        request_endpoints,
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        on_event,
        interfaces: |node: &PrnsNodeHandle| {
            for server in servers {
                node.add_interface(server);
            }
        },
        persistence: NoPersistence,
    });
    (node, addresses)
}

async fn build_initiator_node<F>(
    single: PreConfiguredDestination<'static>,
    on_event: F,
    manifest: &Manifest,
    addr: &str,
) -> PrnsNode<(), (), F, NodeStorage>
where
    F: FnMut(PrnsEvent<'_>, &()),
{
    let client = TcpClientInterface::with_id_and_policy(
        TCP_INTERFACE_ID,
        addr.to_string(),
        benchmark_tcp_policy(&manifest.profile),
        ReconnectPolicy::STANDARD,
    );
    PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [single],
        app_state: (),
        storage: NodeStorage::default(),
        request_endpoints: request_endpoints![],
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        on_event,
        interfaces: |node: &PrnsNodeHandle| {
            node.attach(client);
        },
        persistence: NoPersistence,
    })
}

fn failure_streak_limit(window: usize) -> u64 {
    (window as u64 * 8).max(64)
}

fn died_marker(died: bool) -> &'static str {
    if died {
        " died=1"
    } else {
        ""
    }
}

#[cfg(test)]
mod tests {
    use super::{
        parse_resource_ack, percentile_f64, resource_ack_payload, send_event, Event,
        ExactMillisHistogram,
    };
    use personal_rns::wire::DestinationHash;

    #[test]
    fn request_percentiles_preserve_sub_millisecond_precision() {
        let samples = [0.125, 0.250, 0.375];
        assert_eq!(percentile_f64(&samples, 0.50), 0.250);
        assert!(percentile_f64(&samples, 0.50) > 0.0);
    }

    #[test]
    fn resource_acknowledgements_are_typed_and_sequence_bound() {
        let payload = resource_ack_payload(42);
        assert_eq!(parse_resource_ack(&payload), Some(42));
        assert_eq!(parse_resource_ack(b"not-a-resource-ack"), None);
    }

    #[test]
    fn bounded_event_queue_rejects_overflow_but_allows_teardown_closure() {
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        send_event(&sender, Event::Heard(DestinationHash::new([1; 16])));
        let overflow = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            send_event(&sender, Event::Heard(DestinationHash::new([2; 16])));
        }));
        assert!(overflow.is_err());
        drop(receiver);
        send_event(&sender, Event::Heard(DestinationHash::new([3; 16])));
    }

    #[test]
    fn integer_histogram_matches_exact_nearest_rank_percentiles() {
        let mut histogram = ExactMillisHistogram::new(100);
        for sample in [1, 2, 2, 9, 100] {
            histogram.record(sample);
        }
        assert_eq!(histogram.percentile(0.50), 2.0);
        assert_eq!(histogram.percentile(0.99), 100.0);
    }
}
