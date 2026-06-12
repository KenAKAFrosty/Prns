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
    EstablishLink, IssuedCommand, Journaled, RatchetPolicy, SendLink, SendLinkPayload, SendSingle,
    SendSinglePayload, SetResourceStrategy, Settlement,
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
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::links::LinkId;
use personal_rns::routing::storage::GrowableHeap;
use personal_rns::routing::ProofStrategy;
use personal_rns::wire::DestinationHash;
use tokio::sync::mpsc;

const TCP_INTERFACE_ID: InterfaceId = InterfaceId::new([0xBE; 16]);
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
    payload_len: usize,
    window: usize,
    duration_ms: u64,
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
    LinkUp(LinkId),
    ResourceIn(usize),
    Closed,
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
        Journaled::LinkEstablished(established) => {
            let _ = event_tx.send(Event::LinkUp(established.link_id));
        }
        Journaled::ResourceReceived { data, .. } => {
            let _ = event_tx.send(Event::ResourceIn(data.len()));
        }
        _ => {}
    };

    match role.as_str() {
        "responder" => {
            let bound = if manifest.profile.wire == "udp" {
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
            if manifest.profile.mechanism == "resource" {
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
            if manifest.profile.mechanism == "resource" {
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

    let payload = vec![0xAB; profile.payload_len];
    let started = tokio::time::Instant::now();
    let deadline = started + duration;
    let mut next_id = 1u64;
    let mut sent = 0u64;
    let mut delivered = 0u64;
    let mut timeouts = 0u64;
    let mut in_flight = 0usize;
    let mut rtts: Vec<u64> = Vec::new();
    let send_one = |in_flight: &mut usize, sent: &mut u64, next_id: &mut u64| {
        let command = IssuedCommand {
            id: CommandId(*next_id),
            command: EngineCommand::SendSingle(SendSingle {
                destination,
                payload: SendSinglePayload::from_slice(&payload).expect("payload fits"),
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
    while in_flight > 0 {
        let event = tokio::time::timeout_at(drain_deadline, events.recv()).await;
        let Ok(Some(event)) = event else { break };
        if let Event::Settled(_, Settlement::SendSingle(result)) = event {
            in_flight -= 1;
            match result {
                Ok(receipt) => {
                    delivered += 1;
                    rtts.push(receipt.rtt_ms);
                }
                Err(_) => timeouts += 1,
            }
            if tokio::time::Instant::now() < deadline {
                send_one(&mut in_flight, &mut sent, &mut next_id);
            }
        }
    }
    let elapsed_ms = started.elapsed().as_millis() as u64;

    rtts.sort_unstable();
    let payload_bytes = delivered * profile.payload_len as u64;
    let seconds = (elapsed_ms as f64 / 1000.0).max(f64::EPSILON);
    println!(
        "RESULT sent={sent} delivered={delivered} timeouts={timeouts} \
         payload_bytes={payload_bytes} elapsed_ms={elapsed_ms} \
         delivered_per_sec={:.1} goodput_bytes_per_sec={:.0} \
         rtt_p50_ms={:.0} rtt_p99_ms={:.0}",
        delivered as f64 / seconds,
        payload_bytes as f64 / seconds,
        percentile(&rtts, 0.50),
        percentile(&rtts, 0.99),
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

    let payload = vec![0xAB; profile.payload_len];
    let started = tokio::time::Instant::now();
    let deadline = started + duration;
    let mut next_id = 2u64;
    let mut sent = 0u64;
    let mut delivered = 0u64;
    let mut timeouts = 0u64;
    let mut in_flight = 0usize;
    let mut rtts: Vec<u64> = Vec::new();
    let send_one = |in_flight: &mut usize, sent: &mut u64, next_id: &mut u64| {
        let command = IssuedCommand {
            id: CommandId(*next_id),
            command: EngineCommand::SendLink(SendLink {
                link_id,
                payload: SendLinkPayload::from_slice(&payload).expect("payload fits"),
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
    while in_flight > 0 {
        let event = tokio::time::timeout_at(drain_deadline, events.recv()).await;
        let Ok(Some(event)) = event else { break };
        if let Event::Settled(_, Settlement::SendLink(result)) = event {
            in_flight -= 1;
            match result {
                Ok(receipt) => {
                    delivered += 1;
                    rtts.push(receipt.rtt_ms);
                }
                Err(_) => timeouts += 1,
            }
            if tokio::time::Instant::now() < deadline {
                send_one(&mut in_flight, &mut sent, &mut next_id);
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
    let payload_bytes = delivered * profile.payload_len as u64;
    let seconds = (elapsed_ms as f64 / 1000.0).max(f64::EPSILON);
    println!(
        "RESULT sent={sent} delivered={delivered} timeouts={timeouts} \
         payload_bytes={payload_bytes} elapsed_ms={elapsed_ms} \
         delivered_per_sec={:.1} goodput_bytes_per_sec={:.0} \
         rtt_p50_ms={:.0} rtt_p99_ms={:.0}",
        delivered as f64 / seconds,
        payload_bytes as f64 / seconds,
        percentile(&rtts, 0.50),
        percentile(&rtts, 0.99),
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
                    Some(Event::LinkUp(link_id)) => {
                        next_id += 1;
                        let open_gate = IssuedCommand {
                            id: CommandId(next_id),
                            command: EngineCommand::SetResourceStrategy(SetResourceStrategy {
                                link_id,
                                strategy: ResourceStrategy::Accept {
                                    max_uncompressed_len: 2 * 1024 * 1024,
                                    accept_compressed: false,
                                },
                            }),
                        };
                        if commands.send(HostCommand::Engine(open_gate)).is_err() {
                            return;
                        }
                    }
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
    // The responder opens its strategy gate on hearing the link; give that
    // command a moment to land before the first advertisement races it.
    tokio::time::sleep(Duration::from_millis(150)).await;

    let data = incompressible_payload(profile.payload_len);
    let started = tokio::time::Instant::now();
    let deadline = started + duration;
    let mut next_id = 2u64;
    let mut sent = 0u64;
    let mut settled = 0u64;
    let mut failures = 0u64;
    let mut transfer_ms: Vec<u64> = Vec::new();
    while tokio::time::Instant::now() < deadline {
        next_id += 1;
        let id = CommandId(next_id);
        let transfer_started = tokio::time::Instant::now();
        commands
            .send(HostCommand::SendResource(SendResourceHostCommand {
                id,
                link_id,
                data: data.clone(),
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
    let payload_bytes = settled * profile.payload_len as u64;
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
