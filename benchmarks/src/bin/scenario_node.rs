//! One implementation's *participation binary* for live scenarios: the whole cross-impl
//! contract is `scenario_node <manifest.json> <role> <addr> [duration-ms]` plus a line
//! protocol on stdout (`READY …`, then one final `RESULT k=v …`). The responder binds
//! `addr` (`127.0.0.1:0` lets the OS pick — the bound address comes back on its READY
//! line) and proves every delivery; the initiator connects and pumps windowed SINGLE
//! packets at the announced destination until the profile's wall-time elapses —
//! throughput from the settlement counts, latency straight from the proofs (`rtt_ms`).
//! Another implementation joins a pairing by speaking this same surface, nothing more.

use std::time::Duration;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, CloseLink, CommandId, EngineCommand, EngineState,
    EstablishLink, IssuedCommand, Journaled, RatchetPolicy, Respond, RespondData, SendLink,
    SendLinkPayload, SendRequest, SendRequestData, SendSingle, SendSinglePayload, Settlement,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::InterfaceId;
use personal_rns::reactor::impls::tokio_reactor::{
    run, tokio_grant_lane, Egress, HostCommand, SendResourceHostCommand, TokioHost,
    TokioInterfaceSeam,
};
use personal_rns::reactor::interface_seam::{Interface, MAX_WIRE_FRAME_LEN};
use personal_rns::reactor::interfaces::tcp::core as tcp_core;
use personal_rns::reactor::interfaces::tcp::impls::tokio::{
    TcpClientInterface, TcpServerInterface,
};
use personal_rns::reactor::interfaces::udp::core as udp_core;
use personal_rns::reactor::interfaces::udp::impls::tokio::UdpInterface;
use personal_rns::routing::delivery::Delivery;
use personal_rns::routing::links::request::RequestId;
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::links::LinkId;
use personal_rns::routing::request_handlers::{RequestPathHash, RequestPolicy};
use personal_rns::routing::storage::GrowableHeap;
use personal_rns::routing::ProofStrategy;
use personal_rns::wire::DestinationHash;
use tokio::sync::mpsc;

const TCP_INTERFACE_ID: InterfaceId = InterfaceId::new([0xBE; 16]);
const RELAY_SECOND_INTERFACE_ID: InterfaceId = InterfaceId::new([0xBF; 16]);
const LANE_DEPTH: usize = 64;
const ANNOUNCE_EVERY: Duration = Duration::from_millis(500);
const DRAIN_GRACE: Duration = Duration::from_secs(5);
const QUIET_AFTER_TRAFFIC: Duration = Duration::from_millis(1500);

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
fn msgpack_bin(payload: &[u8]) -> Vec<u8> {
    let mut framed = Vec::with_capacity(payload.len() + 3);
    if payload.len() <= 0xFF {
        framed.push(0xC4);
        framed.push(payload.len() as u8);
    } else {
        framed.push(0xC5);
        framed.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    }
    framed.extend_from_slice(payload);
    framed
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

#[tokio::main(worker_threads = 2)]
async fn main() {
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

    let mut engine = EngineState::<GrowableHeap>::new(fresh_identity());
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
                max_uncompressed_len: 2 * 1024 * 1024,
                accept_compressed: false,
            },
        ));
    }
    let (command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();
    let (notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
    let (in_tx, in_rx) = tokio_grant_lane::<MAX_WIRE_FRAME_LEN>(LANE_DEPTH);
    let (out_tx, out_rx) = tokio_grant_lane::<MAX_WIRE_FRAME_LEN>(LANE_DEPTH);
    let seam = TokioInterfaceSeam::new(TCP_INTERFACE_ID, in_tx, notify_tx, out_rx);
    let egress = Egress::new(vec![(TCP_INTERFACE_ID, out_tx)]);
    let interfaces = match manifest.profile.wire.as_str() {
        "udp" => vec![udp_core::descriptor(
            TCP_INTERFACE_ID,
            udp_core::UDP_BITRATE_GUESS_BPS,
        )],
        _ => vec![tcp_core::descriptor(
            TCP_INTERFACE_ID,
            tcp_core::TCP_BITRATE_GUESS_BPS,
        )],
    };

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

    if role == "relay" {
        relay_node(&manifest).await;
        return;
    }
    match role.as_str() {
        "responder" => {
            let bound = if manifest.profile.topology == "relay" {
                let interface = TcpClientInterface::new(
                    TCP_INTERFACE_ID,
                    addr.clone(),
                    tcp_core::TCP_BITRATE_GUESS_BPS,
                    Duration::from_millis(100),
                );
                tokio::spawn(interface.run(seam));
                addr.clone()
            } else if manifest.profile.wire == "udp" {
                let (local, peer) = udp_halves(&addr);
                let interface = UdpInterface::bind(
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
                let interface = TcpServerInterface::bind(
                    TCP_INTERFACE_ID,
                    addr.as_str(),
                    tcp_core::TCP_BITRATE_GUESS_BPS,
                )
                .await
                .expect("binds the scenario port");
                let bound = interface.local_addr().expect("bound address");
                tokio::spawn(interface.run(seam));
                bound.to_string()
            };
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
            println!("READY role=responder addr={bound}");
            if manifest.profile.mechanism == "churn" {
                respond_churn(destination, command_tx, event_rx).await;
            } else if manifest.profile.mechanism == "request" {
                respond_request(destination, command_tx, event_rx).await;
            } else if manifest.profile.mechanism == "resource" {
                respond_resource(destination, command_tx, event_rx).await;
            } else if manifest.profile.mechanism == "link" {
                respond_link(destination, command_tx, event_rx).await;
            } else {
                respond(destination, command_tx, event_rx).await;
            }
        }
        "initiator" => {
            if manifest.profile.wire == "udp" {
                let (local, peer) = udp_halves(&addr);
                let interface = UdpInterface::bind(
                    TCP_INTERFACE_ID,
                    local,
                    peer,
                    udp_core::UDP_BITRATE_GUESS_BPS,
                )
                .await
                .expect("binds the scenario port");
                tokio::spawn(interface.run(seam));
            } else {
                let interface = TcpClientInterface::new(
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
            } else if manifest.profile.mechanism == "link" {
                initiate_link(&manifest.profile, duration, command_tx, event_rx).await;
            } else {
                initiate(&manifest.profile, duration, command_tx, event_rx).await;
            }
        }
        other => panic!("unknown role {other:?} — {usage}"),
    }
}

/// The proving end: announce on a cadence (ProveAll proves every single inside the
/// engine), count delivered payload bytes, and report once the firehose has been quiet —
/// singles have no teardown to signal the end with, so silence after traffic is it.
async fn respond(
    destination: DestinationHash,
    commands: mpsc::UnboundedSender<HostCommand>,
    mut events: mpsc::UnboundedReceiver<Event>,
) {
    let mut next_id = 1u64;
    let mut announce = tokio::time::interval(ANNOUNCE_EVERY);
    let mut idle = tokio::time::interval(Duration::from_millis(200));
    let mut delivered = 0u64;
    let mut payload_bytes = 0u64;
    let mut last_delivery: Option<tokio::time::Instant> = None;
    loop {
        tokio::select! {
            _ = announce.tick() => {
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
    commands: mpsc::UnboundedSender<HostCommand>,
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
    let mut next_id = 1u64;
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
         next_id: &mut u64,
         sent_sizes: &mut std::collections::HashMap<u64, usize>| {
            let len = sizes.next_len();
            sent_sizes.insert(*next_id, len);
            let command = IssuedCommand {
                id: CommandId(*next_id),
                command: EngineCommand::SendSingle(SendSingle {
                    destination,
                    payload: SendSinglePayload::from_slice(&scratch[..len]).expect("payload fits"),
                }),
            };
            *next_id += 1;
            *sent += 1;
            *in_flight += 1;
            commands.send(HostCommand::Engine(command)).is_ok()
        };

    for _ in 0..profile.window {
        send_one(&mut in_flight, &mut sent, &mut next_id, &mut sent_sizes);
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
                    rtts.push(receipt.rtt_ms);
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
                send_one(&mut in_flight, &mut sent, &mut next_id, &mut sent_sizes);
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
         rtt_p50_ms={:.0} rtt_p99_ms={:.0}{}",
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
    commands: mpsc::UnboundedSender<HostCommand>,
    mut events: mpsc::UnboundedReceiver<Event>,
) {
    let mut next_id = 1u64;
    let mut announce = tokio::time::interval(ANNOUNCE_EVERY);
    let mut delivered = 0u64;
    let mut payload_bytes = 0u64;
    loop {
        tokio::select! {
            _ = announce.tick() => {
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
                    Some(Event::Delivered(bytes)) => {
                        delivered += 1;
                        payload_bytes += bytes as u64;
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

    let scratch = incompressible_payload(profile.payload_max.max(profile.payload_len));
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
    let mut delivered = 0u64;
    let mut timeouts = 0u64;
    let mut in_flight = 0usize;
    let mut sent_sizes: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    let mut delivered_bytes = 0u64;
    let mut rtts: Vec<u64> = Vec::new();
    let mut send_one =
        |in_flight: &mut usize,
         sent: &mut u64,
         next_id: &mut u64,
         sent_sizes: &mut std::collections::HashMap<u64, usize>| {
            let len = sizes.next_len();
            sent_sizes.insert(*next_id, len);
            let command = IssuedCommand {
                id: CommandId(*next_id),
                command: EngineCommand::SendLink(SendLink {
                    link_id,
                    payload: SendLinkPayload::from_slice(&scratch[..len]).expect("payload fits"),
                }),
            };
            *next_id += 1;
            *sent += 1;
            *in_flight += 1;
            commands.send(HostCommand::Engine(command)).is_ok()
        };

    for _ in 0..profile.window {
        send_one(&mut in_flight, &mut sent, &mut next_id, &mut sent_sizes);
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
                    rtts.push(receipt.rtt_ms);
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
                send_one(&mut in_flight, &mut sent, &mut next_id, &mut sent_sizes);
            }
        }
    }
    let elapsed_ms = started.elapsed().as_millis() as u64;

    commands
        .send(HostCommand::Engine(IssuedCommand {
            id: CommandId(next_id),
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
    let payload_bytes = delivered_bytes;
    let seconds = (elapsed_ms as f64 / 1000.0).max(f64::EPSILON);
    println!(
        "RESULT sent={sent} delivered={delivered} timeouts={timeouts} \
         payload_bytes={payload_bytes} elapsed_ms={elapsed_ms} \
         delivered_per_sec={:.1} goodput_bytes_per_sec={:.0} \
         rtt_p50_ms={:.0} rtt_p99_ms={:.0}{}",
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
    commands: mpsc::UnboundedSender<HostCommand>,
    mut events: mpsc::UnboundedReceiver<Event>,
) {
    let mut next_id = 1u64;
    let mut announce = tokio::time::interval(ANNOUNCE_EVERY);
    let mut received = 0u64;
    let mut payload_bytes = 0u64;
    loop {
        tokio::select! {
            _ = announce.tick() => {
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
                    Some(Event::ResourceIn(bytes)) => {
                        received += 1;
                        payload_bytes += bytes as u64;
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
    let scratch = incompressible_payload(profile.payload_max.max(profile.payload_len));
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
        next_id += 1;
        let id = CommandId(next_id);
        let len = sizes.next_len();
        let transfer_started = tokio::time::Instant::now();
        commands
            .send(HostCommand::SendResource(SendResourceHostCommand {
                id,
                link_id,
                data: scratch[..len].to_vec(),
                compressed_candidate: None,
                request_id: None,
            }))
            .expect("reactor alive");
        sent += 1;
        loop {
            match events.recv().await.expect("reactor alive") {
                Event::Settled(settled_id, Settlement::SendResource(result))
                    if settled_id == id =>
                {
                    match result {
                        Ok(()) => {
                            settled += 1;
                            settled_bytes += len as u64;
                            transfer_ms.push(transfer_started.elapsed().as_millis() as u64);
                        }
                        Err(failure) => {
                            eprintln!("transfer failed: {failure:?}");
                            failures += 1;
                        }
                    }
                    break;
                }
                _ => {}
            }
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
         transfer_p50_ms={:.0} transfer_p99_ms={:.0}",
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
    commands: mpsc::UnboundedSender<HostCommand>,
    mut events: mpsc::UnboundedReceiver<Event>,
) {
    let scratch = incompressible_payload(512);
    let mut next_id = 1u64;
    let mut announce = tokio::time::interval(ANNOUNCE_EVERY);
    let mut served = 0u64;
    let mut response_bytes = 0u64;
    loop {
        tokio::select! {
            _ = announce.tick() => {
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
                    Some(Event::Request { link_id, request_id, wanted }) => {
                        next_id += 1;
                        let wanted = wanted.min(scratch.len());
                        let framed = msgpack_bin(&scratch[..wanted]);
                        let respond = IssuedCommand {
                            id: CommandId(next_id),
                            command: EngineCommand::Respond(Respond {
                                link_id,
                                request_id,
                                data: RespondData::from_slice(&framed).expect("response fits"),
                            }),
                        };
                        if commands.send(HostCommand::Engine(respond)).is_err() {
                            return;
                        }
                        served += 1;
                        response_bytes += wanted as u64;
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
    let mut send_one = |in_flight: &mut usize, sent: &mut u64, next_id: &mut u64| {
        let request_len = request_sizes.next_len();
        let wanted = response_sizes.next_len() as u16;
        let mut data = Vec::with_capacity(request_len);
        data.extend_from_slice(&wanted.to_be_bytes());
        data.extend_from_slice(&scratch[..request_len - 2]);
        let framed = msgpack_bin(&data);
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
                        rtts.push(receipt.rtt_ms);
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
         rtt_p50_ms={:.0} rtt_p99_ms={:.0}{}",
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
    let engine = EngineState::<GrowableHeap>::new(fresh_identity());
    let _ = manifest;

    let (_command_tx, command_rx) = mpsc::unbounded_channel::<HostCommand>();
    let (notify_tx, notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
    let (in_a_tx, in_a_rx) = tokio_grant_lane::<MAX_WIRE_FRAME_LEN>(LANE_DEPTH);
    let (out_a_tx, out_a_rx) = tokio_grant_lane::<MAX_WIRE_FRAME_LEN>(LANE_DEPTH);
    let (in_b_tx, in_b_rx) = tokio_grant_lane::<MAX_WIRE_FRAME_LEN>(LANE_DEPTH);
    let (out_b_tx, out_b_rx) = tokio_grant_lane::<MAX_WIRE_FRAME_LEN>(LANE_DEPTH);
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

    let side_a = TcpServerInterface::bind(
        TCP_INTERFACE_ID,
        "127.0.0.1:0",
        tcp_core::TCP_BITRATE_GUESS_BPS,
    )
    .await
    .expect("binds side a");
    let addr_a = side_a.local_addr().expect("bound address");
    let side_b = TcpServerInterface::bind(
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

/// The serving end of session churn: every fresh link gets the strategy gate
/// opened, every delivery counted, and the report comes when the churn has
/// been quiet — closed links are the cycle's normal end, not the run's.
async fn respond_churn(
    destination: DestinationHash,
    commands: mpsc::UnboundedSender<HostCommand>,
    mut events: mpsc::UnboundedReceiver<Event>,
) {
    let mut next_id = 1u64;
    let mut announce = tokio::time::interval(ANNOUNCE_EVERY);
    let mut idle = tokio::time::interval(Duration::from_millis(200));
    let mut received = 0u64;
    let mut payload_bytes = 0u64;
    let mut last_delivery: Option<tokio::time::Instant> = None;
    loop {
        tokio::select! {
            _ = announce.tick() => {
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

    let scratch = incompressible_payload(profile.file_max.max(profile.page_max));
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
                        data: scratch[..len].to_vec(),
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
