//! One implementation's *participation binary* for live scenarios: the whole cross-impl
//! contract is `scenario_node <manifest.json> <role> <addr> [duration-ms]` plus a line
//! protocol on stdout (`READY …`, then one final `RESULT k=v …`). The responder binds
//! `addr` (`127.0.0.1:0` lets the OS pick — the bound address comes back on its READY
//! line) and proves every delivery; the initiator connects and pumps windowed SINGLE
//! packets at the announced destination until the profile's wall-time elapses —
//! throughput from the settlement counts, latency straight from the proofs (`rtt_ms`).
//! Another implementation joins a pairing by speaking this same surface, nothing more.

use std::sync::atomic::{AtomicU64, Ordering};
use std::{sync::Arc, time::Duration};

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, EngineCommand, EngineState,
    EstablishLink, IssuedCommand, Journaled, RatchetPolicy, SendRequest, SendRequestData,
    SendSinglePacket, SendSinglePacketPayload, SendToChannel, SendToChannelBody,
    SendToChannelFailure, SendToLink, SendToLinkPayload, Settlement,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::tcp::core as tcp_core;
use personal_rns::interfaces::udp::core as udp_core;
use personal_rns::interfaces::{
    BitrateBps, InterfaceDescriptor, InterfaceId, InterfaceKind, ReportsStatus,
};
use personal_rns::reactor::impls::tokio_reactor::{
    run, tokio_grant_lane, AddInterfaceCommand, Egress, HostCommand, ReactorWiring, TokioHost,
    TokioInterfaceSeam,
};
use personal_rns::reactor::interface_seam::{Interface, InterfaceSeam, MAX_WIRE_FRAME_LEN};
use personal_rns::routes;
use personal_rns::routing::delivery::Delivery;
use personal_rns::routing::links::channel::MessageType;
use personal_rns::routing::links::resources::{ResourceStrategy, MAX_EFFICIENT_SIZE};
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::request_router::{
    Decline, RequestContext, RequestRoute, RoutePolicy, RouteSet,
};
use personal_rns::runtime::{
    generate_identity_secret, Diagnostic, Manual, Message, PreConfiguredDestination, Prns,
    PrnsEvent, PrnsRecipe, SegmentCompression, TokioPrnsHandle,
};
use personal_rns::shared_instance::{
    join_shared_instance, InstancePorts, OnExisting, RnsLocalBlackholeFile, Role,
    SharedInstanceCredentials, SharedInstanceIntent,
};
#[cfg(feature = "fixed-storage")]
use personal_rns::storage::Esp32S3 as NodeStorage;
#[cfg(not(feature = "fixed-storage"))]
use personal_rns::storage::GrowableHeap as NodeStorage;
use personal_rns::tcp::client::TcpClientInterface;
use personal_rns::tcp::server::TcpServerConnection;
use personal_rns::tcp::tokio_socket::tune;
use personal_rns::udp::UdpInterface;
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

/// A point-to-point TCP listener with a fixed interface id: binds a port, accepts one client, and
/// serves that connection as a single engine interface (the reference's per-connection TCP child),
/// delegating framing to a [`TcpServerConnection`]. The fleet-wide `TcpServer` supervisor is the
/// production multi-client shape; a one-shot pairing is point-to-point, keyed on the fixed id.
struct BenchTcpListener {
    id: InterfaceId,
    listener: tokio::net::TcpListener,
    bitrate: BitrateBps,
}

impl BenchTcpListener {
    async fn bind_with_id(
        id: InterfaceId,
        addr: impl tokio::net::ToSocketAddrs,
        bitrate: BitrateBps,
    ) -> std::io::Result<Self> {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        Ok(Self {
            id,
            listener,
            bitrate,
        })
    }

    fn local_addr(&self) -> std::io::Result<std::net::SocketAddr> {
        self.listener.local_addr()
    }
}

impl Interface for BenchTcpListener {
    const HW_MTU: usize = tcp_core::TCP_HW_MTU_CAP;
    const KIND: InterfaceKind = InterfaceKind::TcpServerPeer;

    fn descriptor(&self) -> InterfaceDescriptor {
        tcp_core::descriptor(self.id, self.bitrate)
    }

    fn channel_tag(&self) -> &[u8] {
        self.id.as_bytes()
    }

    async fn run<Seam: InterfaceSeam>(self, seam: Seam) {
        let Ok((stream, peer)) = self.listener.accept().await else {
            return;
        };
        tune(&stream);
        TcpServerConnection::new(peer.to_string().into_bytes(), stream, self.bitrate)
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
const LANE_DEPTH: usize = 64;
const DRAIN_GRACE: Duration = Duration::from_secs(5);
const QUIET_AFTER_TRAFFIC: Duration = Duration::from_millis(1500);
const BENCH_CHANNEL_MSGTYPE: MessageType = MessageType(0x0042);

#[derive(serde::Deserialize)]
struct Manifest {
    name: String,
    profile: Profile,
}

#[derive(serde::Deserialize)]
struct Profile {
    mechanism: String,
    #[serde(default = "default_wire")]
    wire: String,
    #[serde(default)]
    payload_len: usize,
    #[serde(default)]
    payload_min: usize,
    #[serde(default)]
    payload_max: usize,
    #[serde(default)]
    request_min: usize,
    #[serde(default)]
    request_max: usize,
    #[serde(default)]
    response_min: usize,
    #[serde(default)]
    response_max: usize,
    window: usize,
    duration_ms: u64,
    #[serde(default = "default_announce_every_ms")]
    announce_every_ms: u64,
    #[serde(default = "default_initiator_count")]
    initiator_count: usize,
    #[serde(default = "default_link_count")]
    link_count: usize,
    #[serde(default = "default_size_seed")]
    size_seed: u64,
    #[serde(default = "default_compression")]
    compression: String,
    #[serde(default = "default_payload_shape")]
    payload_shape: String,
    #[serde(default = "default_topology")]
    topology: String,
    #[serde(default)]
    tunnel: bool,
    #[serde(default)]
    reconnect_at_ms: u64,
    #[serde(default)]
    command_share: usize,
    #[serde(default)]
    command_min: usize,
    #[serde(default)]
    command_max: usize,
    #[serde(default)]
    page_share: usize,
    #[serde(default)]
    page_min: usize,
    #[serde(default)]
    page_max: usize,
    #[serde(default)]
    file_min: usize,
    #[serde(default)]
    file_max: usize,
}

/// One churn cycle's traffic: a band rolled from the shared sequence, then a
/// size within it — command messages ride a single link send, pages and
/// files ride a resource.
#[derive(Clone, Copy, PartialEq)]
enum Band {
    Command,
    Page,
    File,
}

fn roll_band(sizes: &mut SizeSequence, profile: &Profile) -> (Band, usize) {
    let roll = sizes.next_in(0, 99);
    if roll < profile.command_share {
        (
            Band::Command,
            sizes.next_in(profile.command_min, profile.command_max),
        )
    } else if roll < profile.command_share + profile.page_share {
        (
            Band::Page,
            sizes.next_in(profile.page_min, profile.page_max),
        )
    } else {
        (
            Band::File,
            sizes.next_in(profile.file_min, profile.file_max),
        )
    }
}

fn default_topology() -> String {
    "direct".into()
}

fn default_size_seed() -> u64 {
    0x5EED_CAFE_F00D_0001
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

fn default_compression() -> String {
    "off".into()
}

fn default_payload_shape() -> String {
    "dense".into()
}

/// The responder mirrors the row's compression posture: only an "auto" row's receiver takes
/// compressed segments.
fn responder_resource_strategy(profile: &Profile) -> ResourceStrategy {
    ResourceStrategy::Accept {
        max_uncompressed_len: 128 * 1024 * 1024,
        accept_compressed: matches!(
            segment_compression(profile),
            SegmentCompression::Attempt { .. }
        ),
    }
}

fn default_announce_every_ms() -> u64 {
    500
}

fn default_initiator_count() -> usize {
    1
}

fn default_link_count() -> usize {
    256
}

/// The varied-size law every node speaks identically: a seeded xorshift draws
/// each message's size in `[min, max]`, so both ends — and both
/// implementations — agree on every byte total without exchanging anything.
struct SizeSequence {
    state: u64,
    min: usize,
    max: usize,
}

impl SizeSequence {
    fn new(seed: u64, min: usize, max: usize, fixed: usize) -> Self {
        let (min, max) = if max > 0 { (min, max) } else { (fixed, fixed) };
        Self {
            state: seed,
            min,
            max,
        }
    }

    fn next_len(&mut self) -> usize {
        self.next_in(self.min, self.max)
    }

    fn next_in(&mut self, min: usize, max: usize) -> usize {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        let span = (max - min + 1) as u64;
        min + (self.state % span) as usize
    }
}

fn default_wire() -> String {
    "tcp".into()
}

/// A UDP scenario's `addr` is the full fixed pairing, `local>peer` — datagrams have no
/// connect, so the orchestrator pre-assigns both ends and each node binds its own half.
fn udp_halves(addr: &str) -> (&str, &str) {
    addr.split_once('>')
        .expect("a udp addr is local>peer, both pre-assigned by the orchestrator")
}

enum Event {
    Heard(DestinationHash),
    Settled(CommandId, Settlement),
    Delivered(usize),
    LinkUp,
    ResourceIn(usize),
    Response(usize),
    Closed,
}

const REQUEST_PATH: &str = "/bench/query";

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

/// Deterministic pseudo-random bytes: bz2 gains nothing, so both ends'
/// keep-only-if-smaller rules keep the full stream on the wire — the bulk
/// measurement measures bulk.
fn incompressible_payload(len: usize) -> Vec<u8> {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut data = Vec::with_capacity(len);
    while data.len() < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        data.extend_from_slice(&state.to_le_bytes());
    }
    data.truncate(len);
    data
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

/// The manifest's payload shape for resource sends: "dense" declines every
/// compression attempt, "compressible" engages the codec on the wire.
fn scenario_payload(profile: &Profile, len: usize) -> Vec<u8> {
    match profile.payload_shape.as_str() {
        "dense" => incompressible_payload(len),
        "compressible" => compressible_payload(len),
        other => panic!("unknown payload shape {other:?} (expected \"dense\" or \"compressible\")"),
    }
}

fn percentile(sorted: &[u64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let rank = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[rank.min(sorted.len() - 1)] as f64
}

/// The engine is constructed by value, so it lands on the stack before it is boxed into storage — a
/// frame that fits a release build's 8 MiB but overflows it unoptimized, where every local is
/// spilled. The node (its construction, the reactor, the interface drivers) is driven on one thread,
/// so that thread carries the frame; a generous stack is what lets a debug build run at all — the
/// conformance pass a debug build is for.
const SCENARIO_STACK_BYTES: usize = 64 * 1024 * 1024;

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static DHAT_ALLOC: dhat::Alloc = dhat::Alloc;

fn main() {
    if cfg!(debug_assertions) {
        eprintln!("================================================================");
        eprintln!("scenario_node is a DEBUG build: crypto runs ~10x slower than release.");
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
    let usage = "usage: scenario_node <manifest.json> <responder|initiator> <addr> [duration-ms]";
    let manifest_path = args.next().expect(usage);
    let role = args.next().expect(usage);
    let addr = args.next().expect(usage);
    let duration_override: Option<u64> = args.next().map(|s| s.parse().expect("duration-ms"));

    let manifest: Manifest =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).expect("read manifest"))
            .expect("parse manifest");
    let duration = Duration::from_millis(duration_override.unwrap_or(manifest.profile.duration_ms));

    // Relay and chain are pure-transport topologies with no destination of their own — the
    // recipe does not model them, so they keep their own hand-roll.
    if role == "relay" {
        if manifest.profile.tunnel {
            tunnel_relay_node(&manifest).await;
        } else {
            relay_node(&manifest).await;
        }
        return;
    }
    if role == "chain" {
        chain_node(&addr).await;
        return;
    }
    // Every contestant endpoint rides the high-level runtime: single/link/channel/links-breadth here,
    // resource/request/churn below (resource opens its accept gate through the recipe's
    // `resource_strategy`, request serves through a `routes!` handler). The only path still hand-rolled
    // is the tunnel-route-survival probe — harness instrumentation, not a drop-your-binary contestant.
    if matches!(
        manifest.profile.mechanism.as_str(),
        "single" | "link" | "channel" | "links-breadth" | "link-storm"
    ) && (!manifest.profile.tunnel || role == "responder")
    {
        run_runtime_endpoint(&manifest, &role, &addr, duration).await;
        return;
    }
    if manifest.profile.mechanism == "resource" && shared_instance_port().is_none() {
        run_resource_endpoint(&manifest, &role, &addr, duration).await;
        return;
    }
    if manifest.profile.mechanism == "request" && shared_instance_port().is_none() {
        run_request_endpoint(&manifest, &role, &addr, duration).await;
        return;
    }
    if manifest.profile.mechanism == "churn" && shared_instance_port().is_none() {
        run_churn_endpoint(&manifest, &role, &addr, duration).await;
        return;
    }
    if let Some(port) = shared_instance_port() {
        if manifest.profile.mechanism == "request" {
            run_request_bus_client(&manifest, &role, duration, port).await;
            return;
        }
        if manifest.profile.mechanism == "churn" {
            run_churn_bus_client(&manifest, &role, duration, port).await;
            return;
        }
        if manifest.profile.mechanism == "resource" {
            run_resource_bus_client(&manifest, &role, duration, port).await;
            return;
        }
        if manifest.profile.mechanism == "resource-fanout" {
            run_resource_fanout_bus_client(&manifest, &role, duration, port).await;
            return;
        }
    }

    if manifest.profile.mechanism == "single" && role == "initiator" {
        run_tunnel_probe(&manifest, &addr, duration).await;
    } else {
        panic!(
            "mechanism {:?} role {:?} has no orchestrate endpoint",
            manifest.profile.mechanism, role
        );
    }
}

/// The single-, link-, and channel-firehose endpoints stood up through the high-level runtime:
/// a [`PrnsRecipe`] with one Single destination and its wires, built by [`Prns::new`]. This end
/// keeps only what is genuinely the app's: the destination address (to announce itself), the
/// command handle, and the event stream. `Prns::run` owns the reactor and is `!Send`, so it is
/// driven on this task in a `select!` against the role's own firehose loop (spoken to through
/// the cloned [`TokioPrnsHandle`]). `Prns::new` stands the engine up on `GrowableHeap`; the
/// `fixed-storage` residence is not yet a `Prns` knob, so firehose endpoints measure heap storage.
async fn run_runtime_endpoint(manifest: &Manifest, role: &str, addr: &str, duration: Duration) {
    let mechanism = manifest.profile.mechanism.as_str();
    let announce_every = Duration::from_millis(manifest.profile.announce_every_ms);
    let initiators = manifest.profile.initiator_count;

    // The recipe borrows its destination names for the node's whole life, and the node lives as
    // long as its `run` loop is driven, so the manifest-derived aspect is promoted to 'static.
    let aspect: &'static str = Box::leak(manifest.name.clone().into_boxed_str());
    let aspects: &'static [&'static str] = Box::leak(Box::new([aspect]));
    let identity_secret = generate_identity_secret();
    let transport_identity =
        (manifest.profile.tunnel && role == "responder").then(|| identity_secret.clone());
    let single = PreConfiguredDestination::Single {
        app_name: "bench",
        aspects,
        identity: identity_secret,
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        resource_strategy: ResourceStrategy::AcceptNone,
    };
    let destination = single
        .destination_hash()
        .expect("the bench destination name is valid");

    let (event_tx, event_rx) = mpsc::unbounded_channel::<Event>();
    let on_event = move |event: PrnsEvent<'_>, _state: &()| {
        let mapped = match event {
            PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) => {
                Some(Event::Heard(destination))
            }
            PrnsEvent::Diagnostic(Diagnostic::CommandSettled { id, settlement }) => {
                Some(Event::Settled(id, settlement))
            }
            PrnsEvent::Diagnostic(Diagnostic::LinkEstablished(_)) => Some(Event::LinkUp),
            PrnsEvent::Diagnostic(Diagnostic::LinkClosed { .. }) => Some(Event::Closed),
            PrnsEvent::Message(Message::Delivered(Delivery::Single(delivery))) => {
                Some(Event::Delivered(delivery.plaintext.len()))
            }
            PrnsEvent::Message(Message::Delivered(Delivery::Link(delivery))) => {
                Some(Event::Delivered(delivery.plaintext.len()))
            }
            PrnsEvent::Message(Message::ChannelMessage { data, .. }) => {
                Some(Event::Delivered(data.len()))
            }
            _ => None,
        };
        if let Some(event) = mapped {
            let _ = event_tx.send(event);
        }
    };

    let shared_port = shared_instance_port();
    if role == "responder" {
        let (node, bound) = match shared_port {
            Some(_) => (
                build_bus_client_node(single, on_event),
                "shared".to_string(),
            ),
            None => {
                build_responder_node(
                    single,
                    (),
                    routes![],
                    on_event,
                    manifest,
                    addr,
                    transport_identity,
                )
                .await
            }
        };
        let commands = node.handle();
        if let Some(port) = shared_port {
            join_bus(&commands, port).await;
        }
        println!("READY role=responder addr={bound}");
        let expected_links = if mechanism == "links-breadth" {
            manifest.profile.link_count
        } else {
            initiators
        };
        let firehose = async {
            if matches!(mechanism, "link" | "channel" | "links-breadth") {
                respond_link(
                    destination,
                    announce_every,
                    expected_links,
                    &commands,
                    event_rx,
                )
                .await;
            } else {
                respond(destination, announce_every, duration, &commands, event_rx).await;
            }
        };
        tokio::select! {
            () = node.run() => unreachable!("the responder's run loop returned"),
            () = firehose => {}
        }
    } else if role == "initiator" {
        let node = match shared_port {
            Some(_) => build_bus_client_node(single, on_event),
            None => build_initiator_node(single, on_event, manifest, addr).await,
        };
        let commands = node.handle();
        if let Some(port) = shared_port {
            join_bus(&commands, port).await;
        }
        println!("READY role=initiator");
        let firehose = async {
            if mechanism == "link" {
                initiate_link(&manifest.profile, duration, &commands, event_rx).await;
            } else if mechanism == "links-breadth" {
                initiate_links_breadth(&manifest.profile, duration, &commands, event_rx).await;
            } else if mechanism == "link-storm" {
                initiate_link_storm(&manifest.profile, duration, &commands, event_rx).await;
            } else if mechanism == "channel" {
                initiate_channel(&manifest.profile, duration, &commands, event_rx).await;
            } else {
                initiate(&manifest.profile, duration, &commands, event_rx).await;
            }
            // Close settlement is engine-state, not wire-state: give the egress lane a beat to
            // flush the close frame, or the responder only learns via its 10s stale reaper.
            tokio::time::sleep(Duration::from_millis(200)).await;
        };
        tokio::select! {
            () = node.run() => unreachable!("the initiator's run loop returned"),
            () = firehose => {}
        }
    } else {
        panic!("unknown role {role:?}");
    }
}

fn shared_instance_port() -> Option<u16> {
    std::env::var("MATCHUP_SHARED_PORT")
        .ok()
        .and_then(|raw| raw.parse().ok())
}

fn build_bus_client_node<F>(
    single: PreConfiguredDestination<'static>,
    on_event: F,
) -> Prns<(), (), F, NodeStorage>
where
    F: FnMut(PrnsEvent<'_>, &()),
{
    Prns::new(PrnsRecipe {
        transport_identity: None,
        pre_configured_destinations: [single],
        app_state: (),
        storage: NodeStorage::default(),
        routes: routes![],
        on_event,
        interfaces: Manual,
    })
}

async fn join_bus(commands: &TokioPrnsHandle, port: u16) {
    let role = join_shared_instance(
        commands,
        SharedInstanceIntent {
            credentials: SharedInstanceCredentials::from_identity_secret(
                &[0xA2; personal_rns::identity::IDENTITY_SECRET_KEY_LEN],
            ),
            blackhole_file: RnsLocalBlackholeFile::new(
                std::env::temp_dir().join(std::format!("prns-scenario-{port}-blackhole")),
            ),
            ports: InstancePorts {
                bus: port,
                control: port + 1,
            },
            on_existing: OnExisting::JoinAsClient,
        },
    )
    .await
    .expect("join the shared-instance bus");
    assert!(
        matches!(role, Role::JoinedAsClient { .. }),
        "expected to join a running host as a client, got {role:?}"
    );
}

struct RequestServed(Arc<AtomicU64>);

struct BenchRequestRoute;

impl RequestRoute<RequestServed> for BenchRequestRoute {
    const PATH: &'static str = REQUEST_PATH;
    const POLICY: RoutePolicy = RoutePolicy::AllowAll;
    async fn handle(mut cx: RequestContext<'_, RequestServed>) -> Result<(), Decline> {
        cx.state.0.fetch_add(1, Ordering::Relaxed);
        cx.respond(b"pong")
    }
}

async fn run_request_bus_client(manifest: &Manifest, role: &str, duration: Duration, port: u16) {
    let aspect: &'static str = Box::leak(manifest.name.clone().into_boxed_str());
    let aspects: &'static [&'static str] = Box::leak(Box::new([aspect]));
    let announce_every = Duration::from_millis(manifest.profile.announce_every_ms);
    let single = PreConfiguredDestination::Single {
        app_name: "bench",
        aspects,
        identity: generate_identity_secret(),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        resource_strategy: ResourceStrategy::AcceptNone,
    };
    if role == "responder" {
        let served = Arc::new(AtomicU64::new(0));
        let destination = single.destination_hash().expect("valid bench destination");
        let node = Prns::new(PrnsRecipe {
            transport_identity: None,
            pre_configured_destinations: [single],
            app_state: RequestServed(Arc::clone(&served)),
            storage: NodeStorage::default(),
            routes: routes![BenchRequestRoute],
            on_event: |_event, _state| {},
            interfaces: Manual,
        });
        let commands = node.handle();
        join_bus(&commands, port).await;
        println!("READY role=responder addr=shared");
        let announcer = commands.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(announce_every);
            loop {
                ticker.tick().await;
                if announcer
                    .issue(EngineCommand::AnnounceNow(AnnounceNow {
                        destination,
                        target: AnnounceTarget::AllInterfaces,
                        app_data: AnnounceAppData::Registered,
                    }))
                    .is_none()
                {
                    break;
                }
            }
        });
        let report = async {
            tokio::time::sleep(duration + DRAIN_GRACE).await;
            let served = served.load(Ordering::Relaxed);
            println!("RESULT served={served} response_bytes={}", served * 4);
        };
        tokio::select! {
            () = node.run() => unreachable!("the responder's run loop returned"),
            () = report => {}
        }
    } else {
        let (heard_tx, mut heard_rx) = mpsc::unbounded_channel::<DestinationHash>();
        let node = build_bus_client_node(single, move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ = heard_tx.send(destination);
            }
        });
        let commands = node.handle();
        join_bus(&commands, port).await;
        println!("READY role=initiator");
        let firehose = async {
            let destination = heard_rx.recv().await.expect("hears the responder");
            let link_id = commands
                .establish_link(destination)
                .await
                .expect("link establishes");
            let path_hash = RequestPathHash::of(REQUEST_PATH);
            let started = tokio::time::Instant::now();
            let deadline = started + duration;
            let mut sent = 0u64;
            let mut delivered = 0u64;
            let mut timeouts = 0u64;
            let mut rtts: Vec<u64> = Vec::new();
            while tokio::time::Instant::now() < deadline {
                sent += 1;
                match commands.request(link_id, path_hash, b"ping").await {
                    Ok((_response, rtt)) => {
                        delivered += 1;
                        rtts.push(rtt.millis());
                    }
                    Err(_) => timeouts += 1,
                }
            }
            let elapsed_ms = started.elapsed().as_millis().max(1) as u64;
            rtts.sort_unstable();
            let per_sec = sent * 1000 / elapsed_ms;
            println!(
                "RESULT sent={sent} delivered={delivered} timeouts={timeouts} \
                 elapsed_ms={elapsed_ms} requests_per_sec={per_sec} \
                 rtt_p50_ms={:.0} rtt_p99_ms={:.0} build={BUILD_PROFILE}",
                percentile(&rtts, 0.50),
                percentile(&rtts, 0.99),
            );
        };
        tokio::select! {
            () = node.run() => unreachable!("the initiator's run loop returned"),
            () = firehose => {}
        }
    }
}

async fn run_churn_bus_client(manifest: &Manifest, role: &str, duration: Duration, port: u16) {
    let aspect: &'static str = Box::leak(manifest.name.clone().into_boxed_str());
    let aspects: &'static [&'static str] = Box::leak(Box::new([aspect]));
    let announce_every = Duration::from_millis(manifest.profile.announce_every_ms);
    let single = PreConfiguredDestination::Single {
        app_name: "bench",
        aspects,
        identity: generate_identity_secret(),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        resource_strategy: ResourceStrategy::AcceptNone,
    };
    if role == "responder" {
        let links = Arc::new(AtomicU64::new(0));
        let destination = single.destination_hash().expect("valid bench destination");
        let links_seen = Arc::clone(&links);
        let node = build_bus_client_node(single, move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::LinkEstablished(_)) = event {
                links_seen.fetch_add(1, Ordering::Relaxed);
            }
        });
        let commands = node.handle();
        join_bus(&commands, port).await;
        println!("READY role=responder addr=shared");
        let announcer = commands.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(announce_every);
            loop {
                ticker.tick().await;
                if announcer
                    .issue(EngineCommand::AnnounceNow(AnnounceNow {
                        destination,
                        target: AnnounceTarget::AllInterfaces,
                        app_data: AnnounceAppData::Registered,
                    }))
                    .is_none()
                {
                    break;
                }
            }
        });
        let report = async {
            tokio::time::sleep(duration + DRAIN_GRACE).await;
            println!("RESULT received={}", links.load(Ordering::Relaxed));
        };
        tokio::select! {
            () = node.run() => unreachable!("the responder's run loop returned"),
            () = report => {}
        }
    } else {
        let (heard_tx, mut heard_rx) = mpsc::unbounded_channel::<DestinationHash>();
        let node = build_bus_client_node(single, move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ = heard_tx.send(destination);
            }
        });
        let commands = node.handle();
        join_bus(&commands, port).await;
        println!("READY role=initiator");
        let firehose = async {
            let destination = heard_rx.recv().await.expect("hears the responder");
            let started = tokio::time::Instant::now();
            let deadline = started + duration;
            let mut cycles = 0u64;
            let mut failures = 0u64;
            let mut establish_ms: Vec<u64> = Vec::new();
            while tokio::time::Instant::now() < deadline {
                let cycle_started = tokio::time::Instant::now();
                match commands.establish_link(destination).await {
                    Ok(link_id) => {
                        establish_ms.push(cycle_started.elapsed().as_millis() as u64);
                        commands.close_link(link_id);
                        cycles += 1;
                    }
                    Err(_) => failures += 1,
                }
            }
            let elapsed_ms = started.elapsed().as_millis().max(1) as u64;
            establish_ms.sort_unstable();
            let attempts = cycles + failures;
            let per_sec = cycles * 1000 / elapsed_ms;
            println!(
                "RESULT sent={attempts} delivered={cycles} timeouts={failures} cycles={cycles} \
                 failures={failures} elapsed_ms={elapsed_ms} cycles_per_sec={per_sec} \
                 establish_p50_ms={:.0} establish_p99_ms={:.0} build={BUILD_PROFILE}",
                percentile(&establish_ms, 0.50),
                percentile(&establish_ms, 0.99),
            );
        };
        tokio::select! {
            () = node.run() => unreachable!("the initiator's run loop returned"),
            () = firehose => {}
        }
    }
}

async fn run_resource_bus_client(manifest: &Manifest, role: &str, duration: Duration, port: u16) {
    let aspect: &'static str = Box::leak(manifest.name.clone().into_boxed_str());
    let aspects: &'static [&'static str] = Box::leak(Box::new([aspect]));
    let announce_every = Duration::from_millis(manifest.profile.announce_every_ms);
    let single = PreConfiguredDestination::Single {
        app_name: "bench",
        aspects,
        identity: generate_identity_secret(),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        resource_strategy: ResourceStrategy::AcceptNone,
    };
    if role == "responder" {
        run_resource_responder(
            single,
            responder_resource_strategy(&manifest.profile),
            port,
            announce_every,
            duration,
        )
        .await;
    } else {
        let (heard_tx, mut heard_rx) = mpsc::unbounded_channel::<DestinationHash>();
        let node = build_bus_client_node(single, move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ = heard_tx.send(destination);
            }
        });
        let commands = node.handle();
        join_bus(&commands, port).await;
        println!("READY role=initiator");
        let firehose = async {
            let destination = heard_rx.recv().await.expect("hears the responder");
            let link_id = commands
                .establish_link(destination)
                .await
                .expect("link establishes");
            let scratch = scenario_payload(
                &manifest.profile,
                manifest
                    .profile
                    .payload_max
                    .max(manifest.profile.payload_len),
            );
            let mut sizes = SizeSequence::new(
                manifest.profile.size_seed,
                manifest.profile.payload_min,
                manifest.profile.payload_max,
                manifest.profile.payload_len,
            );
            let compression = segment_compression(&manifest.profile);
            let started = tokio::time::Instant::now();
            let deadline = started + duration;
            let mut sent = 0u64;
            let mut settled = 0u64;
            let mut failures = 0u64;
            let mut payload_bytes = 0u64;
            let mut transfer_ms: Vec<u64> = Vec::new();
            while tokio::time::Instant::now() < deadline {
                let len = sizes.next_len();
                sent += 1;
                let transfer_started = tokio::time::Instant::now();
                match commands
                    .send_resource_with_compression(
                        link_id,
                        len as u64,
                        &scratch[..len],
                        compression,
                    )
                    .await
                {
                    Ok(()) => {
                        settled += 1;
                        payload_bytes += len as u64;
                        transfer_ms.push(transfer_started.elapsed().as_millis() as u64);
                    }
                    Err(_) => failures += 1,
                }
            }
            let elapsed_ms = started.elapsed().as_millis().max(1) as u64;
            transfer_ms.sort_unstable();
            let seconds = (elapsed_ms as f64 / 1000.0).max(f64::EPSILON);
            println!(
                "RESULT sent={sent} settled={settled} failures={failures} \
                 payload_bytes={payload_bytes} elapsed_ms={elapsed_ms} \
                 goodput_bytes_per_sec={:.0} goodput_mbits_per_sec={:.2} \
                 transfer_p50_ms={:.0} transfer_p99_ms={:.0} build={BUILD_PROFILE}",
                payload_bytes as f64 / seconds,
                payload_bytes as f64 * 8.0 / seconds / 1_000_000.0,
                percentile(&transfer_ms, 0.50),
                percentile(&transfer_ms, 0.99),
            );
        };
        tokio::select! {
            () = node.run() => unreachable!("the initiator's run loop returned"),
            () = firehose => {}
        }
    }
}

/// The accepting end shared by the resource and resource-fanout mechanisms: a bus client that
/// accepts inbound resources on its destination and tallies every concluded transfer, however many
/// links they ride. Counting at the destination, not per link, is what lets the fanout responder be
/// the same code as the single-link one.
async fn run_resource_responder(
    single: PreConfiguredDestination<'static>,
    resource_strategy: ResourceStrategy,
    port: u16,
    announce_every: Duration,
    duration: Duration,
) {
    let received = Arc::new(AtomicU64::new(0));
    let bytes = Arc::new(AtomicU64::new(0));
    let destination = single.destination_hash().expect("valid bench destination");
    let received_cb = Arc::clone(&received);
    let bytes_cb = Arc::clone(&bytes);
    let node = build_bus_client_node(single, move |event, _state| {
        if let PrnsEvent::Message(Message::Resource { data, .. }) = event {
            received_cb.fetch_add(1, Ordering::Relaxed);
            bytes_cb.fetch_add(data.len() as u64, Ordering::Relaxed);
        }
    });
    let commands = node.handle();
    join_bus(&commands, port).await;
    let report = async {
        commands
            .set_resource_strategy(destination, resource_strategy)
            .await;
        println!("READY role=responder addr=shared");
        let announcer = commands.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(announce_every);
            loop {
                ticker.tick().await;
                if announcer
                    .issue(EngineCommand::AnnounceNow(AnnounceNow {
                        destination,
                        target: AnnounceTarget::AllInterfaces,
                        app_data: AnnounceAppData::Registered,
                    }))
                    .is_none()
                {
                    break;
                }
            }
        });
        tokio::time::sleep(duration + DRAIN_GRACE).await;
        println!(
            "RESULT received={} payload_bytes={}",
            received.load(Ordering::Relaxed),
            bytes.load(Ordering::Relaxed)
        );
    };
    tokio::select! {
        () = node.run() => unreachable!("the responder's run loop returned"),
        () = report => {}
    }
}

/// The resource throughput firehose spread across links: establish `link_count` links to one
/// responder, then run one resource transfer in flight per link concurrently for the wall-time, and
/// report the aggregate goodput. Where resource-transfer is one link's window-bound stream, this
/// asks whether more links pipeline more goodput, and stresses the per-transfer resource state at
/// breadth. Establishing all N proves the table held them; a link that completes no transfer trips
/// the per-link check.
async fn run_resource_fanout_bus_client(
    manifest: &Manifest,
    role: &str,
    duration: Duration,
    port: u16,
) {
    let aspect: &'static str = Box::leak(manifest.name.clone().into_boxed_str());
    let aspects: &'static [&'static str] = Box::leak(Box::new([aspect]));
    let announce_every = Duration::from_millis(manifest.profile.announce_every_ms);
    let single = PreConfiguredDestination::Single {
        app_name: "bench",
        aspects,
        identity: generate_identity_secret(),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        resource_strategy: ResourceStrategy::AcceptNone,
    };
    if role == "responder" {
        run_resource_responder(
            single,
            responder_resource_strategy(&manifest.profile),
            port,
            announce_every,
            duration,
        )
        .await;
        return;
    }

    let (heard_tx, mut heard_rx) = mpsc::unbounded_channel::<DestinationHash>();
    let node = build_bus_client_node(single, move |event, _state| {
        if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
            let _ = heard_tx.send(destination);
        }
    });
    let commands = node.handle();
    join_bus(&commands, port).await;
    println!("READY role=initiator");
    let link_count = manifest.profile.link_count;
    let payload_min = manifest.profile.payload_min;
    let payload_max = manifest
        .profile
        .payload_max
        .max(manifest.profile.payload_len);
    let payload_len = manifest.profile.payload_len;
    let size_seed = manifest.profile.size_seed;
    let compression = segment_compression(&manifest.profile);
    let firehose = async {
        let destination = heard_rx.recv().await.expect("hears the responder");
        let mut links = Vec::with_capacity(link_count);
        for _ in 0..link_count {
            links.push(
                commands
                    .establish_link(destination)
                    .await
                    .expect("link establishes"),
            );
        }
        let scratch: Arc<Vec<u8>> = Arc::new(scenario_payload(&manifest.profile, payload_max));
        let started = tokio::time::Instant::now();
        let deadline = started + duration;
        let mut set: tokio::task::JoinSet<(u64, u64, u64, u64)> = tokio::task::JoinSet::new();
        for (index, link_id) in links.iter().copied().enumerate() {
            let commands = commands.clone();
            let scratch = Arc::clone(&scratch);
            let mut sizes = SizeSequence::new(
                size_seed ^ index as u64,
                payload_min,
                payload_max,
                payload_len,
            );
            set.spawn(async move {
                let mut sent = 0u64;
                let mut settled = 0u64;
                let mut failures = 0u64;
                let mut bytes = 0u64;
                while tokio::time::Instant::now() < deadline {
                    let len = sizes.next_len();
                    sent += 1;
                    match commands
                        .send_resource_with_compression(
                            link_id,
                            len as u64,
                            &scratch[..len],
                            compression,
                        )
                        .await
                    {
                        Ok(()) => {
                            settled += 1;
                            bytes += len as u64;
                        }
                        Err(_) => failures += 1,
                    }
                }
                (sent, settled, failures, bytes)
            });
        }
        let mut total_sent = 0u64;
        let mut total_settled = 0u64;
        let mut total_failures = 0u64;
        let mut total_bytes = 0u64;
        let mut silent_links = 0usize;
        while let Some(joined) = set.join_next().await {
            let (sent, settled, failures, bytes) = joined.expect("a resource task panicked");
            total_sent += sent;
            total_settled += settled;
            total_failures += failures;
            total_bytes += bytes;
            if settled == 0 {
                silent_links += 1;
            }
        }
        let elapsed_ms = started.elapsed().as_millis().max(1) as u64;
        for link_id in links {
            commands.close_link(link_id);
        }
        assert!(
            silent_links == 0,
            "{silent_links} of {link_count} links completed no resource transfer",
        );
        let seconds = (elapsed_ms as f64 / 1000.0).max(f64::EPSILON);
        println!(
            "RESULT sent={total_sent} settled={total_settled} failures={total_failures} \
             links={link_count} payload_bytes={total_bytes} elapsed_ms={elapsed_ms} \
             goodput_bytes_per_sec={:.0} goodput_mbits_per_sec={:.2} build={BUILD_PROFILE}",
            total_bytes as f64 / seconds,
            total_bytes as f64 * 8.0 / seconds / 1_000_000.0,
        );
    };
    tokio::select! {
        () = node.run() => unreachable!("the initiator's run loop returned"),
        () = firehose => {}
    }
}

/// Build the responder's node: its listening wires fold straight into the recipe, and the bound
/// READY address line comes back beside it (the server address, plus fan-in listeners joined by
/// `+`). The interface kind differs per branch, but `Prns::new` erases it into one node type.
async fn build_responder_node<St, R, F>(
    single: PreConfiguredDestination<'static>,
    app_state: St,
    routes: R,
    on_event: F,
    manifest: &Manifest,
    addr: &str,
    transport_identity: Option<Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>>,
) -> (Prns<St, R, F, NodeStorage>, String)
where
    R: RouteSet<St>,
    F: FnMut(PrnsEvent<'_>, &St),
{
    if manifest.profile.topology == "relay" {
        let client = TcpClientInterface::new_with_id(
            TCP_INTERFACE_ID,
            addr.to_string(),
            tcp_core::TCP_BITRATE_ESTIMATE,
            Duration::from_millis(100),
        );
        let node = Prns::new(PrnsRecipe {
            transport_identity,
            pre_configured_destinations: [single],
            app_state,
            storage: NodeStorage::default(),
            routes,
            on_event,
            interfaces: |node: &TokioPrnsHandle| {
                node.attach(client);
            },
        });
        (node, addr.to_string())
    } else if manifest.profile.wire == "udp" {
        let (local, peer) = udp_halves(addr);
        let udp = UdpInterface::bind_with_id(
            TCP_INTERFACE_ID,
            local,
            peer,
            udp_core::UDP_BITRATE_ESTIMATE,
        )
        .await
        .expect("binds the scenario port");
        let node = Prns::new(PrnsRecipe {
            transport_identity,
            pre_configured_destinations: [single],
            app_state,
            storage: NodeStorage::default(),
            routes,
            on_event,
            interfaces: |node: &TokioPrnsHandle| {
                node.attach(udp);
            },
        });
        (node, addr.to_string())
    } else {
        let primary =
            BenchTcpListener::bind_with_id(TCP_INTERFACE_ID, addr, tcp_core::TCP_BITRATE_ESTIMATE)
                .await
                .expect("binds the scenario port");
        let mut addresses = primary.local_addr().expect("bound address").to_string();
        let mut servers = vec![primary];
        for index in 0..manifest.profile.initiator_count.saturating_sub(1) {
            let extra = BenchTcpListener::bind_with_id(
                fanin_listener_id(index),
                "127.0.0.1:0",
                tcp_core::TCP_BITRATE_ESTIMATE,
            )
            .await
            .expect("binds an extra listener");
            addresses.push('+');
            addresses.push_str(&extra.local_addr().expect("bound address").to_string());
            servers.push(extra);
        }
        let node = Prns::new(PrnsRecipe {
            transport_identity,
            pre_configured_destinations: [single],
            app_state,
            storage: NodeStorage::default(),
            routes,
            on_event,
            interfaces: |node: &TokioPrnsHandle| {
                for server in servers {
                    node.add_interface(server);
                }
            },
        });
        (node, addresses)
    }
}

/// Build the initiator's node: one dialing wire (a UDP half or a TCP client) folded into the recipe.
async fn build_initiator_node<F>(
    single: PreConfiguredDestination<'static>,
    on_event: F,
    manifest: &Manifest,
    addr: &str,
) -> Prns<(), (), F, NodeStorage>
where
    F: FnMut(PrnsEvent<'_>, &()),
{
    if manifest.profile.wire == "udp" {
        let (local, peer) = udp_halves(addr);
        let udp = UdpInterface::bind_with_id(
            TCP_INTERFACE_ID,
            local,
            peer,
            udp_core::UDP_BITRATE_ESTIMATE,
        )
        .await
        .expect("binds the scenario port");
        Prns::new(PrnsRecipe {
            transport_identity: None,
            pre_configured_destinations: [single],
            app_state: (),
            storage: NodeStorage::default(),
            routes: routes![],
            on_event,
            interfaces: |node: &TokioPrnsHandle| {
                node.attach(udp);
            },
        })
    } else {
        let client = TcpClientInterface::new_with_id(
            TCP_INTERFACE_ID,
            addr.to_string(),
            tcp_core::TCP_BITRATE_ESTIMATE,
            Duration::from_millis(100),
        );
        Prns::new(PrnsRecipe {
            transport_identity: None,
            pre_configured_destinations: [single],
            app_state: (),
            storage: NodeStorage::default(),
            routes: routes![],
            on_event,
            interfaces: |node: &TokioPrnsHandle| {
                node.attach(client);
            },
        })
    }
}

async fn run_resource_endpoint(manifest: &Manifest, role: &str, addr: &str, duration: Duration) {
    let aspect: &'static str = Box::leak(manifest.name.clone().into_boxed_str());
    let aspects: &'static [&'static str] = Box::leak(Box::new([aspect]));
    let announce_every = Duration::from_millis(manifest.profile.announce_every_ms);
    let initiators = manifest.profile.initiator_count;
    let resource_strategy = if role == "responder" {
        responder_resource_strategy(&manifest.profile)
    } else {
        ResourceStrategy::AcceptNone
    };
    let single = PreConfiguredDestination::Single {
        app_name: "bench",
        aspects,
        identity: generate_identity_secret(),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        resource_strategy,
    };
    let destination = single
        .destination_hash()
        .expect("the bench destination name is valid");

    let (event_tx, event_rx) = mpsc::unbounded_channel::<Event>();
    let on_event = move |event: PrnsEvent<'_>, _state: &()| {
        let mapped = match event {
            PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) => {
                Some(Event::Heard(destination))
            }
            PrnsEvent::Diagnostic(Diagnostic::LinkEstablished(_)) => Some(Event::LinkUp),
            PrnsEvent::Diagnostic(Diagnostic::LinkClosed { .. }) => Some(Event::Closed),
            PrnsEvent::Message(Message::Resource { data, .. }) => {
                Some(Event::ResourceIn(data.len()))
            }
            PrnsEvent::Diagnostic(Diagnostic::ResourceAssembled { total_size, .. }) => {
                Some(Event::ResourceIn(total_size as usize))
            }
            _ => None,
        };
        if let Some(event) = mapped {
            let _ = event_tx.send(event);
        }
    };

    if role == "responder" {
        let (node, bound) =
            build_responder_node(single, (), routes![], on_event, manifest, addr, None).await;
        let commands = node.handle();
        println!("READY role=responder addr={bound}");
        let firehose = respond_resource_runtime(
            destination,
            announce_every,
            duration,
            initiators,
            &commands,
            event_rx,
        );
        tokio::select! {
            () = node.run() => unreachable!("the responder's run loop returned"),
            () = firehose => {}
        }
    } else if role == "initiator" {
        let node = build_initiator_node(single, on_event, manifest, addr).await;
        let commands = node.handle();
        println!("READY role=initiator");
        let firehose = async {
            initiate_resource_runtime(&manifest.profile, duration, &commands, event_rx).await;
            tokio::time::sleep(Duration::from_millis(200)).await;
        };
        tokio::select! {
            () = node.run() => unreachable!("the initiator's run loop returned"),
            () = firehose => {}
        }
    } else {
        panic!("unknown role {role:?}");
    }
}

async fn respond_resource_runtime(
    destination: DestinationHash,
    announce_every: Duration,
    duration: Duration,
    initiator_count: usize,
    commands: &TokioPrnsHandle,
    mut events: mpsc::UnboundedReceiver<Event>,
) {
    let mut links_up = 0usize;
    let mut closed_links = 0usize;
    let mut announce = tokio::time::interval(announce_every);
    let mut announcing = true;
    let report_at = tokio::time::Instant::now() + duration + DRAIN_GRACE;
    let mut received = 0u64;
    let mut payload_bytes = 0u64;
    loop {
        tokio::select! {
            _ = announce.tick(), if announcing => {
                if commands
                    .issue(EngineCommand::AnnounceNow(AnnounceNow {
                        destination,
                        target: AnnounceTarget::AllInterfaces,
                        app_data: AnnounceAppData::Registered,
                    }))
                    .is_none()
                {
                    return;
                }
            }
            _ = tokio::time::sleep_until(report_at) => {
                println!("RESULT received={received} payload_bytes={payload_bytes}");
                return;
            }
            event = events.recv() => {
                match event {
                    Some(Event::LinkUp) => {
                        links_up += 1;
                        if links_up >= initiator_count {
                            announcing = false;
                        }
                    }
                    Some(Event::ResourceIn(bytes)) => {
                        received += 1;
                        payload_bytes += bytes as u64;
                    }
                    Some(Event::Closed) if closed_links + 1 < initiator_count => {
                        closed_links += 1;
                    }
                    Some(Event::Closed) | None => {
                        println!("RESULT received={received} payload_bytes={payload_bytes}");
                        return;
                    }
                    Some(_) => {}
                }
            }
        }
    }
}

struct CyclingSource<'a> {
    block: &'a [u8],
    pos: usize,
    remaining: usize,
}

impl<'a> CyclingSource<'a> {
    fn new(block: &'a [u8], total_len: usize) -> Self {
        Self {
            block,
            pos: 0,
            remaining: total_len,
        }
    }
}

impl AsyncRead for CyclingSource<'_> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        while this.remaining > 0 && buf.remaining() > 0 {
            if this.pos == this.block.len() {
                this.pos = 0;
            }
            let take = this
                .remaining
                .min(buf.remaining())
                .min(this.block.len() - this.pos);
            buf.put_slice(&this.block[this.pos..this.pos + take]);
            this.pos += take;
            this.remaining -= take;
        }
        std::task::Poll::Ready(Ok(()))
    }
}

async fn initiate_resource_runtime(
    profile: &Profile,
    duration: Duration,
    commands: &TokioPrnsHandle,
    mut events: mpsc::UnboundedReceiver<Event>,
) {
    let destination = loop {
        match events.recv().await.expect("reactor alive") {
            Event::Heard(destination) => break destination,
            _ => {}
        }
    };
    let link_id = commands
        .establish_link(destination)
        .await
        .expect("link establishes");
    let block = scenario_payload(profile, MAX_EFFICIENT_SIZE);
    let compression = segment_compression(profile);
    let mut sizes = SizeSequence::new(
        profile.size_seed,
        profile.payload_min,
        profile.payload_max,
        profile.payload_len,
    );
    let started = tokio::time::Instant::now();
    let deadline = started + duration;
    let mut sent = 0u64;
    let mut settled = 0u64;
    let mut failures = 0u64;
    let mut payload_bytes = 0u64;
    let mut transfer_ms: Vec<u64> = Vec::new();
    while tokio::time::Instant::now() < deadline {
        let len = sizes.next_len();
        sent += 1;
        let transfer_started = tokio::time::Instant::now();
        match commands
            .send_resource_with_compression(
                link_id,
                len as u64,
                CyclingSource::new(&block, len),
                compression,
            )
            .await
        {
            Ok(()) => {
                settled += 1;
                payload_bytes += len as u64;
                transfer_ms.push(transfer_started.elapsed().as_millis() as u64);
            }
            Err(_) => failures += 1,
        }
    }
    let elapsed_ms = started.elapsed().as_millis().max(1) as u64;
    commands.close_link(link_id);
    transfer_ms.sort_unstable();
    let seconds = (elapsed_ms as f64 / 1000.0).max(f64::EPSILON);
    println!(
        "RESULT sent={sent} settled={settled} failures={failures} \
         payload_bytes={payload_bytes} elapsed_ms={elapsed_ms} \
         goodput_bytes_per_sec={:.0} goodput_mbits_per_sec={:.2} \
         transfer_p50_ms={:.0} transfer_p99_ms={:.0} build={BUILD_PROFILE}",
        payload_bytes as f64 / seconds,
        payload_bytes as f64 * 8.0 / seconds / 1_000_000.0,
        percentile(&transfer_ms, 0.50),
        percentile(&transfer_ms, 0.99),
    );
}

struct RequestServer {
    served: Arc<AtomicU64>,
    response_bytes: Arc<AtomicU64>,
    scratch: Arc<Vec<u8>>,
}

struct BenchSizedRequestRoute;

impl RequestRoute<RequestServer> for BenchSizedRequestRoute {
    const PATH: &'static str = REQUEST_PATH;
    const POLICY: RoutePolicy = RoutePolicy::AllowAll;
    async fn handle(mut cx: RequestContext<'_, RequestServer>) -> Result<(), Decline> {
        let wanted = msgpack_bin_payload(cx.data)
            .get(..2)
            .map(|len| u16::from_be_bytes([len[0], len[1]]) as usize)
            .unwrap_or(0)
            .min(cx.state.scratch.len());
        let mut framed = Vec::with_capacity(wanted + 3);
        begin_msgpack_bin(wanted, &mut framed);
        framed.extend_from_slice(&cx.state.scratch[..wanted]);
        cx.state.served.fetch_add(1, Ordering::Relaxed);
        cx.state
            .response_bytes
            .fetch_add(wanted as u64, Ordering::Relaxed);
        cx.respond(&framed)
    }
}

async fn run_request_endpoint(manifest: &Manifest, role: &str, addr: &str, duration: Duration) {
    let aspect: &'static str = Box::leak(manifest.name.clone().into_boxed_str());
    let aspects: &'static [&'static str] = Box::leak(Box::new([aspect]));
    let announce_every = Duration::from_millis(manifest.profile.announce_every_ms);
    let initiators = manifest.profile.initiator_count;
    let single = PreConfiguredDestination::Single {
        app_name: "bench",
        aspects,
        identity: generate_identity_secret(),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        resource_strategy: ResourceStrategy::AcceptNone,
    };
    let destination = single
        .destination_hash()
        .expect("the bench destination name is valid");

    if role == "responder" {
        let served = Arc::new(AtomicU64::new(0));
        let response_bytes = Arc::new(AtomicU64::new(0));
        let app_state = RequestServer {
            served: Arc::clone(&served),
            response_bytes: Arc::clone(&response_bytes),
            scratch: Arc::new(incompressible_payload(512)),
        };
        let (event_tx, event_rx) = mpsc::unbounded_channel::<Event>();
        let on_event = move |event: PrnsEvent<'_>, _state: &RequestServer| {
            let mapped = match event {
                PrnsEvent::Diagnostic(Diagnostic::LinkEstablished(_)) => Some(Event::LinkUp),
                PrnsEvent::Diagnostic(Diagnostic::LinkClosed { .. }) => Some(Event::Closed),
                _ => None,
            };
            if let Some(event) = mapped {
                let _ = event_tx.send(event);
            }
        };
        let (node, bound) = build_responder_node(
            single,
            app_state,
            routes![BenchSizedRequestRoute],
            on_event,
            manifest,
            addr,
            None,
        )
        .await;
        let commands = node.handle();
        println!("READY role=responder addr={bound}");
        let firehose = respond_request_runtime(
            destination,
            announce_every,
            duration,
            initiators,
            &served,
            &response_bytes,
            &commands,
            event_rx,
        );
        tokio::select! {
            () = node.run() => unreachable!("the responder's run loop returned"),
            () = firehose => {}
        }
    } else if role == "initiator" {
        let (event_tx, event_rx) = mpsc::unbounded_channel::<Event>();
        let on_event = move |event: PrnsEvent<'_>, _state: &()| {
            let mapped = match event {
                PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) => {
                    Some(Event::Heard(destination))
                }
                PrnsEvent::Diagnostic(Diagnostic::CommandSettled { id, settlement }) => {
                    Some(Event::Settled(id, settlement))
                }
                PrnsEvent::Message(Message::Response { data, .. }) => {
                    Some(Event::Response(msgpack_bin_payload(data).len()))
                }
                _ => None,
            };
            if let Some(event) = mapped {
                let _ = event_tx.send(event);
            }
        };
        let node = build_initiator_node(single, on_event, manifest, addr).await;
        let commands = node.handle();
        println!("READY role=initiator");
        let firehose = async {
            initiate_request_runtime(&manifest.profile, duration, &commands, event_rx).await;
            tokio::time::sleep(Duration::from_millis(200)).await;
        };
        tokio::select! {
            () = node.run() => unreachable!("the initiator's run loop returned"),
            () = firehose => {}
        }
    } else {
        panic!("unknown role {role:?}");
    }
}

async fn respond_request_runtime(
    destination: DestinationHash,
    announce_every: Duration,
    duration: Duration,
    initiator_count: usize,
    served: &AtomicU64,
    response_bytes: &AtomicU64,
    commands: &TokioPrnsHandle,
    mut events: mpsc::UnboundedReceiver<Event>,
) {
    let mut links_up = 0usize;
    let mut closed_links = 0usize;
    let mut announce = tokio::time::interval(announce_every);
    let mut announcing = true;
    let report_at = tokio::time::Instant::now() + duration + DRAIN_GRACE;
    loop {
        tokio::select! {
            _ = announce.tick(), if announcing => {
                if commands
                    .issue(EngineCommand::AnnounceNow(AnnounceNow {
                        destination,
                        target: AnnounceTarget::AllInterfaces,
                        app_data: AnnounceAppData::Registered,
                    }))
                    .is_none()
                {
                    return;
                }
            }
            _ = tokio::time::sleep_until(report_at) => {
                println!(
                    "RESULT served={} response_bytes={}",
                    served.load(Ordering::Relaxed),
                    response_bytes.load(Ordering::Relaxed)
                );
                return;
            }
            event = events.recv() => {
                match event {
                    Some(Event::LinkUp) => {
                        links_up += 1;
                        if links_up >= initiator_count {
                            announcing = false;
                        }
                    }
                    Some(Event::Closed) if closed_links + 1 < initiator_count => {
                        closed_links += 1;
                    }
                    Some(Event::Closed) | None => {
                        println!(
                            "RESULT served={} response_bytes={}",
                            served.load(Ordering::Relaxed),
                            response_bytes.load(Ordering::Relaxed)
                        );
                        return;
                    }
                    Some(_) => {}
                }
            }
        }
    }
}

async fn initiate_request_runtime(
    profile: &Profile,
    duration: Duration,
    commands: &TokioPrnsHandle,
    mut events: mpsc::UnboundedReceiver<Event>,
) {
    let destination = loop {
        match events.recv().await.expect("reactor alive") {
            Event::Heard(destination) => break destination,
            _ => {}
        }
    };
    let link_id = commands
        .establish_link(destination)
        .await
        .expect("link establishes");

    let scratch = incompressible_payload(profile.request_max.max(2));
    let mut request_sizes = SizeSequence::new(
        profile.size_seed,
        profile.request_min.max(2),
        profile.request_max,
        profile.request_min.max(2),
    );
    let mut response_sizes = SizeSequence::new(
        profile.size_seed ^ 0xA5A5_A5A5_A5A5_A5A5,
        profile.response_min,
        profile.response_max,
        profile.response_min,
    );
    let path_hash = RequestPathHash::of(REQUEST_PATH);
    let started = tokio::time::Instant::now();
    let deadline = started + duration;
    let mut sent = 0u64;
    let mut delivered = 0u64;
    let mut delivered_after_reconnect = 0u64;
    let reconnect_after = Duration::from_millis(profile.reconnect_at_ms.saturating_add(1000));
    let mut timeouts = 0u64;
    let mut in_flight = 0usize;
    let mut request_bytes = 0u64;
    let mut response_bytes = 0u64;
    let mut rtts: Vec<u64> = Vec::new();
    let mut framed = Vec::with_capacity(profile.request_max + 3);
    let mut send_one = |in_flight: &mut usize, sent: &mut u64| {
        let request_len = request_sizes.next_len();
        let wanted = response_sizes.next_len() as u16;
        begin_msgpack_bin(request_len, &mut framed);
        framed.extend_from_slice(&wanted.to_be_bytes());
        framed.extend_from_slice(&scratch[..request_len - 2]);
        request_bytes += request_len as u64;
        if commands
            .issue(EngineCommand::SendRequest(SendRequest {
                link_id,
                path_hash,
                data: SendRequestData::from_slice(&framed).expect("request fits"),
            }))
            .is_some()
        {
            *sent += 1;
            *in_flight += 1;
        }
    };

    for _ in 0..profile.window {
        send_one(&mut in_flight, &mut sent);
    }
    let drain_deadline = deadline + DRAIN_GRACE;
    let failure_streak_limit = failure_streak_limit(profile.window);
    let mut failure_streak = 0u64;
    let mut died = false;
    while in_flight > 0 {
        let event = tokio::time::timeout_at(drain_deadline, events.recv()).await;
        let Ok(Some(event)) = event else { break };
        match event {
            Event::Settled(_, Settlement::SendRequest(result)) => {
                in_flight -= 1;
                match result {
                    Ok(receipt) => {
                        failure_streak = 0;
                        delivered += 1;
                        if profile.reconnect_at_ms > 0 && started.elapsed() > reconnect_after {
                            delivered_after_reconnect += 1;
                        }
                        rtts.push(receipt.rtt.millis());
                    }
                    Err(_) => {
                        timeouts += 1;
                        failure_streak += 1;
                    }
                }
                if !died && failure_streak >= failure_streak_limit {
                    died = true;
                    eprintln!("DIED mechanism=request failure_streak={failure_streak}");
                }
                if !died && tokio::time::Instant::now() < deadline {
                    send_one(&mut in_flight, &mut sent);
                }
            }
            Event::Response(bytes) => {
                response_bytes += bytes as u64;
            }
            _ => {}
        }
    }
    let elapsed_ms = started.elapsed().as_millis() as u64;

    commands.close_link(link_id);
    let close_deadline = tokio::time::Instant::now() + DRAIN_GRACE;
    loop {
        match tokio::time::timeout_at(close_deadline, events.recv()).await {
            Ok(Some(Event::Settled(_, Settlement::CloseLink(_)))) | Ok(None) | Err(_) => break,
            Ok(Some(_)) => {}
        }
    }

    rtts.sort_unstable();
    let seconds = (elapsed_ms as f64 / 1000.0).max(f64::EPSILON);
    let reconnect_field = if profile.reconnect_at_ms > 0 {
        format!(" delivered_after_reconnect={delivered_after_reconnect}")
    } else {
        String::new()
    };
    println!(
        "RESULT sent={sent} delivered={delivered} timeouts={timeouts} \
         request_bytes={request_bytes} response_bytes={response_bytes} \
         elapsed_ms={elapsed_ms} requests_per_sec={:.1} \
         rtt_p50_ms={:.0} rtt_p99_ms={:.0}{}{reconnect_field} build={BUILD_PROFILE}",
        delivered as f64 / seconds,
        percentile(&rtts, 0.50),
        percentile(&rtts, 0.99),
        died_marker(died),
    );
}

async fn run_churn_endpoint(manifest: &Manifest, role: &str, addr: &str, duration: Duration) {
    let aspect: &'static str = Box::leak(manifest.name.clone().into_boxed_str());
    let aspects: &'static [&'static str] = Box::leak(Box::new([aspect]));
    let announce_every = Duration::from_millis(manifest.profile.announce_every_ms);
    let resource_strategy = if role == "responder" {
        responder_resource_strategy(&manifest.profile)
    } else {
        ResourceStrategy::AcceptNone
    };
    let single = PreConfiguredDestination::Single {
        app_name: "bench",
        aspects,
        identity: generate_identity_secret(),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        resource_strategy,
    };
    let destination = single
        .destination_hash()
        .expect("the bench destination name is valid");

    let (event_tx, event_rx) = mpsc::unbounded_channel::<Event>();
    let on_event = move |event: PrnsEvent<'_>, _state: &()| {
        let mapped = match event {
            PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) => {
                Some(Event::Heard(destination))
            }
            PrnsEvent::Diagnostic(Diagnostic::CommandSettled { id, settlement }) => {
                Some(Event::Settled(id, settlement))
            }
            PrnsEvent::Diagnostic(Diagnostic::LinkEstablished(_)) => Some(Event::LinkUp),
            PrnsEvent::Message(Message::Delivered(Delivery::Single(delivery))) => {
                Some(Event::Delivered(delivery.plaintext.len()))
            }
            PrnsEvent::Message(Message::Delivered(Delivery::Link(delivery))) => {
                Some(Event::Delivered(delivery.plaintext.len()))
            }
            PrnsEvent::Message(Message::Resource { data, .. }) => {
                Some(Event::ResourceIn(data.len()))
            }
            PrnsEvent::Diagnostic(Diagnostic::ResourceAssembled { total_size, .. }) => {
                Some(Event::ResourceIn(total_size as usize))
            }
            _ => None,
        };
        if let Some(event) = mapped {
            let _ = event_tx.send(event);
        }
    };

    if role == "responder" {
        let (node, bound) =
            build_responder_node(single, (), routes![], on_event, manifest, addr, None).await;
        let commands = node.handle();
        println!("READY role=responder addr={bound}");
        let firehose =
            respond_churn_runtime(destination, announce_every, duration, &commands, event_rx);
        tokio::select! {
            () = node.run() => unreachable!("the responder's run loop returned"),
            () = firehose => {}
        }
    } else if role == "initiator" {
        let node = build_initiator_node(single, on_event, manifest, addr).await;
        let commands = node.handle();
        println!("READY role=initiator");
        let firehose = async {
            initiate_churn_runtime(&manifest.profile, duration, &commands, event_rx).await;
            tokio::time::sleep(Duration::from_millis(200)).await;
        };
        tokio::select! {
            () = node.run() => unreachable!("the initiator's run loop returned"),
            () = firehose => {}
        }
    } else {
        panic!("unknown role {role:?}");
    }
}

async fn respond_churn_runtime(
    destination: DestinationHash,
    announce_every: Duration,
    duration: Duration,
    commands: &TokioPrnsHandle,
    mut events: mpsc::UnboundedReceiver<Event>,
) {
    let mut announce = tokio::time::interval(announce_every);
    let mut announcing = true;
    let mut idle = tokio::time::interval(Duration::from_millis(200));
    let report_at = tokio::time::Instant::now() + duration + DRAIN_GRACE;
    let mut received = 0u64;
    let mut payload_bytes = 0u64;
    let mut last_delivery: Option<tokio::time::Instant> = None;
    loop {
        tokio::select! {
            _ = announce.tick(), if announcing => {
                if commands
                    .issue(EngineCommand::AnnounceNow(AnnounceNow {
                        destination,
                        target: AnnounceTarget::AllInterfaces,
                        app_data: AnnounceAppData::Registered,
                    }))
                    .is_none()
                {
                    return;
                }
            }
            _ = idle.tick() => {
                if last_delivery.is_some_and(|at| at.elapsed() > QUIET_AFTER_TRAFFIC) {
                    println!("RESULT received={received} payload_bytes={payload_bytes}");
                    return;
                }
            }
            _ = tokio::time::sleep_until(report_at) => {
                println!("RESULT received={received} payload_bytes={payload_bytes}");
                return;
            }
            event = events.recv() => {
                match event {
                    Some(Event::LinkUp) => {
                        announcing = false;
                    }
                    Some(Event::Delivered(bytes)) | Some(Event::ResourceIn(bytes)) => {
                        received += 1;
                        payload_bytes += bytes as u64;
                        last_delivery = Some(tokio::time::Instant::now());
                    }
                    None => return,
                    Some(_) => {}
                }
            }
        }
    }
}

async fn initiate_churn_runtime(
    profile: &Profile,
    duration: Duration,
    commands: &TokioPrnsHandle,
    mut events: mpsc::UnboundedReceiver<Event>,
) {
    let destination = loop {
        match events.recv().await.expect("reactor alive") {
            Event::Heard(destination) => break destination,
            _ => {}
        }
    };

    let scratch = scenario_payload(profile, profile.file_max.max(profile.page_max));
    let compression = segment_compression(profile);
    let mut sizes = SizeSequence::new(profile.size_seed, 0, 0, 1);
    let started = tokio::time::Instant::now();
    let deadline = started + duration;
    let mut cycles = 0u64;
    let mut failures = 0u64;
    let mut commands_moved = 0u64;
    let mut pages_moved = 0u64;
    let mut files_moved = 0u64;
    let mut payload_bytes = 0u64;
    let mut establish_ms: Vec<u64> = Vec::new();
    let mut cycle_ms: Vec<u64> = Vec::new();
    let mut close_ms: Vec<u64> = Vec::new();
    let mut transfer_ms_by_band: [Vec<u64>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut failure_streak = 0u64;
    let mut died = false;

    'churn: while tokio::time::Instant::now() < deadline {
        let cycle_started = tokio::time::Instant::now();
        let Some(establish_id) =
            commands.issue(EngineCommand::EstablishLink(EstablishLink { destination }))
        else {
            break;
        };
        let link_id = loop {
            match events.recv().await {
                Some(Event::Settled(id, Settlement::EstablishLink(result)))
                    if id == establish_id =>
                {
                    match result {
                        Ok(established) => break established.link_id,
                        Err(_) => {
                            failures += 1;
                            failure_streak += 1;
                            if failure_streak >= CHURN_FAILURE_STREAK_LIMIT {
                                died = true;
                                eprintln!("DIED mechanism=churn failure_streak={failure_streak}");
                                break 'churn;
                            }
                            continue 'churn;
                        }
                    }
                }
                Some(_) => {}
                None => break 'churn,
            }
        };
        establish_ms.push(cycle_started.elapsed().as_millis() as u64);

        let (band, len) = roll_band(&mut sizes, profile);
        let transfer_started = tokio::time::Instant::now();
        let moved = match band {
            Band::Command => {
                let Some(transfer_id) = commands.issue(EngineCommand::SendToLink(SendToLink {
                    link_id,
                    payload: SendToLinkPayload::from_slice(&scratch[..len]).expect("command fits"),
                })) else {
                    break;
                };
                loop {
                    match events.recv().await {
                        Some(Event::Settled(id, Settlement::SendToLink(result)))
                            if id == transfer_id =>
                        {
                            break result.is_ok();
                        }
                        Some(_) => {}
                        None => break 'churn,
                    }
                }
            }
            Band::Page | Band::File => commands
                .send_resource_with_compression(link_id, len as u64, &scratch[..len], compression)
                .await
                .is_ok(),
        };
        let transfer_elapsed = transfer_started.elapsed().as_millis() as u64;
        if moved {
            failure_streak = 0;
            payload_bytes += len as u64;
            let band_index = match band {
                Band::Command => {
                    commands_moved += 1;
                    0
                }
                Band::Page => {
                    pages_moved += 1;
                    1
                }
                Band::File => {
                    files_moved += 1;
                    2
                }
            };
            transfer_ms_by_band[band_index].push(transfer_elapsed);
        } else {
            failures += 1;
            failure_streak += 1;
        }

        let close_started = tokio::time::Instant::now();
        commands.close_link(link_id);
        loop {
            match events.recv().await {
                Some(Event::Settled(_, Settlement::CloseLink(_))) => break,
                Some(_) => {}
                None => break 'churn,
            }
        }
        close_ms.push(close_started.elapsed().as_millis() as u64);
        if moved {
            cycles += 1;
            cycle_ms.push(cycle_started.elapsed().as_millis() as u64);
        }
        if !died && failure_streak >= CHURN_FAILURE_STREAK_LIMIT {
            died = true;
            eprintln!("DIED mechanism=churn failure_streak={failure_streak}");
            break;
        }
    }
    let elapsed_ms = started.elapsed().as_millis() as u64;

    establish_ms.sort_unstable();
    cycle_ms.sort_unstable();
    let seconds = (elapsed_ms as f64 / 1000.0).max(f64::EPSILON);
    println!(
        "RESULT cycles={cycles} failures={failures} commands={commands_moved} \
         pages={pages_moved} files={files_moved} payload_bytes={payload_bytes} \
         elapsed_ms={elapsed_ms} cycles_per_sec={:.1} \
         establish_p50_ms={:.0} establish_p99_ms={:.0} \
         cycle_p50_ms={:.0} cycle_p99_ms={:.0}{}",
        cycles as f64 / seconds,
        percentile(&establish_ms, 0.50),
        percentile(&establish_ms, 0.99),
        percentile(&cycle_ms, 0.50),
        percentile(&cycle_ms, 0.99),
        died_marker(died),
    );

    let [mut command_ms, mut page_ms, mut file_ms] = transfer_ms_by_band;
    let establish_line = phase_line("establish", &mut establish_ms);
    let close_line = phase_line("close", &mut close_ms);
    let command_line = phase_line("transfer_command", &mut command_ms);
    let page_line = phase_line("transfer_page", &mut page_ms);
    let file_line = phase_line("transfer_file", &mut file_ms);
    eprintln!(
        "PHASES {establish_line} | {close_line} | {command_line} | {page_line} | {file_line}"
    );
}

async fn run_tunnel_probe(manifest: &Manifest, addr: &str, duration: Duration) {
    let mut engine = EngineState::<NodeStorage>::new(generate_identity_secret());
    let node = engine.held_identity_hashes()[0];
    let _destination = engine
        .register_single_destination(
            &node,
            "bench",
            &[&manifest.name],
            b"",
            ProofStrategy::ProveAll,
            LinkRequestPolicy::AcceptAll,
            RatchetPolicy::NoRatchets,
        )
        .expect("registers the bench destination");
    let (command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();
    let (notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
    let (in_tx, in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, LANE_DEPTH);
    let (out_tx, out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, LANE_DEPTH);
    let seam = TokioInterfaceSeam::new(TCP_INTERFACE_ID, in_tx, notify_tx, out_rx);
    let egress = Egress::new(vec![(TCP_INTERFACE_ID, out_tx)]);
    let interfaces = vec![tcp_core::descriptor(
        TCP_INTERFACE_ID,
        tcp_core::TCP_BITRATE_ESTIMATE,
    )];
    let (event_tx, event_rx) = mpsc::unbounded_channel::<Event>();
    let journal = move |journaled: Journaled<'_>| match journaled {
        Journaled::AnnounceHeard { destination, .. } => {
            let _ = event_tx.send(Event::Heard(destination));
        }
        Journaled::CommandSettled { id, settlement } => {
            let _ = event_tx.send(Event::Settled(id, settlement));
        }
        _ => {}
    };
    let interface = TcpClientInterface::new_with_id(
        TCP_INTERFACE_ID,
        addr.to_string(),
        tcp_core::TCP_BITRATE_ESTIMATE,
        Duration::from_millis(100),
    );
    tokio::spawn(interface.run(seam));
    tokio::spawn(run(
        engine,
        TokioHost::new(),
        ReactorWiring {
            interfaces,
            ifacs: vec![],
            notify: notify_rx,
            inbound_lanes: vec![(TCP_INTERFACE_ID, in_rx)],
            commands: command_rx,
            egress,
        },
        journal,
    ));
    println!("READY role=initiator");
    initiate_single(&manifest.profile, duration, command_tx, event_rx).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
}

/// The proving end: announce on a cadence (ProveAll proves every single inside the
/// engine), count delivered payload bytes, and report once the firehose has been quiet —
/// singles have no teardown to signal the end with, so silence after traffic is it.
async fn respond(
    destination: DestinationHash,
    announce_every: Duration,
    duration: Duration,
    commands: &TokioPrnsHandle,
    mut events: mpsc::UnboundedReceiver<Event>,
) {
    let mut announce = tokio::time::interval(announce_every);
    let report_at = tokio::time::Instant::now() + duration + DRAIN_GRACE;
    let mut delivered = 0u64;
    let mut payload_bytes = 0u64;
    loop {
        tokio::select! {
            _ = announce.tick(), if delivered == 0 => {
                if commands
                    .issue(EngineCommand::AnnounceNow(AnnounceNow {
                        destination,
                        target: AnnounceTarget::AllInterfaces,
                        app_data: AnnounceAppData::Registered,
                    }))
                    .is_none()
                {
                    return;
                }
            }
            _ = tokio::time::sleep_until(report_at) => {
                println!("RESULT delivered={delivered} payload_bytes={payload_bytes}");
                return;
            }
            event = events.recv() => {
                match event {
                    Some(Event::Delivered(bytes)) => {
                        delivered += 1;
                        payload_bytes += bytes as u64;
                    }
                    None => return,
                    Some(_) => {}
                }
            }
        }
    }
}

/// The measuring end: hear the announce, then keep `window` singles in flight at the
/// destination until the wall-time elapses, drain what's left, and report — throughput
/// from the settlement counts, latency from the proofs' own `rtt_ms`.
async fn initiate(
    profile: &Profile,
    duration: Duration,
    commands: &TokioPrnsHandle,
    mut events: mpsc::UnboundedReceiver<Event>,
) {
    let destination = loop {
        match events.recv().await.expect("reactor alive") {
            Event::Heard(destination) => break destination,
            _ => {}
        }
    };

    let scratch = incompressible_payload(profile.payload_max.max(profile.payload_len));
    let mut sizes = SizeSequence::new(
        profile.size_seed,
        profile.payload_min,
        profile.payload_max,
        profile.payload_len,
    );
    let started = tokio::time::Instant::now();
    let deadline = started + duration;
    let mut sent = 0u64;
    let mut delivered = 0u64;
    let mut timeouts = 0u64;
    let mut in_flight = 0usize;
    let mut sent_sizes: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    let mut delivered_bytes = 0u64;
    let mut rtts: Vec<u64> = Vec::new();
    let mut send_one =
        |in_flight: &mut usize,
         sent: &mut u64,
         sent_sizes: &mut std::collections::HashMap<u64, usize>| {
            let len = sizes.next_len();
            if let Some(id) = commands.issue(EngineCommand::SendSinglePacket(SendSinglePacket {
                destination,
                payload: SendSinglePacketPayload::from_slice(&scratch[..len])
                    .expect("payload fits"),
            })) {
                sent_sizes.insert(id.0, len);
                *sent += 1;
                *in_flight += 1;
            }
        };

    for _ in 0..profile.window {
        send_one(&mut in_flight, &mut sent, &mut sent_sizes);
    }
    let drain_deadline = deadline + DRAIN_GRACE;
    let failure_streak_limit = failure_streak_limit(profile.window);
    let mut failure_streak = 0u64;
    let mut died = false;
    while in_flight > 0 {
        let event = tokio::time::timeout_at(drain_deadline, events.recv()).await;
        let Ok(Some(event)) = event else { break };
        if let Event::Settled(id, Settlement::SendSinglePacket(result)) = event {
            in_flight -= 1;
            let size = sent_sizes.remove(&id.0).unwrap_or(0) as u64;
            match result {
                Ok(receipt) => {
                    failure_streak = 0;
                    delivered += 1;
                    delivered_bytes += size;
                    rtts.push(receipt.rtt.millis());
                }
                Err(_) => {
                    timeouts += 1;
                    failure_streak += 1;
                }
            }
            if !died && failure_streak >= failure_streak_limit {
                died = true;
                eprintln!("DIED mechanism=single failure_streak={failure_streak}");
            }
            if !died && tokio::time::Instant::now() < deadline {
                send_one(&mut in_flight, &mut sent, &mut sent_sizes);
            }
        }
    }
    let elapsed_ms = started.elapsed().as_millis() as u64;

    rtts.sort_unstable();
    let payload_bytes = delivered_bytes;
    let seconds = (elapsed_ms as f64 / 1000.0).max(f64::EPSILON);
    println!(
        "RESULT sent={sent} delivered={delivered} timeouts={timeouts} \
         payload_bytes={payload_bytes} elapsed_ms={elapsed_ms} \
         delivered_per_sec={:.1} goodput_bytes_per_sec={:.0} \
         rtt_p50_ms={:.0} rtt_p99_ms={:.0}{} build={BUILD_PROFILE}",
        delivered as f64 / seconds,
        payload_bytes as f64 / seconds,
        percentile(&rtts, 0.50),
        percentile(&rtts, 0.99),
        died_marker(died),
    );
}

/// The proving end: announce on a cadence until the peer's link arrives (ProveAll does
/// the proving inside the engine), count delivered payload bytes, and report when the
/// initiator closes the link.
async fn respond_link(
    destination: DestinationHash,
    announce_every: Duration,
    expected_links: usize,
    commands: &TokioPrnsHandle,
    mut events: mpsc::UnboundedReceiver<Event>,
) {
    let mut links_up = 0usize;
    let mut closed_links = 0usize;
    let mut announce = tokio::time::interval(announce_every);
    let mut announcing = true;
    let mut delivered = 0u64;
    let mut payload_bytes = 0u64;
    loop {
        tokio::select! {
            _ = announce.tick(), if announcing => {
                if commands
                    .issue(EngineCommand::AnnounceNow(AnnounceNow {
                        destination,
                        target: AnnounceTarget::AllInterfaces,
                        app_data: AnnounceAppData::Registered,
                    }))
                    .is_none()
                {
                    return;
                }
            }
            event = events.recv() => {
                match event {
                    Some(Event::LinkUp) => {
                        links_up += 1;
                        if links_up >= expected_links {
                            announcing = false;
                        }
                    }
                    Some(Event::Delivered(bytes)) => {
                        delivered += 1;
                        payload_bytes += bytes as u64;
                    }
                    Some(Event::Closed) if closed_links + 1 < expected_links => {
                        closed_links += 1;
                    }
                    Some(Event::Closed) | None => {
                        println!("RESULT delivered={delivered} payload_bytes={payload_bytes}");
                        return;
                    }
                    Some(_) => {}
                }
            }
        }
    }
}

/// The measuring end: establish one link, keep `window` sends in flight until the
/// wall-time elapses, drain what's left, close the link, and report — throughput from
/// the settlement counts, latency from the receipts' own `rtt_ms`.
async fn initiate_link(
    profile: &Profile,
    duration: Duration,
    commands: &TokioPrnsHandle,
    mut events: mpsc::UnboundedReceiver<Event>,
) {
    let destination = loop {
        match events.recv().await.expect("reactor alive") {
            Event::Heard(destination) => break destination,
            _ => {}
        }
    };
    let establish = commands
        .issue(EngineCommand::EstablishLink(EstablishLink { destination }))
        .expect("reactor alive");
    let link_id = loop {
        match events.recv().await.expect("reactor alive") {
            Event::Settled(id, Settlement::EstablishLink(Ok(established))) if id == establish => {
                break established.link_id;
            }
            Event::Settled(id, Settlement::EstablishLink(Err(failure))) if id == establish => {
                panic!("link refused: {failure:?}");
            }
            _ => {}
        }
    };

    let scratch = incompressible_payload(profile.payload_max.max(profile.payload_len));
    let mut sizes = SizeSequence::new(
        profile.size_seed,
        profile.payload_min,
        profile.payload_max,
        profile.payload_len,
    );
    let started = tokio::time::Instant::now();
    let deadline = started + duration;
    let mut sent = 0u64;
    let mut delivered = 0u64;
    let mut timeouts = 0u64;
    let mut in_flight = 0usize;
    let mut sent_sizes: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    let mut delivered_bytes = 0u64;
    let mut rtts: Vec<u64> = Vec::new();
    let mut send_one =
        |in_flight: &mut usize,
         sent: &mut u64,
         sent_sizes: &mut std::collections::HashMap<u64, usize>| {
            let len = sizes.next_len();
            if let Some(id) = commands.issue(EngineCommand::SendToLink(SendToLink {
                link_id,
                payload: SendToLinkPayload::from_slice(&scratch[..len]).expect("payload fits"),
            })) {
                sent_sizes.insert(id.0, len);
                *sent += 1;
                *in_flight += 1;
            }
        };

    for _ in 0..profile.window {
        send_one(&mut in_flight, &mut sent, &mut sent_sizes);
    }
    let drain_deadline = deadline + DRAIN_GRACE;
    let failure_streak_limit = failure_streak_limit(profile.window);
    let mut failure_streak = 0u64;
    let mut died = false;
    while in_flight > 0 {
        let event = tokio::time::timeout_at(drain_deadline, events.recv()).await;
        let Ok(Some(event)) = event else { break };
        if let Event::Settled(id, Settlement::SendToLink(result)) = event {
            in_flight -= 1;
            let size = sent_sizes.remove(&id.0).unwrap_or(0) as u64;
            match result {
                Ok(receipt) => {
                    failure_streak = 0;
                    delivered += 1;
                    delivered_bytes += size;
                    rtts.push(receipt.rtt.millis());
                }
                Err(_) => {
                    timeouts += 1;
                    failure_streak += 1;
                }
            }
            if !died && failure_streak >= failure_streak_limit {
                died = true;
                eprintln!("DIED mechanism=link failure_streak={failure_streak}");
            }
            if !died && tokio::time::Instant::now() < deadline {
                send_one(&mut in_flight, &mut sent, &mut sent_sizes);
            }
        }
    }
    let elapsed_ms = started.elapsed().as_millis() as u64;

    assert!(commands.close_link(link_id), "reactor alive");
    let close_deadline = tokio::time::Instant::now() + DRAIN_GRACE;
    loop {
        match tokio::time::timeout_at(close_deadline, events.recv()).await {
            Ok(Some(Event::Settled(_, Settlement::CloseLink(_)))) | Ok(None) | Err(_) => break,
            Ok(Some(_)) => {}
        }
    }

    rtts.sort_unstable();
    let payload_bytes = delivered_bytes;
    let seconds = (elapsed_ms as f64 / 1000.0).max(f64::EPSILON);
    println!(
        "RESULT sent={sent} delivered={delivered} timeouts={timeouts} \
         payload_bytes={payload_bytes} elapsed_ms={elapsed_ms} \
         delivered_per_sec={:.1} goodput_bytes_per_sec={:.0} \
         rtt_p50_ms={:.0} rtt_p99_ms={:.0}{} build={BUILD_PROFILE}",
        delivered as f64 / seconds,
        payload_bytes as f64 / seconds,
        percentile(&rtts, 0.50),
        percentile(&rtts, 0.99),
        died_marker(died),
    );
}

/// The breadth end: establish `link_count` links at once and hold them all open, keeping exactly one
/// send in flight on each (refilling the link that just settled) until the wall-time elapses, then
/// close them all. Where `initiate_link` measures depth on one link, this measures how many links a
/// host carries concurrently. Establishing all N proves the host's transported-link table held them;
/// a delivery on every link proves it kept switching each — a link the host silently dropped goes
/// quiet and trips the per-link assertion.
async fn initiate_links_breadth(
    profile: &Profile,
    duration: Duration,
    commands: &TokioPrnsHandle,
    mut events: mpsc::UnboundedReceiver<Event>,
) {
    let destination = loop {
        match events.recv().await.expect("reactor alive") {
            Event::Heard(destination) => break destination,
            _ => {}
        }
    };

    let link_count = profile.link_count;
    for _ in 0..link_count {
        commands
            .issue(EngineCommand::EstablishLink(EstablishLink { destination }))
            .expect("reactor alive");
    }
    let mut links = Vec::with_capacity(link_count);
    let establish_deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while links.len() < link_count {
        match tokio::time::timeout_at(establish_deadline, events.recv()).await {
            Ok(Some(Event::Settled(_, Settlement::EstablishLink(Ok(established))))) => {
                links.push(established.link_id);
            }
            Ok(Some(Event::Settled(_, Settlement::EstablishLink(Err(failure))))) => {
                panic!(
                    "link {} of {link_count} refused: {failure:?}",
                    links.len() + 1
                );
            }
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => panic!(
                "only {} of {link_count} links established before the deadline",
                links.len()
            ),
        }
    }

    let scratch = incompressible_payload(profile.payload_max.max(profile.payload_len));
    let mut sizes = SizeSequence::new(
        profile.size_seed,
        profile.payload_min,
        profile.payload_max,
        profile.payload_len,
    );
    let started = tokio::time::Instant::now();
    let deadline = started + duration;
    let mut sent = 0u64;
    let mut delivered = 0u64;
    let mut timeouts = 0u64;
    let mut delivered_bytes = 0u64;
    let mut in_flight = 0usize;
    let mut rtts: Vec<u64> = Vec::new();
    let mut per_link_delivered = vec![0u64; link_count];
    let mut id_to_link: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    let mut id_to_size: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    let mut send_on =
        |link_index: usize,
         in_flight: &mut usize,
         sent: &mut u64,
         id_to_link: &mut std::collections::HashMap<u64, usize>,
         id_to_size: &mut std::collections::HashMap<u64, usize>| {
            let len = sizes.next_len();
            if let Some(id) = commands.issue(EngineCommand::SendToLink(SendToLink {
                link_id: links[link_index],
                payload: SendToLinkPayload::from_slice(&scratch[..len]).expect("payload fits"),
            })) {
                id_to_link.insert(id.0, link_index);
                id_to_size.insert(id.0, len);
                *sent += 1;
                *in_flight += 1;
            }
        };

    for link_index in 0..link_count {
        send_on(
            link_index,
            &mut in_flight,
            &mut sent,
            &mut id_to_link,
            &mut id_to_size,
        );
    }

    let drain_deadline = deadline + DRAIN_GRACE;
    while in_flight > 0 {
        let event = tokio::time::timeout_at(drain_deadline, events.recv()).await;
        let Ok(Some(event)) = event else { break };
        if let Event::Settled(id, Settlement::SendToLink(result)) = event {
            in_flight -= 1;
            let link_index = id_to_link.remove(&id.0).expect("a tracked send");
            let size = id_to_size.remove(&id.0).unwrap_or(0) as u64;
            match result {
                Ok(receipt) => {
                    delivered += 1;
                    delivered_bytes += size;
                    per_link_delivered[link_index] += 1;
                    rtts.push(receipt.rtt.millis());
                }
                Err(_) => timeouts += 1,
            }
            if tokio::time::Instant::now() < deadline {
                send_on(
                    link_index,
                    &mut in_flight,
                    &mut sent,
                    &mut id_to_link,
                    &mut id_to_size,
                );
            }
        }
    }
    let elapsed_ms = started.elapsed().as_millis() as u64;

    for &link_id in &links {
        commands.close_link(link_id);
    }
    let mut closed = 0usize;
    let close_deadline = tokio::time::Instant::now() + DRAIN_GRACE;
    while closed < link_count {
        match tokio::time::timeout_at(close_deadline, events.recv()).await {
            Ok(Some(Event::Settled(_, Settlement::CloseLink(_)))) => closed += 1,
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => break,
        }
    }

    let silent_links = per_link_delivered.iter().filter(|&&d| d == 0).count();
    assert!(
        silent_links == 0,
        "{silent_links} of {link_count} relayed links delivered nothing — a carried link was dropped",
    );

    rtts.sort_unstable();
    let payload_bytes = delivered_bytes;
    let seconds = (elapsed_ms as f64 / 1000.0).max(f64::EPSILON);
    println!(
        "RESULT sent={sent} delivered={delivered} timeouts={timeouts} \
         links_established={link_count} payload_bytes={payload_bytes} elapsed_ms={elapsed_ms} \
         delivered_per_sec={:.1} rtt_p50_ms={:.0} rtt_p99_ms={:.0} build={BUILD_PROFILE}",
        delivered as f64 / seconds,
        percentile(&rtts, 0.50),
        percentile(&rtts, 0.99),
    );
}

/// The storm end: keep `window` full link lifecycles in flight at once — establish a link,
/// close it the instant it settles, and refill the slot the moment the close settles — for
/// the wall-time, then drain. Where `initiate_links_breadth` holds N links open to measure
/// table breadth, this churns establish/teardown at breadth to measure establishment
/// throughput: every cycle pays the whole handshake crypto (the initiator's Ed25519 proof
/// verify and X25519 session DH, the responder's proof sign and DH) with no data transfer to
/// dilute it, so the establishment path is the entire cost. `window` bounds the links open at
/// once, so it stays under the link table's ceiling at any throughput.
async fn initiate_link_storm(
    profile: &Profile,
    duration: Duration,
    commands: &TokioPrnsHandle,
    mut events: mpsc::UnboundedReceiver<Event>,
) {
    let destination = loop {
        match events.recv().await.expect("reactor alive") {
            Event::Heard(destination) => break destination,
            _ => {}
        }
    };

    let window = profile.window.max(1);
    let started = tokio::time::Instant::now();
    let deadline = started + duration;
    let mut established = 0u64;
    let mut closed = 0u64;
    let mut failures = 0u64;
    let mut outstanding = 0usize;
    let mut establish_ms: Vec<u64> = Vec::new();
    let mut pending: std::collections::HashMap<u64, tokio::time::Instant> =
        std::collections::HashMap::new();

    let start_one =
        |outstanding: &mut usize,
         pending: &mut std::collections::HashMap<u64, tokio::time::Instant>| {
            if let Some(id) =
                commands.issue(EngineCommand::EstablishLink(EstablishLink { destination }))
            {
                pending.insert(id.0, tokio::time::Instant::now());
                *outstanding += 1;
            }
        };

    for _ in 0..window {
        start_one(&mut outstanding, &mut pending);
    }

    let drain_deadline = deadline + DRAIN_GRACE;
    while outstanding > 0 {
        let event = tokio::time::timeout_at(drain_deadline, events.recv()).await;
        let Ok(Some(event)) = event else { break };
        match event {
            Event::Settled(id, Settlement::EstablishLink(Ok(est))) => {
                established += 1;
                if let Some(at) = pending.remove(&id.0) {
                    establish_ms.push(at.elapsed().as_millis() as u64);
                }
                commands.close_link(est.link_id);
            }
            Event::Settled(id, Settlement::EstablishLink(Err(_))) => {
                failures += 1;
                pending.remove(&id.0);
                outstanding -= 1;
                if tokio::time::Instant::now() < deadline {
                    start_one(&mut outstanding, &mut pending);
                }
            }
            Event::Settled(_, Settlement::CloseLink(_)) => {
                closed += 1;
                outstanding -= 1;
                if tokio::time::Instant::now() < deadline {
                    start_one(&mut outstanding, &mut pending);
                }
            }
            _ => {}
        }
    }

    let elapsed_ms = started.elapsed().as_millis() as u64;
    establish_ms.sort_unstable();
    let seconds = (duration.as_millis() as f64 / 1000.0).max(f64::EPSILON);
    println!(
        "RESULT established={established} closed={closed} failures={failures} window={window} \
         elapsed_ms={elapsed_ms} establish_per_sec={:.1} establish_p50_ms={:.0} \
         establish_p99_ms={:.0} build={BUILD_PROFILE}",
        established as f64 / seconds,
        percentile(&establish_ms, 0.50),
        percentile(&establish_ms, 0.99),
    );
}

/// The measuring end of the channel firehose: establish one link, then keep the
/// channel's send window full until the wall-time elapses, drain, close, and report.
/// Unlike the bare link, the channel paces its own emission with a grow-on-proof /
/// shrink-on-loss window, so a window-full settlement is backpressure — the slot is
/// refilled and the attempt goes uncounted — while only an exhausted retransmit budget
/// settles a real timeout. `sent` therefore counts the emissions the channel accepted,
/// so it stays equal to delivered + timeouts.
async fn initiate_channel(
    profile: &Profile,
    duration: Duration,
    commands: &TokioPrnsHandle,
    mut events: mpsc::UnboundedReceiver<Event>,
) {
    let destination = loop {
        match events.recv().await.expect("reactor alive") {
            Event::Heard(destination) => break destination,
            _ => {}
        }
    };
    let establish = commands
        .issue(EngineCommand::EstablishLink(EstablishLink { destination }))
        .expect("reactor alive");
    let link_id = loop {
        match events.recv().await.expect("reactor alive") {
            Event::Settled(id, Settlement::EstablishLink(Ok(established))) if id == establish => {
                break established.link_id;
            }
            Event::Settled(id, Settlement::EstablishLink(Err(failure))) if id == establish => {
                panic!("link refused: {failure:?}");
            }
            _ => {}
        }
    };

    let scratch = incompressible_payload(profile.payload_max.max(profile.payload_len));
    let mut sizes = SizeSequence::new(
        profile.size_seed,
        profile.payload_min,
        profile.payload_max,
        profile.payload_len,
    );
    let started = tokio::time::Instant::now();
    let deadline = started + duration;
    let mut sent = 0u64;
    let mut delivered = 0u64;
    let mut timeouts = 0u64;
    let mut in_flight = 0usize;
    let mut sent_sizes: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    let mut delivered_bytes = 0u64;
    let mut rtts: Vec<u64> = Vec::new();
    let mut emit_one =
        |in_flight: &mut usize, sent_sizes: &mut std::collections::HashMap<u64, usize>| {
            let len = sizes.next_len();
            if let Some(id) = commands.issue(EngineCommand::SendToChannel(SendToChannel {
                link_id,
                message_type: BENCH_CHANNEL_MSGTYPE,
                body: SendToChannelBody::from_slice(&scratch[..len]).expect("payload fits"),
            })) {
                sent_sizes.insert(id.0, len);
                *in_flight += 1;
            }
        };

    for _ in 0..profile.window {
        emit_one(&mut in_flight, &mut sent_sizes);
    }
    let drain_deadline = deadline + DRAIN_GRACE;
    let failure_streak_limit = failure_streak_limit(profile.window);
    let mut failure_streak = 0u64;
    let mut died = false;
    while in_flight > 0 {
        let event = tokio::time::timeout_at(drain_deadline, events.recv()).await;
        let Ok(Some(event)) = event else { break };
        if let Event::Settled(id, Settlement::SendToChannel(result)) = event {
            in_flight -= 1;
            let size = sent_sizes.remove(&id.0).unwrap_or(0) as u64;
            match result {
                Ok(receipt) => {
                    failure_streak = 0;
                    sent += 1;
                    delivered += 1;
                    delivered_bytes += size;
                    rtts.push(receipt.rtt.millis());
                }
                Err(SendToChannelFailure::WindowFull) => {}
                Err(_) => {
                    sent += 1;
                    timeouts += 1;
                    failure_streak += 1;
                }
            }
            if !died && failure_streak >= failure_streak_limit {
                died = true;
                eprintln!("DIED mechanism=channel failure_streak={failure_streak}");
            }
            if !died && tokio::time::Instant::now() < deadline {
                emit_one(&mut in_flight, &mut sent_sizes);
            }
        }
    }
    let elapsed_ms = started.elapsed().as_millis() as u64;

    assert!(commands.close_link(link_id), "reactor alive");
    let close_deadline = tokio::time::Instant::now() + DRAIN_GRACE;
    loop {
        match tokio::time::timeout_at(close_deadline, events.recv()).await {
            Ok(Some(Event::Settled(_, Settlement::CloseLink(_)))) | Ok(None) | Err(_) => break,
            Ok(Some(_)) => {}
        }
    }

    rtts.sort_unstable();
    let payload_bytes = delivered_bytes;
    let seconds = (elapsed_ms as f64 / 1000.0).max(f64::EPSILON);
    println!(
        "RESULT sent={sent} delivered={delivered} timeouts={timeouts} \
         payload_bytes={payload_bytes} elapsed_ms={elapsed_ms} \
         delivered_per_sec={:.1} goodput_bytes_per_sec={:.0} \
         rtt_p50_ms={:.0} rtt_p99_ms={:.0}{} build={BUILD_PROFILE}",
        delivered as f64 / seconds,
        payload_bytes as f64 / seconds,
        percentile(&rtts, 0.50),
        percentile(&rtts, 0.99),
        died_marker(died),
    );
}

/// A pure transport node: no destinations, no app, just the engine with its transport identity
/// standing between two endpoints on two server interfaces. Everything it does (announce
/// rebroadcast with the transport stamp, link request booking, blind ciphertext switching) is
/// engine machinery under test.
async fn relay_node(manifest: &Manifest) {
    let engine = EngineState::<NodeStorage>::new(generate_identity_secret());
    let _ = manifest;

    let (_command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();
    let (notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
    let (in_a_tx, in_a_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, LANE_DEPTH);
    let (out_a_tx, out_a_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, LANE_DEPTH);
    let (in_b_tx, in_b_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, LANE_DEPTH);
    let (out_b_tx, out_b_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, LANE_DEPTH);
    let seam_a = TokioInterfaceSeam::new(TCP_INTERFACE_ID, in_a_tx, notify_tx.clone(), out_a_rx);
    let seam_b = TokioInterfaceSeam::new(RELAY_SECOND_INTERFACE_ID, in_b_tx, notify_tx, out_b_rx);
    let egress = Egress::new(vec![
        (TCP_INTERFACE_ID, out_a_tx),
        (RELAY_SECOND_INTERFACE_ID, out_b_tx),
    ]);
    let interfaces = vec![
        tcp_core::descriptor(TCP_INTERFACE_ID, tcp_core::TCP_BITRATE_ESTIMATE),
        tcp_core::descriptor(RELAY_SECOND_INTERFACE_ID, tcp_core::TCP_BITRATE_ESTIMATE),
    ];

    let side_a = BenchTcpListener::bind_with_id(
        TCP_INTERFACE_ID,
        "127.0.0.1:0",
        tcp_core::TCP_BITRATE_ESTIMATE,
    )
    .await
    .expect("binds side a");
    let addr_a = side_a.local_addr().expect("bound address");
    let side_b = BenchTcpListener::bind_with_id(
        RELAY_SECOND_INTERFACE_ID,
        "127.0.0.1:0",
        tcp_core::TCP_BITRATE_ESTIMATE,
    )
    .await
    .expect("binds side b");
    let addr_b = side_b.local_addr().expect("bound address");
    tokio::spawn(side_a.run(seam_a));
    tokio::spawn(side_b.run(seam_b));
    tokio::spawn(run(
        engine,
        TokioHost::new(),
        ReactorWiring {
            interfaces,
            ifacs: vec![],
            notify: notify_rx,
            inbound_lanes: vec![
                (TCP_INTERFACE_ID, in_a_rx),
                (RELAY_SECOND_INTERFACE_ID, in_b_rx),
            ],
            commands: command_rx,
            egress,
        },
        |_: Journaled<'_>| {},
    ));
    println!("READY role=relay addr={addr_a}>{addr_b}");
    std::future::pending::<()>().await;
}

async fn initiate_single(
    profile: &Profile,
    duration: Duration,
    commands: mpsc::UnboundedSender<HostCommand>,
    mut events: mpsc::UnboundedReceiver<Event>,
) {
    let destination = loop {
        match events.recv().await.expect("reactor alive") {
            Event::Heard(destination) => break destination,
            _ => {}
        }
    };

    let scratch = incompressible_payload(profile.payload_max.max(profile.payload_len).max(1));
    let mut sizes = SizeSequence::new(
        profile.size_seed,
        profile.payload_min,
        profile.payload_max,
        profile.payload_len,
    );
    let started = tokio::time::Instant::now();
    let deadline = started + duration;
    let drain_until = deadline + DRAIN_GRACE;
    let reconnect_after_ms = profile.reconnect_at_ms.saturating_add(1000);
    // Time-paced, not window-on-settle: a burst loss at the reconnect would otherwise fill the
    // window with stuck sends and stall the firehose before it can prove post-repoint delivery. A
    // short local timeout reaps the lost sends so fresh ones keep flowing.
    const LOCAL_TIMEOUT_MS: u64 = 5_000;
    const OUTSTANDING_CAP: usize = 256;
    let mut send_tick = tokio::time::interval(Duration::from_millis(25));
    let mut sweep = tokio::time::interval(Duration::from_millis(200));
    let mut next_id = 1u64;
    let mut sent = 0u64;
    let mut delivered = 0u64;
    let mut delivered_after_reconnect = 0u64;
    let mut timeouts = 0u64;
    let mut rtts: Vec<u64> = Vec::new();
    let mut outstanding: std::collections::HashMap<u64, u64> = std::collections::HashMap::new();
    loop {
        tokio::select! {
            _ = send_tick.tick() => {
                if tokio::time::Instant::now() < deadline && outstanding.len() < OUTSTANDING_CAP {
                    let len = sizes.next_len();
                    let id = next_id;
                    next_id += 1;
                    let command = IssuedCommand {
                        id: CommandId(id),
                        command: EngineCommand::SendSinglePacket(SendSinglePacket {
                            destination,
                            payload: SendSinglePacketPayload::from_slice(&scratch[..len])
                                .expect("payload fits"),
                        }),
                    };
                    if commands.send(HostCommand::Engine(command)).is_ok() {
                        outstanding.insert(id, started.elapsed().as_millis() as u64);
                        sent += 1;
                    }
                }
            }
            _ = sweep.tick() => {
                let now_ms = started.elapsed().as_millis() as u64;
                outstanding.retain(|_, send_ms| {
                    if now_ms.saturating_sub(*send_ms) > LOCAL_TIMEOUT_MS {
                        timeouts += 1;
                        false
                    } else {
                        true
                    }
                });
            }
            _ = tokio::time::sleep_until(drain_until) => {
                break;
            }
            event = events.recv() => {
                match event {
                    Some(Event::Settled(CommandId(id), Settlement::SendSinglePacket(result))) => {
                        if let Some(send_ms) = outstanding.remove(&id) {
                            match result {
                                Ok(receipt) => {
                                    delivered += 1;
                                    if profile.reconnect_at_ms > 0 && send_ms > reconnect_after_ms {
                                        delivered_after_reconnect += 1;
                                    }
                                    rtts.push(receipt.rtt.millis());
                                }
                                Err(_) => timeouts += 1,
                            }
                        }
                    }
                    None => break,
                    Some(_) => {}
                }
            }
        }
    }
    timeouts += outstanding.len() as u64;
    let elapsed_ms = started.elapsed().as_millis() as u64;

    rtts.sort_unstable();
    let seconds = (elapsed_ms as f64 / 1000.0).max(f64::EPSILON);
    let reconnect_field = if profile.reconnect_at_ms > 0 {
        format!(" delivered_after_reconnect={delivered_after_reconnect}")
    } else {
        String::new()
    };
    println!(
        "RESULT sent={sent} delivered={delivered} timeouts={timeouts} \
         elapsed_ms={elapsed_ms} delivered_per_sec={:.1} \
         rtt_p50_ms={:.0} rtt_p99_ms={:.0}{reconnect_field} build={BUILD_PROFILE}",
        delivered as f64 / seconds,
        percentile(&rtts, 0.50),
        percentile(&rtts, 0.99),
    );
}

async fn tunnel_relay_node(manifest: &Manifest) {
    let engine = EngineState::<NodeStorage>::new(generate_identity_secret());
    let reconnect_at = Duration::from_millis(manifest.profile.reconnect_at_ms);

    let (command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();
    let (notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
    let (in_b_tx, in_b_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, LANE_DEPTH);
    let (out_b_tx, out_b_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, LANE_DEPTH);
    let seam_b = TokioInterfaceSeam::new(
        RELAY_SECOND_INTERFACE_ID,
        in_b_tx,
        notify_tx.clone(),
        out_b_rx,
    );
    let egress = Egress::new(vec![(RELAY_SECOND_INTERFACE_ID, out_b_tx)]);
    let interfaces = vec![tcp_core::descriptor(
        RELAY_SECOND_INTERFACE_ID,
        tcp_core::TCP_BITRATE_ESTIMATE,
    )];

    let client_side = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binds the client side");
    let addr_a = client_side.local_addr().expect("bound address");
    let peer_side = BenchTcpListener::bind_with_id(
        RELAY_SECOND_INTERFACE_ID,
        "127.0.0.1:0",
        tcp_core::TCP_BITRATE_ESTIMATE,
    )
    .await
    .expect("binds the peer side");
    let addr_b = peer_side.local_addr().expect("bound address");

    tokio::spawn(peer_side.run(seam_b));
    tokio::spawn(tunnel_client_side(
        client_side,
        command_tx.clone(),
        notify_tx,
        reconnect_at,
    ));
    tokio::spawn(run(
        engine,
        TokioHost::new(),
        ReactorWiring {
            interfaces,
            ifacs: vec![],
            notify: notify_rx,
            inbound_lanes: vec![(RELAY_SECOND_INTERFACE_ID, in_b_rx)],
            commands: command_rx,
            egress,
        },
        |_: Journaled<'_>| {},
    ));
    println!("READY role=relay addr={addr_a}>{addr_b}");
    std::future::pending::<()>().await;
}

async fn tunnel_client_side(
    listener: tokio::net::TcpListener,
    commands: mpsc::UnboundedSender<HostCommand>,
    notify_tx: mpsc::UnboundedSender<InterfaceId>,
    reconnect_at: Duration,
) {
    let mut connection_index = 0u32;
    loop {
        let Ok((stream, peer)) = listener.accept().await else {
            return;
        };
        tune(&stream);
        let tag = format!("{peer}#{connection_index}").into_bytes();
        let id = InterfaceId::from_channel_tag(InterfaceKind::TcpServerPeer, &tag);
        let descriptor = tcp_core::descriptor(id, tcp_core::TCP_BITRATE_ESTIMATE);
        let (in_tx, in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, LANE_DEPTH);
        let (out_tx, out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, LANE_DEPTH);
        let seam = TokioInterfaceSeam::new(id, in_tx, notify_tx.clone(), out_rx);
        if commands
            .send(HostCommand::AddInterface(AddInterfaceCommand {
                descriptor,
                inbound: in_rx,
                egress: out_tx,
                ifac: None,
            }))
            .is_err()
        {
            return;
        }
        let connection =
            TcpServerConnection::new(tag, stream, tcp_core::TCP_BITRATE_ESTIMATE).run(seam);
        let task = tokio::spawn(connection);
        if connection_index == 0 {
            tokio::spawn(async move {
                tokio::time::sleep(reconnect_at).await;
                task.abort();
            });
        }
        connection_index += 1;
    }
}

/// A pure transport node in a trunk: listen for the next hop downstream, dial the
/// previous hop upstream, switch everything between them.
async fn chain_node(upstream: &str) {
    let engine = EngineState::<NodeStorage>::new(generate_identity_secret());

    let (_command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();
    let (notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
    let (in_down_tx, in_down_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, LANE_DEPTH);
    let (out_down_tx, out_down_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, LANE_DEPTH);
    let (in_up_tx, in_up_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, LANE_DEPTH);
    let (out_up_tx, out_up_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, LANE_DEPTH);
    let seam_down =
        TokioInterfaceSeam::new(TCP_INTERFACE_ID, in_down_tx, notify_tx.clone(), out_down_rx);
    let seam_up =
        TokioInterfaceSeam::new(RELAY_SECOND_INTERFACE_ID, in_up_tx, notify_tx, out_up_rx);
    let egress = Egress::new(vec![
        (TCP_INTERFACE_ID, out_down_tx),
        (RELAY_SECOND_INTERFACE_ID, out_up_tx),
    ]);
    let interfaces = vec![
        tcp_core::descriptor(TCP_INTERFACE_ID, tcp_core::TCP_BITRATE_ESTIMATE),
        tcp_core::descriptor(RELAY_SECOND_INTERFACE_ID, tcp_core::TCP_BITRATE_ESTIMATE),
    ];

    let downstream = BenchTcpListener::bind_with_id(
        TCP_INTERFACE_ID,
        "127.0.0.1:0",
        tcp_core::TCP_BITRATE_ESTIMATE,
    )
    .await
    .expect("binds downstream side");
    let addr = downstream.local_addr().expect("bound address");
    let up = TcpClientInterface::new_with_id(
        RELAY_SECOND_INTERFACE_ID,
        upstream.to_string(),
        tcp_core::TCP_BITRATE_ESTIMATE,
        Duration::from_millis(100),
    );
    tokio::spawn(downstream.run(seam_down));
    tokio::spawn(up.run(seam_up));
    tokio::spawn(run(
        engine,
        TokioHost::new(),
        ReactorWiring {
            interfaces,
            ifacs: vec![],
            notify: notify_rx,
            inbound_lanes: vec![
                (TCP_INTERFACE_ID, in_down_rx),
                (RELAY_SECOND_INTERFACE_ID, in_up_rx),
            ],
            commands: command_rx,
            egress,
        },
        |_: Journaled<'_>| {},
    ));
    println!("READY role=chain addr={addr}");
    std::future::pending::<()>().await;
}

const CHURN_FAILURE_STREAK_LIMIT: u64 = 64;

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

fn phase_line(label: &str, samples: &mut Vec<u64>) -> String {
    samples.sort_unstable();
    let over_500 = samples.iter().filter(|&&ms| ms > 500).count();
    let near_1s = samples
        .iter()
        .filter(|&&ms| (900..=1100).contains(&ms))
        .count();
    format!(
        "{label} n={} p50={:.0} p99={:.0} max={} over_500ms={over_500} near_1s={near_1s}",
        samples.len(),
        percentile(samples, 0.50),
        percentile(samples, 0.99),
        samples.last().copied().unwrap_or(0),
    )
}
