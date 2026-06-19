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
    AnnounceAppData, AnnounceNow, AnnounceTarget, CloseLink, CommandId, EngineCommand, EngineState,
    EstablishLink, IssuedCommand, Journaled, RatchetPolicy, Respond, RespondData, SendChannel,
    SendChannelBody, SendChannelFailure, SendLink, SendLinkPayload, SendRequest, SendRequestData,
    SendSingle, SendSinglePayload, Settlement,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::rns_parity::tcp::client::tokio::TcpClientInterface;
use personal_rns::interfaces::rns_parity::tcp::core as tcp_core;
use personal_rns::interfaces::rns_parity::tcp::server::tokio::TcpServerConnection;
use personal_rns::interfaces::rns_parity::tcp::tokio_socket::tune;
use personal_rns::interfaces::rns_parity::udp::core as udp_core;
use personal_rns::interfaces::rns_parity::udp::impls::tokio::UdpInterface;
use personal_rns::interfaces::{InterfaceConfig, InterfaceId, InterfaceKind, ReportsStatus};
use personal_rns::reactor::impls::tokio_reactor::{
    run, tokio_grant_lane, Egress, HostCommand, HostResourcePayload, SendResourceHostCommand,
    SendResourceSegmentHostCommand, TokioHost, TokioInterfaceSeam,
};
use personal_rns::reactor::interface_seam::{Interface, InterfaceSeam, MAX_WIRE_FRAME_LEN};
use personal_rns::routing::delivery::Delivery;
use personal_rns::routing::links::channel::MessageType;
use personal_rns::routing::links::request::RequestId;
use personal_rns::routing::links::resources::{ResourceStrategy, MAX_EFFICIENT_SIZE};
use personal_rns::routing::links::LinkId;
use personal_rns::routing::request_handlers::{RequestPathHash, RequestPolicy};
use personal_rns::routing::ProofStrategy;
use personal_rns::runtime::request_router::{Decline, RequestContext, RequestRoute, RoutePolicy};
use personal_rns::runtime::{
    Diagnostic, InstancePorts, LocalInstance, Message, OnExisting, PreConfiguredDestination, Prns,
    PrnsEvent, PrnsRecipe, Role, TokioPrnsHandle,
};
#[cfg(feature = "fixed-storage")]
use personal_rns::storage::Esp32S3 as NodeStorage;
#[cfg(not(feature = "fixed-storage"))]
use personal_rns::storage::GrowableHeap as NodeStorage;
use personal_rns::wire::DestinationHash;
use personal_rns::{interfaces, routes};
use tokio::sync::{mpsc, oneshot};

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

/// A point-to-point TCP listener with a fixed interface id, the shape the benchmark's nodes wire
/// their seams and lanes to. It binds a port, accepts one client, and serves that connection as a
/// single engine interface (the reference's per-connection TCP child), delegating the framing to a
/// [`TcpServerConnection`]. The fleet-wide [`TcpServer`](personal_rns::interfaces::rns_parity::tcp::server::tokio::TcpServer)
/// supervisor is the production multi-client shape; a one-shot benchmark pairing is point-to-point,
/// so it keeps the fixed id its hand-rolled reactor and recipe already key on.
struct BenchTcpListener {
    id: InterfaceId,
    listener: tokio::net::TcpListener,
    bitrate_bps: u32,
}

impl BenchTcpListener {
    async fn bind_with_id(
        id: InterfaceId,
        addr: impl tokio::net::ToSocketAddrs,
        bitrate_bps: u32,
    ) -> std::io::Result<Self> {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        Ok(Self {
            id,
            listener,
            bitrate_bps,
        })
    }

    fn local_addr(&self) -> std::io::Result<std::net::SocketAddr> {
        self.listener.local_addr()
    }
}

impl Interface for BenchTcpListener {
    const HW_MTU: usize = tcp_core::TCP_HW_MTU_CAP;
    const KIND: InterfaceKind = InterfaceKind::TcpServerPeer;

    fn descriptor(&self) -> InterfaceConfig {
        tcp_core::descriptor(self.id, self.bitrate_bps)
    }

    fn reachability_tag(&self) -> &[u8] {
        self.id.as_bytes()
    }

    async fn run<Seam: InterfaceSeam>(self, seam: Seam) {
        let Ok((stream, peer)) = self.listener.accept().await else {
            return;
        };
        tune(&stream);
        TcpServerConnection::new(peer.to_string().into_bytes(), stream, self.bitrate_bps)
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
    #[serde(default = "default_size_seed")]
    size_seed: u64,
    #[serde(default = "default_topology")]
    topology: String,
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

fn default_announce_every_ms() -> u64 {
    500
}

fn default_initiator_count() -> usize {
    1
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
    Request {
        link_id: LinkId,
        request_id: RequestId,
        wanted: usize,
    },
    Response(usize),
    Closed,
}

const REQUEST_PATH: &str = "/bench/query";

/// The engine's request/response codec carries the app's data as RAW msgpack
/// value bytes — byte-true pass-through. The reference packs and unpacks its
/// side natively, so this bench frames every payload as a msgpack bin value
/// to speak across.
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

fn msgpack_bin_into<'a>(payload: &[u8], framed: &'a mut Vec<u8>) -> &'a [u8] {
    begin_msgpack_bin(payload.len(), framed);
    framed.extend_from_slice(payload);
    framed.as_slice()
}

fn msgpack_bin_payload(framed: &[u8]) -> &[u8] {
    match framed.first() {
        Some(0xC4) => &framed[2..],
        Some(0xC5) => &framed[3..],
        _ => framed,
    }
}

fn fresh_identity() -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    let mut key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    getrandom::getrandom(&mut *key).expect("OS CSPRNG");
    key
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
        relay_node(&manifest).await;
        return;
    }
    if role == "chain" {
        chain_node(&addr).await;
        return;
    }
    // The single- and link-firehose endpoints ride the high-level runtime. Request joins them on the
    // shared-instance bus through `routes!` and the request/respond handle; resource and churn still
    // hand-roll the reactor below for the node-to-node perf path, resource because the responder-side
    // resource strategy is not yet a recipe knob.
    if matches!(
        manifest.profile.mechanism.as_str(),
        "single" | "link" | "channel"
    ) {
        run_runtime_endpoint(&manifest, &role, &addr, duration).await;
        return;
    }
    if let Some(port) = shared_instance_port() {
        if manifest.profile.mechanism == "request" {
            run_request_bus_client(&manifest, &role, duration, port).await;
            return;
        }
    }

    let mut engine = EngineState::<NodeStorage>::new(fresh_identity());
    let node = engine.held_identity_hashes()[0];
    let destination = engine
        .register_single_destination(
            &node,
            "bench",
            &[&manifest.name],
            b"",
            ProofStrategy::ProveAll,
            RatchetPolicy::NoRatchets,
        )
        .expect("registers the bench destination");

    if manifest.profile.mechanism == "request" && role == "responder" {
        engine
            .register_request_handler(&destination, REQUEST_PATH, RequestPolicy::AllowAll)
            .expect("registers the bench handler");
    }
    if matches!(manifest.profile.mechanism.as_str(), "resource" | "churn") && role == "responder" {
        assert!(engine.set_default_resource_strategy(
            &destination,
            ResourceStrategy::Accept {
                max_uncompressed_len: 128 * 1024 * 1024,
                accept_compressed: false,
            },
        ));
    }
    let (command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();
    let (notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
    let (in_tx, in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, LANE_DEPTH);
    let (out_tx, out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, LANE_DEPTH);
    let seam = TokioInterfaceSeam::new(TCP_INTERFACE_ID, in_tx, notify_tx.clone(), out_rx);
    let extra_listener_count = if role == "responder"
        && manifest.profile.wire != "udp"
        && manifest.profile.topology != "relay"
    {
        manifest.profile.initiator_count.saturating_sub(1)
    } else {
        0
    };
    let mut egress_lanes = vec![(TCP_INTERFACE_ID, out_tx)];
    let mut extra_listeners = Vec::new();
    for index in 0..extra_listener_count {
        let id = fanin_listener_id(index);
        let (extra_in_tx, extra_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, LANE_DEPTH);
        let (extra_out_tx, extra_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, LANE_DEPTH);
        egress_lanes.push((id, extra_out_tx));
        extra_listeners.push((
            id,
            TokioInterfaceSeam::new(id, extra_in_tx, notify_tx.clone(), extra_out_rx),
            extra_in_rx,
        ));
    }
    let egress = Egress::new(egress_lanes);
    let mut interfaces = match manifest.profile.wire.as_str() {
        "udp" => vec![udp_core::descriptor(
            TCP_INTERFACE_ID,
            udp_core::UDP_BITRATE_GUESS_BPS,
        )],
        _ => vec![tcp_core::descriptor(
            TCP_INTERFACE_ID,
            tcp_core::TCP_BITRATE_GUESS_BPS,
        )],
    };
    for (id, _, _) in &extra_listeners {
        interfaces.push(tcp_core::descriptor(*id, tcp_core::TCP_BITRATE_GUESS_BPS));
    }

    let (event_tx, event_rx) = mpsc::unbounded_channel::<Event>();
    let journal = move |journaled: Journaled<'_>| match journaled {
        Journaled::AnnounceHeard { destination, .. } => {
            let _ = event_tx.send(Event::Heard(destination));
        }
        Journaled::CommandSettled { id, settlement } => {
            let _ = event_tx.send(Event::Settled(id, settlement));
        }
        Journaled::Delivered(Delivery::Single(delivery)) => {
            let _ = event_tx.send(Event::Delivered(delivery.plaintext.len()));
        }
        Journaled::Delivered(Delivery::Link(delivery)) => {
            let _ = event_tx.send(Event::Delivered(delivery.plaintext.len()));
        }
        Journaled::LinkClosed { .. } => {
            let _ = event_tx.send(Event::Closed);
        }
        Journaled::LinkEstablished(_) => {
            let _ = event_tx.send(Event::LinkUp);
        }
        Journaled::ResourceReceived { data, .. } => {
            let _ = event_tx.send(Event::ResourceIn(data.len()));
        }
        Journaled::ResourceAssembled { total_size, .. } => {
            let _ = event_tx.send(Event::ResourceIn(total_size as usize));
        }
        Journaled::RequestReceived {
            link_id,
            request_id,
            data,
            ..
        } => {
            let wanted = msgpack_bin_payload(data)
                .get(..2)
                .map(|len| u16::from_be_bytes([len[0], len[1]]) as usize)
                .unwrap_or(0);
            let _ = event_tx.send(Event::Request {
                link_id,
                request_id,
                wanted,
            });
        }
        Journaled::ResponseReceived { data, .. } => {
            let _ = event_tx.send(Event::Response(msgpack_bin_payload(data).len()));
        }
        _ => {}
    };

    match role.as_str() {
        "responder" => {
            let mut in_lanes = vec![(TCP_INTERFACE_ID, in_rx)];
            let bound = if manifest.profile.topology == "relay" {
                let interface = TcpClientInterface::new_with_id(
                    TCP_INTERFACE_ID,
                    addr.clone(),
                    tcp_core::TCP_BITRATE_GUESS_BPS,
                    Duration::from_millis(100),
                );
                tokio::spawn(interface.run(seam));
                addr.clone()
            } else if manifest.profile.wire == "udp" {
                let (local, peer) = udp_halves(&addr);
                let interface = UdpInterface::bind_with_id(
                    TCP_INTERFACE_ID,
                    local,
                    peer,
                    udp_core::UDP_BITRATE_GUESS_BPS,
                )
                .await
                .expect("binds the scenario port");
                tokio::spawn(interface.run(seam));
                addr.clone()
            } else {
                let interface = BenchTcpListener::bind_with_id(
                    TCP_INTERFACE_ID,
                    addr.as_str(),
                    tcp_core::TCP_BITRATE_GUESS_BPS,
                )
                .await
                .expect("binds the scenario port");
                let bound = interface.local_addr().expect("bound address");
                tokio::spawn(interface.run(seam));
                let mut addresses = bound.to_string();
                for (id, extra_seam, extra_in_rx) in extra_listeners.drain(..) {
                    let extra = BenchTcpListener::bind_with_id(
                        id,
                        "127.0.0.1:0",
                        tcp_core::TCP_BITRATE_GUESS_BPS,
                    )
                    .await
                    .expect("binds an extra listener");
                    let extra_bound = extra.local_addr().expect("bound address");
                    tokio::spawn(extra.run(extra_seam));
                    in_lanes.push((id, extra_in_rx));
                    addresses.push('+');
                    addresses.push_str(&extra_bound.to_string());
                }
                addresses
            };
            tokio::spawn(run(
                engine,
                interfaces,
                vec![],
                TokioHost::new(),
                notify_rx,
                in_lanes,
                command_rx,
                egress,
                journal,
            ));
            println!("READY role=responder addr={bound}");
            let announce_every = Duration::from_millis(manifest.profile.announce_every_ms);
            let initiators = manifest.profile.initiator_count;
            if manifest.profile.mechanism == "churn" {
                respond_churn(destination, announce_every, command_tx, event_rx).await;
            } else if manifest.profile.mechanism == "request" {
                respond_request(
                    destination,
                    announce_every,
                    initiators,
                    command_tx,
                    event_rx,
                )
                .await;
            } else if manifest.profile.mechanism == "resource" {
                respond_resource(
                    destination,
                    announce_every,
                    initiators,
                    command_tx,
                    event_rx,
                )
                .await;
            } else {
                panic!(
                    "mechanism {:?} is not a hand-rolled responder",
                    manifest.profile.mechanism
                );
            }
        }
        "initiator" => {
            if manifest.profile.wire == "udp" {
                let (local, peer) = udp_halves(&addr);
                let interface = UdpInterface::bind_with_id(
                    TCP_INTERFACE_ID,
                    local,
                    peer,
                    udp_core::UDP_BITRATE_GUESS_BPS,
                )
                .await
                .expect("binds the scenario port");
                tokio::spawn(interface.run(seam));
            } else {
                let interface = TcpClientInterface::new_with_id(
                    TCP_INTERFACE_ID,
                    addr.clone(),
                    tcp_core::TCP_BITRATE_GUESS_BPS,
                    Duration::from_millis(100),
                );
                tokio::spawn(interface.run(seam));
            }
            tokio::spawn(run(
                engine,
                interfaces,
                vec![],
                TokioHost::new(),
                notify_rx,
                vec![(TCP_INTERFACE_ID, in_rx)],
                command_rx,
                egress,
                journal,
            ));
            println!("READY role=initiator");
            if manifest.profile.mechanism == "churn" {
                initiate_churn(&manifest.profile, duration, command_tx, event_rx).await;
            } else if manifest.profile.mechanism == "request" {
                initiate_request(&manifest.profile, duration, command_tx, event_rx).await;
            } else if manifest.profile.mechanism == "resource" {
                initiate_resource(&manifest.profile, duration, command_tx, event_rx).await;
            } else {
                panic!(
                    "mechanism {:?} is not a hand-rolled initiator",
                    manifest.profile.mechanism
                );
            }
            // Close settlement is engine-state, not wire-state: give the egress lane a
            // beat to flush the close frame, or the responder only learns via its 10s
            // stale reaper.
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        other => panic!("unknown role {other:?} — {usage}"),
    }
}

/// The single-, link-, and channel-firehose endpoints stood up through the high-level runtime: a
/// [`PrnsRecipe`] carrying one Single destination and the wires it runs over, built into a [`Prns`]
/// node by [`Prns::new`]. The engine, channels, lanes, and reactor the hand-roll below still spells
/// out are all assembled by the runtime; this end keeps only what is genuinely the app's — the
/// destination's address (to announce itself), the command handle, and the event stream. Because
/// `Prns::run` owns the reactor and is `!Send`, it is driven on this task in a `select!` against the
/// role's own firehose loop, which speaks to the node through the cloned [`TokioPrnsHandle`] handle.
///
/// `Prns::new` stands the engine up on `GrowableHeap`; the `fixed-storage` (`Esp32S3`) residence is
/// not yet a `Prns` knob, so the firehose endpoints always measure heap storage. The hand-rolled
/// request/resource/churn paths still honor `NodeStorage`.
async fn run_runtime_endpoint(manifest: &Manifest, role: &str, addr: &str, duration: Duration) {
    let mechanism = manifest.profile.mechanism.as_str();
    let announce_every = Duration::from_millis(manifest.profile.announce_every_ms);
    let initiators = manifest.profile.initiator_count;

    // The recipe borrows its destination names for the node's whole life, and the node lives as
    // long as its `run` loop is driven, so the manifest-derived aspect is promoted to 'static.
    let aspect: &'static str = Box::leak(manifest.name.clone().into_boxed_str());
    let aspects: &'static [&'static str] = Box::leak(Box::new([aspect]));
    let single = PreConfiguredDestination::Single {
        app_name: "bench",
        aspects,
        identity: fresh_identity(),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        ratchet: RatchetPolicy::NoRatchets,
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
            Some(_) => (build_bus_client_node(single, on_event), "shared".to_string()),
            None => build_responder_node(single, on_event, manifest, addr).await,
        };
        let commands = node.handle();
        if let Some(port) = shared_port {
            join_bus(&commands, port).await;
        }
        println!("READY role=responder addr={bound}");
        let firehose = async {
            if mechanism == "link" || mechanism == "channel" {
                respond_link(destination, announce_every, initiators, &commands, event_rx).await;
            } else {
                respond(destination, announce_every, &commands, event_rx).await;
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
        transport: None,
        pre_configured_destinations: [single],
        app_state: (),
        storage: NodeStorage::default(),
        routes: routes![],
        on_event,
        interfaces: interfaces![],
    })
}

async fn join_bus(commands: &TokioPrnsHandle, port: u16) {
    let role = commands
        .join_local_instance(LocalInstance {
            identity_dir: std::env::temp_dir(),
            ports: InstancePorts {
                bus: port,
                control: port + 1,
            },
            on_existing: OnExisting::JoinAsClient,
        })
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
        identity: fresh_identity(),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        ratchet: RatchetPolicy::NoRatchets,
    };
    if role == "responder" {
        let served = Arc::new(AtomicU64::new(0));
        let destination = single.destination_hash().expect("valid bench destination");
        let node = Prns::new(PrnsRecipe {
            transport: None,
            pre_configured_destinations: [single],
            app_state: RequestServed(Arc::clone(&served)),
            storage: NodeStorage::default(),
            routes: routes![BenchRequestRoute],
            on_event: |_event, _state| {},
            interfaces: interfaces![],
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

/// Build the responder's node: its listening wires fold straight into the recipe (a relayed client,
/// a UDP half, or a TCP server plus any fan-in listeners as a homogeneous `Vec`), and the bound
/// READY address line comes back beside it (the server address, plus fan-in listeners joined by
/// `+`). The interface kind differs per branch, but `Prns::new` erases it, so every arm yields the
/// same node type.
async fn build_responder_node<F>(
    single: PreConfiguredDestination<'static>,
    on_event: F,
    manifest: &Manifest,
    addr: &str,
) -> (Prns<(), (), F, NodeStorage>, String)
where
    F: FnMut(PrnsEvent<'_>, &()),
{
    if manifest.profile.topology == "relay" {
        let client = TcpClientInterface::new_with_id(
            TCP_INTERFACE_ID,
            addr.to_string(),
            tcp_core::TCP_BITRATE_GUESS_BPS,
            Duration::from_millis(100),
        );
        let node = Prns::new(PrnsRecipe {
            transport: None,
            pre_configured_destinations: [single],
            app_state: (),
            storage: NodeStorage::default(),
            routes: routes![],
            on_event,
            interfaces: interfaces![client],
        });
        (node, addr.to_string())
    } else if manifest.profile.wire == "udp" {
        let (local, peer) = udp_halves(addr);
        let udp = UdpInterface::bind_with_id(
            TCP_INTERFACE_ID,
            local,
            peer,
            udp_core::UDP_BITRATE_GUESS_BPS,
        )
        .await
        .expect("binds the scenario port");
        let node = Prns::new(PrnsRecipe {
            transport: None,
            pre_configured_destinations: [single],
            app_state: (),
            storage: NodeStorage::default(),
            routes: routes![],
            on_event,
            interfaces: interfaces![udp],
        });
        (node, addr.to_string())
    } else {
        let primary = BenchTcpListener::bind_with_id(
            TCP_INTERFACE_ID,
            addr,
            tcp_core::TCP_BITRATE_GUESS_BPS,
        )
        .await
        .expect("binds the scenario port");
        let mut addresses = primary.local_addr().expect("bound address").to_string();
        let mut servers = vec![primary];
        for index in 0..manifest.profile.initiator_count.saturating_sub(1) {
            let extra = BenchTcpListener::bind_with_id(
                fanin_listener_id(index),
                "127.0.0.1:0",
                tcp_core::TCP_BITRATE_GUESS_BPS,
            )
            .await
            .expect("binds an extra listener");
            addresses.push('+');
            addresses.push_str(&extra.local_addr().expect("bound address").to_string());
            servers.push(extra);
        }
        let node = Prns::new(PrnsRecipe {
            transport: None,
            pre_configured_destinations: [single],
            app_state: (),
            storage: NodeStorage::default(),
            routes: routes![],
            on_event,
            interfaces: servers,
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
            udp_core::UDP_BITRATE_GUESS_BPS,
        )
        .await
        .expect("binds the scenario port");
        Prns::new(PrnsRecipe {
            transport: None,
            pre_configured_destinations: [single],
            app_state: (),
            storage: NodeStorage::default(),
            routes: routes![],
            on_event,
            interfaces: interfaces![udp],
        })
    } else {
        let client = TcpClientInterface::new_with_id(
            TCP_INTERFACE_ID,
            addr.to_string(),
            tcp_core::TCP_BITRATE_GUESS_BPS,
            Duration::from_millis(100),
        );
        Prns::new(PrnsRecipe {
            transport: None,
            pre_configured_destinations: [single],
            app_state: (),
            storage: NodeStorage::default(),
            routes: routes![],
            on_event,
            interfaces: interfaces![client],
        })
    }
}

/// The proving end: announce on a cadence (ProveAll proves every single inside the
/// engine), count delivered payload bytes, and report once the firehose has been quiet —
/// singles have no teardown to signal the end with, so silence after traffic is it.
async fn respond(
    destination: DestinationHash,
    announce_every: Duration,
    commands: &TokioPrnsHandle,
    mut events: mpsc::UnboundedReceiver<Event>,
) {
    let mut announce = tokio::time::interval(announce_every);
    let mut idle = tokio::time::interval(Duration::from_millis(200));
    let mut delivered = 0u64;
    let mut payload_bytes = 0u64;
    let mut last_delivery: Option<tokio::time::Instant> = None;
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
            _ = idle.tick() => {
                if last_delivery.is_some_and(|at| at.elapsed() > QUIET_AFTER_TRAFFIC) {
                    println!("RESULT delivered={delivered} payload_bytes={payload_bytes}");
                    return;
                }
            }
            event = events.recv() => {
                match event {
                    Some(Event::Delivered(bytes)) => {
                        delivered += 1;
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
            if let Some(id) = commands.issue(EngineCommand::SendSingle(SendSingle {
                destination,
                payload: SendSinglePayload::from_slice(&scratch[..len]).expect("payload fits"),
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
        if let Event::Settled(id, Settlement::SendSingle(result)) = event {
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
    initiator_count: usize,
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
                        if links_up >= initiator_count {
                            announcing = false;
                        }
                    }
                    Some(Event::Delivered(bytes)) => {
                        delivered += 1;
                        payload_bytes += bytes as u64;
                    }
                    Some(Event::Closed) if closed_links + 1 < initiator_count => {
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
            if let Some(id) = commands.issue(EngineCommand::SendLink(SendLink {
                link_id,
                payload: SendLinkPayload::from_slice(&scratch[..len]).expect("payload fits"),
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
        if let Event::Settled(id, Settlement::SendLink(result)) = event {
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
            if let Some(id) = commands.issue(EngineCommand::SendChannel(SendChannel {
                link_id,
                message_type: BENCH_CHANNEL_MSGTYPE,
                body: SendChannelBody::from_slice(&scratch[..len]).expect("payload fits"),
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
        if let Event::Settled(id, Settlement::SendChannel(result)) = event {
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
                Err(SendChannelFailure::WindowFull) => {}
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

/// The accepting end: announce until the link arrives, open the strategy
/// gate for it, count every hash-proved transfer, and report when the
/// initiator closes the link.
async fn respond_resource(
    destination: DestinationHash,
    announce_every: Duration,
    initiator_count: usize,
    commands: mpsc::UnboundedSender<HostCommand>,
    mut events: mpsc::UnboundedReceiver<Event>,
) {
    let mut links_up = 0usize;
    let mut closed_links = 0usize;
    let mut next_id = 1u64;
    let mut announce = tokio::time::interval(announce_every);
    let mut announcing = true;
    let mut received = 0u64;
    let mut payload_bytes = 0u64;
    loop {
        tokio::select! {
            _ = announce.tick(), if announcing => {
                let command = IssuedCommand {
                    id: CommandId(next_id),
                    command: EngineCommand::AnnounceNow(AnnounceNow {
                        destination,
                        target: AnnounceTarget::AllInterfaces,
                        app_data: AnnounceAppData::Registered,
                    }),
                };
                next_id += 1;
                if commands.send(HostCommand::Engine(command)).is_err() {
                    return;
                }
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

/// The measuring end: establish one link, then send maximum-size resources
/// back to back — one at a time, the protocol's own rule — until the
/// wall-time elapses. Goodput from the settled transfers; per-transfer wall
/// time measured locally.
async fn initiate_resource(
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
    commands
        .send(HostCommand::Engine(IssuedCommand {
            id: CommandId(1),
            command: EngineCommand::EstablishLink(EstablishLink { destination }),
        }))
        .expect("reactor alive");
    let link_id = loop {
        match events.recv().await.expect("reactor alive") {
            Event::Settled(CommandId(1), Settlement::EstablishLink(Ok(established))) => {
                break established.link_id;
            }
            Event::Settled(CommandId(1), Settlement::EstablishLink(Err(failure))) => {
                panic!("link refused: {failure:?}");
            }
            _ => {}
        }
    };
    // One segment's worth of source bytes is all the initiator ever holds: every segment reads at
    // most MAX_EFFICIENT_SIZE, and the bytes are incompressible and proved per segment, so the same
    // buffer feeds every segment of every transfer. A real sender streams from a file the same way.
    let scratch: Arc<[u8]> = incompressible_payload(
        profile
            .payload_max
            .max(profile.payload_len)
            .min(MAX_EFFICIENT_SIZE),
    )
    .into();
    let mut sizes = SizeSequence::new(
        profile.size_seed,
        profile.payload_min,
        profile.payload_max,
        profile.payload_len,
    );
    let started = tokio::time::Instant::now();
    let deadline = started + duration;
    let mut next_id = 2u64;
    let mut sent = 0u64;
    let mut settled = 0u64;
    let mut failures = 0u64;
    let mut settled_bytes = 0u64;
    let mut transfer_ms: Vec<u64> = Vec::new();
    while tokio::time::Instant::now() < deadline {
        let len = sizes.next_len();
        let transfer_started = tokio::time::Instant::now();
        sent += 1;
        // One logical resource of `len` bytes, sent as MAX_EFFICIENT_SIZE segments — the
        // host half of TokioPrnsHandle::send_resource, awaiting each segment's proof before the
        // next so the engine holds one segment at a time. A payload at or under one segment
        // is a single unsplit transfer.
        let total_segments = (len as u64).div_ceil(MAX_EFFICIENT_SIZE as u64).max(1);
        let mut remaining = len;
        let mut transfer_ok = true;
        for segment_index in 1..=total_segments {
            let this_segment = remaining.min(MAX_EFFICIENT_SIZE);
            remaining -= this_segment;
            next_id += 1;
            let id = CommandId(next_id);
            let (completion, settled_rx) = oneshot::channel();
            commands
                .send(HostCommand::SendResourceSegment(
                    SendResourceSegmentHostCommand {
                        id,
                        link_id,
                        data: HostResourcePayload::shared_prefix(
                            Arc::clone(&scratch),
                            this_segment,
                        )
                        .expect("profile size stays within scratch"),
                        request_id: None,
                        segment_index,
                        total_segments,
                        total_data_size: len as u64,
                        completion,
                    },
                ))
                .expect("reactor alive");
            match settled_rx.await {
                Ok(Settlement::SendResource(Ok(()))) => {}
                Ok(Settlement::SendResource(Err(failure))) => {
                    eprintln!("transfer failed: {failure:?}");
                    transfer_ok = false;
                    break;
                }
                Ok(_) | Err(_) => {
                    transfer_ok = false;
                    break;
                }
            }
        }
        if transfer_ok {
            settled += 1;
            settled_bytes += len as u64;
            transfer_ms.push(transfer_started.elapsed().as_millis() as u64);
        } else {
            failures += 1;
        }
    }
    let elapsed_ms = started.elapsed().as_millis() as u64;

    commands
        .send(HostCommand::Engine(IssuedCommand {
            id: CommandId(next_id + 1),
            command: EngineCommand::CloseLink(CloseLink { link_id }),
        }))
        .expect("reactor alive");
    let close_deadline = tokio::time::Instant::now() + DRAIN_GRACE;
    loop {
        match tokio::time::timeout_at(close_deadline, events.recv()).await {
            Ok(Some(Event::Settled(_, Settlement::CloseLink(_)))) | Ok(None) | Err(_) => break,
            Ok(Some(_)) => {}
        }
    }

    transfer_ms.sort_unstable();
    let payload_bytes = settled_bytes;
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

/// The serving end of the RPC shape: a registered handler answers every
/// allowed request with exactly the byte count the request named — the
/// realistic query/answer pattern, sizes varied by the initiator.
async fn respond_request(
    destination: DestinationHash,
    announce_every: Duration,
    initiator_count: usize,
    commands: mpsc::UnboundedSender<HostCommand>,
    mut events: mpsc::UnboundedReceiver<Event>,
) {
    let mut links_up = 0usize;
    let mut closed_links = 0usize;
    let scratch = incompressible_payload(512);
    let mut framed = Vec::with_capacity(scratch.len() + 3);
    let mut next_id = 1u64;
    let mut announce = tokio::time::interval(announce_every);
    let mut announcing = true;
    let mut served = 0u64;
    let mut response_bytes = 0u64;
    loop {
        tokio::select! {
            _ = announce.tick(), if announcing => {
                let command = IssuedCommand {
                    id: CommandId(next_id),
                    command: EngineCommand::AnnounceNow(AnnounceNow {
                        destination,
                        target: AnnounceTarget::AllInterfaces,
                        app_data: AnnounceAppData::Registered,
                    }),
                };
                next_id += 1;
                if commands.send(HostCommand::Engine(command)).is_err() {
                    return;
                }
            }
            event = events.recv() => {
                match event {
                    Some(Event::LinkUp) => {
                        links_up += 1;
                        if links_up >= initiator_count {
                            announcing = false;
                        }
                    }
                    Some(Event::Request { link_id, request_id, wanted }) => {
                        next_id += 1;
                        let wanted = wanted.min(scratch.len());
                        let framed = msgpack_bin_into(&scratch[..wanted], &mut framed);
                        let respond = IssuedCommand {
                            id: CommandId(next_id),
                            command: EngineCommand::Respond(Respond {
                                link_id,
                                request_id,
                                data: RespondData::from_slice(framed).expect("response fits"),
                            }),
                        };
                        if commands.send(HostCommand::Engine(respond)).is_err() {
                            return;
                        }
                        served += 1;
                        response_bytes += wanted as u64;
                    }
                    Some(Event::Closed) if closed_links + 1 < initiator_count => {
                        closed_links += 1;
                    }
                    Some(Event::Closed) | None => {
                        println!("RESULT served={served} response_bytes={response_bytes}");
                        return;
                    }
                    Some(_) => {}
                }
            }
        }
    }
}

/// The asking end: one link, then `window` requests in flight until the
/// wall-time elapses — each request a varied size, each naming a varied
/// response size it wants back. Latency from the settled receipts.
async fn initiate_request(
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
    commands
        .send(HostCommand::Engine(IssuedCommand {
            id: CommandId(1),
            command: EngineCommand::EstablishLink(EstablishLink { destination }),
        }))
        .expect("reactor alive");
    let link_id = loop {
        match events.recv().await.expect("reactor alive") {
            Event::Settled(CommandId(1), Settlement::EstablishLink(Ok(established))) => {
                break established.link_id;
            }
            Event::Settled(CommandId(1), Settlement::EstablishLink(Err(failure))) => {
                panic!("link refused: {failure:?}");
            }
            _ => {}
        }
    };

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
    let mut next_id = 2u64;
    let mut sent = 0u64;
    let mut delivered = 0u64;
    let mut timeouts = 0u64;
    let mut in_flight = 0usize;
    let mut request_bytes = 0u64;
    let mut response_bytes = 0u64;
    let mut rtts: Vec<u64> = Vec::new();
    let mut framed = Vec::with_capacity(profile.request_max + 3);
    let mut send_one = |in_flight: &mut usize, sent: &mut u64, next_id: &mut u64| {
        let request_len = request_sizes.next_len();
        let wanted = response_sizes.next_len() as u16;
        begin_msgpack_bin(request_len, &mut framed);
        framed.extend_from_slice(&wanted.to_be_bytes());
        framed.extend_from_slice(&scratch[..request_len - 2]);
        request_bytes += request_len as u64;
        let command = IssuedCommand {
            id: CommandId(*next_id),
            command: EngineCommand::SendRequest(SendRequest {
                link_id,
                path_hash,
                data: SendRequestData::from_slice(&framed).expect("request fits"),
            }),
        };
        *next_id += 1;
        *sent += 1;
        *in_flight += 1;
        commands.send(HostCommand::Engine(command)).is_ok()
    };

    for _ in 0..profile.window {
        send_one(&mut in_flight, &mut sent, &mut next_id);
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
                    send_one(&mut in_flight, &mut sent, &mut next_id);
                }
            }
            Event::Response(bytes) => {
                response_bytes += bytes as u64;
            }
            _ => {}
        }
    }
    let elapsed_ms = started.elapsed().as_millis() as u64;

    commands
        .send(HostCommand::Engine(IssuedCommand {
            id: CommandId(next_id + 1),
            command: EngineCommand::CloseLink(CloseLink { link_id }),
        }))
        .expect("reactor alive");
    let close_deadline = tokio::time::Instant::now() + DRAIN_GRACE;
    loop {
        match tokio::time::timeout_at(close_deadline, events.recv()).await {
            Ok(Some(Event::Settled(_, Settlement::CloseLink(_)))) | Ok(None) | Err(_) => break,
            Ok(Some(_)) => {}
        }
    }

    rtts.sort_unstable();
    let seconds = (elapsed_ms as f64 / 1000.0).max(f64::EPSILON);
    println!(
        "RESULT sent={sent} delivered={delivered} timeouts={timeouts} \
         request_bytes={request_bytes} response_bytes={response_bytes} \
         elapsed_ms={elapsed_ms} requests_per_sec={:.1} \
         rtt_p50_ms={:.0} rtt_p99_ms={:.0}{} build={BUILD_PROFILE}",
        delivered as f64 / seconds,
        percentile(&rtts, 0.50),
        percentile(&rtts, 0.99),
        died_marker(died),
    );
}

/// A pure transport node: no destinations, no app — just the engine with its
/// transport identity, standing between two endpoints on two server
/// interfaces. Everything it does (announce rebroadcast with the transport
/// stamp, link request booking, blind ciphertext switching) is engine
/// machinery under test.
async fn relay_node(manifest: &Manifest) {
    let engine = EngineState::<NodeStorage>::new(fresh_identity());
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
        tcp_core::descriptor(TCP_INTERFACE_ID, tcp_core::TCP_BITRATE_GUESS_BPS),
        tcp_core::descriptor(RELAY_SECOND_INTERFACE_ID, tcp_core::TCP_BITRATE_GUESS_BPS),
    ];

    let side_a = BenchTcpListener::bind_with_id(
        TCP_INTERFACE_ID,
        "127.0.0.1:0",
        tcp_core::TCP_BITRATE_GUESS_BPS,
    )
    .await
    .expect("binds side a");
    let addr_a = side_a.local_addr().expect("bound address");
    let side_b = BenchTcpListener::bind_with_id(
        RELAY_SECOND_INTERFACE_ID,
        "127.0.0.1:0",
        tcp_core::TCP_BITRATE_GUESS_BPS,
    )
    .await
    .expect("binds side b");
    let addr_b = side_b.local_addr().expect("bound address");
    tokio::spawn(side_a.run(seam_a));
    tokio::spawn(side_b.run(seam_b));
    tokio::spawn(run(
        engine,
        interfaces,
        vec![],
        TokioHost::new(),
        notify_rx,
        vec![
            (TCP_INTERFACE_ID, in_a_rx),
            (RELAY_SECOND_INTERFACE_ID, in_b_rx),
        ],
        command_rx,
        egress,
        |_: Journaled<'_>| {},
    ));
    println!("READY role=relay addr={addr_a}>{addr_b}");
    std::future::pending::<()>().await;
}

/// A pure transport node in a trunk: listen for the next hop downstream, dial the
/// previous hop upstream, switch everything between them.
async fn chain_node(upstream: &str) {
    let engine = EngineState::<NodeStorage>::new(fresh_identity());

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
        tcp_core::descriptor(TCP_INTERFACE_ID, tcp_core::TCP_BITRATE_GUESS_BPS),
        tcp_core::descriptor(RELAY_SECOND_INTERFACE_ID, tcp_core::TCP_BITRATE_GUESS_BPS),
    ];

    let downstream = BenchTcpListener::bind_with_id(
        TCP_INTERFACE_ID,
        "127.0.0.1:0",
        tcp_core::TCP_BITRATE_GUESS_BPS,
    )
    .await
    .expect("binds downstream side");
    let addr = downstream.local_addr().expect("bound address");
    let up = TcpClientInterface::new_with_id(
        RELAY_SECOND_INTERFACE_ID,
        upstream.to_string(),
        tcp_core::TCP_BITRATE_GUESS_BPS,
        Duration::from_millis(100),
    );
    tokio::spawn(downstream.run(seam_down));
    tokio::spawn(up.run(seam_up));
    tokio::spawn(run(
        engine,
        interfaces,
        vec![],
        TokioHost::new(),
        notify_rx,
        vec![
            (TCP_INTERFACE_ID, in_down_rx),
            (RELAY_SECOND_INTERFACE_ID, in_up_rx),
        ],
        command_rx,
        egress,
        |_: Journaled<'_>| {},
    ));
    println!("READY role=chain addr={addr}");
    std::future::pending::<()>().await;
}

/// The serving end of session churn: every fresh link gets the strategy gate
/// opened, every delivery counted, and the report comes when the churn has
/// been quiet — closed links are the cycle's normal end, not the run's.
async fn respond_churn(
    destination: DestinationHash,
    announce_every: Duration,
    commands: mpsc::UnboundedSender<HostCommand>,
    mut events: mpsc::UnboundedReceiver<Event>,
) {
    let mut next_id = 1u64;
    let mut announce = tokio::time::interval(announce_every);
    let mut announcing = true;
    let mut idle = tokio::time::interval(Duration::from_millis(200));
    let mut received = 0u64;
    let mut payload_bytes = 0u64;
    let mut last_delivery: Option<tokio::time::Instant> = None;
    loop {
        tokio::select! {
            _ = announce.tick(), if announcing => {
                let command = IssuedCommand {
                    id: CommandId(next_id),
                    command: EngineCommand::AnnounceNow(AnnounceNow {
                        destination,
                        target: AnnounceTarget::AllInterfaces,
                        app_data: AnnounceAppData::Registered,
                    }),
                };
                next_id += 1;
                if commands.send(HostCommand::Engine(command)).is_err() {
                    return;
                }
            }
            _ = idle.tick() => {
                if last_delivery.is_some_and(|at| at.elapsed() > QUIET_AFTER_TRAFFIC) {
                    println!("RESULT received={received} payload_bytes={payload_bytes}");
                    return;
                }
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

/// The churning end: hear the announce once, then live whole sessions back
/// to back — establish, move one banded payload (command sends on the link,
/// pages and files as resources), tear down. The product is sessions per
/// second and where the time goes.
async fn initiate_churn(
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

    let scratch: Arc<[u8]> = incompressible_payload(profile.file_max.max(profile.page_max)).into();
    let mut sizes = SizeSequence::new(profile.size_seed, 0, 0, 1);
    let started = tokio::time::Instant::now();
    let deadline = started + duration;
    let mut next_id = 1u64;
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
        next_id += 1;
        let establish_id = CommandId(next_id);
        commands
            .send(HostCommand::Engine(IssuedCommand {
                id: establish_id,
                command: EngineCommand::EstablishLink(EstablishLink { destination }),
            }))
            .expect("reactor alive");
        let link_id = loop {
            match events.recv().await.expect("reactor alive") {
                Event::Settled(id, Settlement::EstablishLink(result)) if id == establish_id => {
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
                _ => {}
            }
        };
        establish_ms.push(cycle_started.elapsed().as_millis() as u64);

        let (band, len) = roll_band(&mut sizes, profile);
        let transfer_started = tokio::time::Instant::now();
        next_id += 1;
        let transfer_id = CommandId(next_id);
        let moved = match band {
            Band::Command => {
                commands
                    .send(HostCommand::Engine(IssuedCommand {
                        id: transfer_id,
                        command: EngineCommand::SendLink(SendLink {
                            link_id,
                            payload: SendLinkPayload::from_slice(&scratch[..len])
                                .expect("command fits"),
                        }),
                    }))
                    .expect("reactor alive");
                loop {
                    match events.recv().await.expect("reactor alive") {
                        Event::Settled(id, Settlement::SendLink(result)) if id == transfer_id => {
                            break result.is_ok();
                        }
                        _ => {}
                    }
                }
            }
            Band::Page | Band::File => {
                commands
                    .send(HostCommand::SendResource(SendResourceHostCommand {
                        id: transfer_id,
                        link_id,
                        data: HostResourcePayload::shared_prefix(Arc::clone(&scratch), len)
                            .expect("profile size stays within scratch"),
                        compressed_candidate: None,
                        request_id: None,
                    }))
                    .expect("reactor alive");
                loop {
                    match events.recv().await.expect("reactor alive") {
                        Event::Settled(id, Settlement::SendResource(result))
                            if id == transfer_id =>
                        {
                            break result.is_ok();
                        }
                        _ => {}
                    }
                }
            }
        };
        let transfer_elapsed = transfer_started.elapsed().as_millis() as u64;
        if moved {
            failure_streak = 0;
            payload_bytes += len as u64;
            match band {
                Band::Command => commands_moved += 1,
                Band::Page => pages_moved += 1,
                Band::File => files_moved += 1,
            }
            let band_index = match band {
                Band::Command => 0,
                Band::Page => 1,
                Band::File => 2,
            };
            transfer_ms_by_band[band_index].push(transfer_elapsed);
        } else {
            failures += 1;
            failure_streak += 1;
        }

        let close_started = tokio::time::Instant::now();
        next_id += 1;
        let close_id = CommandId(next_id);
        commands
            .send(HostCommand::Engine(IssuedCommand {
                id: close_id,
                command: EngineCommand::CloseLink(CloseLink { link_id }),
            }))
            .expect("reactor alive");
        loop {
            match events.recv().await.expect("reactor alive") {
                Event::Settled(id, Settlement::CloseLink(_)) if id == close_id => break,
                _ => {}
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
