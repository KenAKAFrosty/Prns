#![allow(
    clippy::default_constructed_unit_structs,
    clippy::single_match,
    clippy::too_many_arguments,
    clippy::unit_arg
)]

mod link_channel;
mod request;
mod resource;

use std::sync::atomic::{AtomicU64, Ordering};
use std::{sync::Arc, time::Duration};

use benchmarks::{
    deterministic_payload, ScenarioId, ScenarioManifest as Manifest, SizeSequence,
    WorkloadProfile as Profile,
};
use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, EngineCommand, EstablishLink,
    LinkClosedReason, RatchetPolicy, RequestResponseTimeout, SendSinglePacket,
    SendSinglePacketPayload, SendToLink, SendToLinkFailure, SendToLinkPayload, Settlement,
};
use personal_rns::interfaces::{
    tcp, ConfiguredInterfacePolicy, EffectiveInterfacePolicy, InterfaceDescriptor, InterfaceId,
    InterfaceKind, MtuPolicy, ReportsStatus,
};
use personal_rns::reactor::interface_seam::{Interface, InterfaceSeam};
use personal_rns::reactor::reconnect::ReconnectPolicy;
use personal_rns::routes;
use personal_rns::routing::delivery::Delivery;
use personal_rns::routing::links::resources::{ResourceStrategy, MAX_EFFICIENT_SIZE};
use personal_rns::routing::links::LinkId;
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::request_router::{
    Decline, RequestContext, RequestRoute, RoutePolicy, RouteSet,
};
use personal_rns::runtime::{
    generate_identity_secret, Diagnostic, Message, PreConfiguredDestination, PrnsEvent, PrnsNode,
    PrnsNodeHandle, PrnsNodeRecipe, RequestHandlerRegistration, SegmentCompression,
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

fn benchmark_tcp_policy(profile: &Profile) -> EffectiveInterfacePolicy {
    tcp::configured_policy(ConfiguredInterfacePolicy {
        bitrate: Some(tcp::TCP_BITRATE_ESTIMATE),
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
        max_uncompressed_len: 128 * 1024 * 1024,
        accept_compressed: matches!(
            segment_compression(profile),
            SegmentCompression::Attempt { .. }
        ),
    }
}

enum Event {
    Heard(DestinationHash),
    Settled(CommandId, Settlement),
    Delivered(usize),
    LinkUp,
    ResourceIn { link_id: LinkId, bytes: usize },
    ResourceAck(u64),
    Closed,
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

    match manifest.name {
        ScenarioId::SinglePacketThroughput | ScenarioId::LinkMessageThroughput => {
            run_runtime_endpoint(&manifest, &role, &addr, duration).await;
        }
        ScenarioId::RequestResponse => {
            run_request_endpoint(&manifest, &role, &addr, duration).await;
        }
        ScenarioId::ResourceMaxSegment | ScenarioId::Resource64mibStream => {
            run_resource_endpoint(&manifest, &role, &addr, duration).await;
        }
    }
}

use link_channel::run_runtime_endpoint;
use request::run_request_endpoint;
use resource::run_resource_endpoint;

async fn build_responder_node<St, R, F>(
    single: PreConfiguredDestination<'static>,
    app_state: St,
    routes: R,
    on_event: F,
    manifest: &Manifest,
    addr: &str,
) -> (PrnsNode<St, R, F, NodeStorage>, String)
where
    R: RouteSet<St>,
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
        routes,
        on_event,
        interfaces: |node: &PrnsNodeHandle| {
            for server in servers {
                node.add_interface(server);
            }
        },
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
        routes: routes![],
        on_event,
        interfaces: |node: &PrnsNodeHandle| {
            node.attach(client);
        },
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
    use super::{parse_resource_ack, percentile_f64, resource_ack_payload};

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
}
