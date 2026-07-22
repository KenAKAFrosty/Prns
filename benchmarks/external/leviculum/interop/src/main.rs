//! A leviculum (reticulum-std) participation node speaking the benchmark
//! harness's scenario-node contract:
//!
//!     leviculum-node <manifest.json> <responder|initiator> <addr> [duration-ms]
//!
//! then the stdout line protocol — `READY role=…` once it is bound/dialed, and
//! one final `RESULT k=v …`. It fields both interop mechanisms: `single`
//! (one-shot packets proven by the destination's PROVE_ALL strategy) and `link`
//! (a session the initiator establishes first). Behaviour is byte-comparable
//! with the Go node (`../../go-reticulum/interop/main.go`): the same seeded
//! size sequence, the same windowed firehose, the same RESULT fields.
//!
//! Built against the vendored upstream at ../.upstream/reticulum-std.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use serde::Deserialize;
use tokio::sync::mpsc;
use tokio::sync::Mutex;

use reticulum_core::DestinationHash;
use reticulum_std::driver::ReticulumNodeBuilder;
use reticulum_std::{
    generate_identity, Destination, DestinationType, Direction, NodeEvent, ProofStrategy,
};

#[derive(Debug, Default, Deserialize)]
struct Profile {
    #[serde(default)]
    mechanism: String,
    #[serde(default)]
    payload_len: i64,
    #[serde(default)]
    payload_min: i64,
    #[serde(default)]
    payload_max: i64,
    #[serde(default)]
    size_seed: u64,
    #[serde(default)]
    window: usize,
    #[serde(default)]
    duration_ms: u64,
}

#[derive(Debug, Default, Deserialize)]
struct Manifest {
    #[serde(default)]
    name: String,
    #[serde(default)]
    profile: Profile,
}

/// The varied-size law every node speaks identically: a seeded xorshift draws
/// each message's size in [min, max] — the same sequence the Go, Rust and
/// Python nodes draw, so byte totals stay comparable without exchanging
/// anything.
struct SizeSequence {
    state: u64,
    min: i64,
    max: i64,
}

impl SizeSequence {
    fn new(m: &Manifest) -> Self {
        let mut seed = m.profile.size_seed;
        if seed == 0 {
            seed = 0x5EEDCAFEF00D0001;
        }
        let (mut lo, mut hi) = (m.profile.payload_min, m.profile.payload_max);
        if hi == 0 {
            lo = m.profile.payload_len;
            hi = m.profile.payload_len;
        }
        SizeSequence {
            state: seed,
            min: lo,
            max: hi,
        }
    }

    fn next_len(&mut self) -> usize {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        let span = (self.max - self.min + 1) as u64;
        (self.min + (self.state % span) as i64) as usize
    }
}

fn free_port() -> u16 {
    match TcpListener::bind("127.0.0.1:0") {
        Ok(l) => l.local_addr().map(|a| a.port()).unwrap_or(45000),
        Err(_) => 45000,
    }
}

fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let mut rank = ((sorted.len() - 1) as f64 * p + 0.5) as usize;
    if rank >= sorted.len() {
        rank = sorted.len() - 1;
    }
    sorted[rank]
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        println!("RESULT error=usage");
        return;
    }
    let manifest_path = &args[0];
    let role = &args[1];
    let addr = &args[2];
    let duration_override: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);

    let raw = std::fs::read(manifest_path).unwrap_or_default();
    let m: Manifest = serde_json::from_slice(&raw).unwrap_or_default();

    if m.profile.mechanism != "single" && m.profile.mechanism != "link" {
        println!("RESULT error=unsupported-mechanism:{}", m.profile.mechanism);
        return;
    }

    let mut duration_ms = m.profile.duration_ms;
    if duration_override > 0 {
        duration_ms = duration_override;
    }
    let duration = Duration::from_millis(duration_ms);

    match role.as_str() {
        "responder" => responder(m).await,
        "initiator" => initiator(m, addr, duration).await,
        _ => println!("RESULT error=unknown-role"),
    }
}

async fn responder(m: Manifest) {
    let port = free_port();
    let listen: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();

    let mut node = match ReticulumNodeBuilder::new()
        .add_tcp_server(listen)
        .build()
        .await
    {
        Ok(n) => n,
        Err(e) => {
            println!("RESULT error=reticulum:{:?}", e);
            return;
        }
    };
    if let Err(e) = node.start().await {
        println!("RESULT error=start:{:?}", e);
        return;
    }
    let mut events = match node.take_event_receiver() {
        Some(rx) => rx,
        None => {
            println!("RESULT error=no-event-receiver");
            return;
        }
    };

    let is_link = m.profile.mechanism == "link";
    let id = generate_identity();
    let mut dest = match Destination::new(
        Some(id),
        Direction::In,
        DestinationType::Single,
        "bench",
        &[m.name.as_str()],
    ) {
        Ok(d) => d,
        Err(e) => {
            println!("RESULT error=destination:{:?}", e);
            return;
        }
    };
    dest.set_proof_strategy(ProofStrategy::All);
    if is_link {
        dest.set_accepts_links(true);
    }
    let dest_hash = *dest.hash();
    node.register_destination(dest);

    println!("READY role=responder addr=127.0.0.1:{}", port);

    let node = Arc::new(node);
    let mut delivered: u64 = 0;
    let mut payload_bytes: u64 = 0;
    let mut last_delivery: Option<Instant> = None;
    // Track the accepted link id so we count only its data and report on close.
    let mut accepted_link = false;

    // Announce on its own task, and only until the first delivery lands — once a
    // packet arrives the initiator has clearly heard us. Keeping it off the
    // report loop is what makes the single mechanism terminate: the announce
    // dispatch can block once the peer disconnects and the send buffer fills,
    // and if that ran on the select loop it would stall the idle check that
    // detects the quiet and reports. (Link reports on close, so it never hit
    // this, but we keep the same structure for both.)
    let (stop_announce_tx, mut stop_announce_rx) = mpsc::channel::<()>(1);
    let announce_node = Arc::clone(&node);
    let announce_hash = dest_hash;
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(500));
        loop {
            tokio::select! {
                _ = stop_announce_rx.recv() => return,
                _ = ticker.tick() => {
                    let _ = announce_node.announce_destination(&announce_hash, None).await;
                }
            }
        }
    });

    let mut idle = tokio::time::interval(Duration::from_millis(200));
    let report = |delivered: u64, payload_bytes: u64| {
        println!(
            "RESULT delivered={} payload_bytes={}",
            delivered, payload_bytes
        );
    };

    loop {
        tokio::select! {
            maybe_event = events.recv() => {
                let event = match maybe_event {
                    Some(e) => e,
                    None => {
                        report(delivered, payload_bytes);
                        return;
                    }
                };
                match event {
                    NodeEvent::PacketReceived { destination, data, .. } if !is_link => {
                        if destination == dest_hash {
                            delivered += 1;
                            payload_bytes += data.len() as u64;
                            last_delivery = Some(Instant::now());
                            if delivered == 1 {
                                let _ = stop_announce_tx.try_send(());
                            }
                        }
                    }
                    NodeEvent::LinkRequest { link_id, destination_hash, .. } if is_link => {
                        if destination_hash == dest_hash {
                            let _ = node.accept_link(&link_id).await;
                            accepted_link = true;
                        }
                    }
                    // LinkHandle.send() uses the Channel multiplexer
                    // (PacketContext::Channel), so the peer receives the payload
                    // as MessageReceived; plain (non-channel) link data arrives
                    // as LinkDataReceived. Count both so we stay robust.
                    NodeEvent::MessageReceived { data, .. } if is_link => {
                        delivered += 1;
                        payload_bytes += data.len() as u64;
                        last_delivery = Some(Instant::now());
                        if delivered == 1 {
                            let _ = stop_announce_tx.try_send(());
                        }
                    }
                    NodeEvent::LinkDataReceived { data, .. } if is_link => {
                        delivered += 1;
                        payload_bytes += data.len() as u64;
                        last_delivery = Some(Instant::now());
                        if delivered == 1 {
                            let _ = stop_announce_tx.try_send(());
                        }
                    }
                    NodeEvent::LinkClosed { .. } if is_link && accepted_link => {
                        report(delivered, payload_bytes);
                        return;
                    }
                    _ => {}
                }
            }
            _ = idle.tick() => {
                // Single: report after a quiet period with no delivery.
                if !is_link {
                    if let Some(last) = last_delivery {
                        if last.elapsed() > Duration::from_millis(1500) {
                            report(delivered, payload_bytes);
                            return;
                        }
                    }
                }
            }
        }
    }
}

async fn initiator(m: Manifest, addr: &str, duration: Duration) {
    let target: SocketAddr = match addr.parse() {
        Ok(a) => a,
        Err(_) => {
            println!("RESULT error=bad-addr");
            return;
        }
    };

    let mut node = match ReticulumNodeBuilder::new()
        .add_tcp_client(target)
        .build()
        .await
    {
        Ok(n) => n,
        Err(e) => {
            println!("RESULT error=reticulum:{:?}", e);
            return;
        }
    };
    if let Err(e) = node.start().await {
        println!("RESULT error=start:{:?}", e);
        return;
    }
    let mut events = match node.take_event_receiver() {
        Some(rx) => rx,
        None => {
            println!("RESULT error=no-event-receiver");
            return;
        }
    };

    println!("READY role=initiator");

    let is_link = m.profile.mechanism == "link";
    let empty_result = "RESULT sent=0 delivered=0 timeouts=0 payload_bytes=0 elapsed_ms=0 delivered_per_sec=0.0 goodput_bytes_per_sec=0 rtt_p50_ms=0 rtt_p99_ms=0";

    // Wait for the responder's announce — it yields the destination hash, the
    // Ed25519 signing key (for link connect) and the full identity public key.
    let (dest_hash, signing_key, public_key) = match wait_for_announce(&mut events).await {
        Some(v) => v,
        None => {
            println!("{}", empty_result);
            return;
        }
    };

    // For singles, register the responder's destination locally with its
    // public identity. leviculum verifies single-packet delivery proofs against
    // a *locally registered* destination's identity (node/mod.rs ProofReceived),
    // not the stored remote identity from the announce — so without this the
    // initiator never confirms any delivery. The reconstructed destination has
    // the same name+identity, hence the same hash, as the responder's.
    if !is_link {
        if let Ok(remote_id) = reticulum_core::Identity::from_public_key_bytes(&public_key) {
            if let Ok(remote_dest) = Destination::new(
                Some(remote_id),
                Direction::In,
                DestinationType::Single,
                "bench",
                &[m.name.as_str()],
            ) {
                if *remote_dest.hash() == dest_hash {
                    node.register_destination(remote_dest);
                }
            }
        }
    }

    let node = Arc::new(node);

    let mut sizes = SizeSequence::new(&m);
    let scratch_len = m.profile.payload_max.max(m.profile.payload_len).max(1) as usize;
    let scratch = vec![0xABu8; scratch_len];

    let window = m.profile.window.max(1);

    let mut sent: u64 = 0;
    let mut delivered: u64 = 0;
    let mut timeouts: u64 = 0;
    let mut delivered_bytes: u64 = 0;
    let mut rtts: Vec<u64> = Vec::new();

    let started = Instant::now();
    let deadline = started + duration;
    let drain_deadline = deadline + Duration::from_secs(5);

    if is_link {
        // Establish the link first, then fire windowed sends over it.
        let link = match node.connect(&dest_hash, &signing_key).await {
            Ok(h) => h,
            Err(e) => {
                println!("RESULT error=link:{:?}", e);
                return;
            }
        };
        let link_id = *link.link_id();
        // Wait for LinkEstablished (initiator side) before sending.
        let established = wait_for_link_established(&mut events, &link_id).await;
        if !established {
            println!("{}", empty_result);
            return;
        }
        let link = Arc::new(link);

        // FIFO of (send_time, size) for outstanding link messages. The channel
        // delivers in order, so LinkDeliveryConfirmed events match sends FIFO.
        let outstanding: Arc<Mutex<VecDeque<(Instant, u64)>>> =
            Arc::new(Mutex::new(VecDeque::new()));
        let mut in_flight: usize = 0;

        // Each send is spawned so a Busy/pacing stall on one message doesn't
        // block the firehose loop. The send_time is captured at enqueue.
        let send_one = |sizes: &mut SizeSequence,
                        sent: &mut u64,
                        outstanding: &Arc<Mutex<VecDeque<(Instant, u64)>>>,
                        link: &Arc<reticulum_std::LinkHandle>,
                        scratch: &[u8]| {
            let size = sizes.next_len();
            *sent += 1;
            let now = Instant::now();
            let outstanding = Arc::clone(outstanding);
            let link = Arc::clone(link);
            let data = scratch[..size].to_vec();
            let sz = size as u64;
            tokio::spawn(async move {
                {
                    let mut q = outstanding.lock().await;
                    q.push_back((now, sz));
                }
                let _ = link.send(&data).await;
            });
        };

        for _ in 0..window {
            send_one(&mut sizes, &mut sent, &outstanding, &link, &scratch);
            in_flight += 1;
        }

        loop {
            if in_flight == 0 {
                break;
            }
            let now = Instant::now();
            if now >= drain_deadline {
                break;
            }
            let wait = drain_deadline - now;
            let evt = tokio::time::timeout(wait, events.recv()).await;
            match evt {
                Err(_) => break,
                Ok(None) => break,
                Ok(Some(event)) => match event {
                    NodeEvent::LinkDeliveryConfirmed { link_id: lid, .. } if lid == link_id => {
                        in_flight -= 1;
                        let entry = { outstanding.lock().await.pop_front() };
                        if let Some((send_time, size)) = entry {
                            delivered += 1;
                            delivered_bytes += size;
                            rtts.push(send_time.elapsed().as_millis() as u64);
                        } else {
                            delivered += 1;
                        }
                        if Instant::now() < deadline {
                            send_one(&mut sizes, &mut sent, &outstanding, &link, &scratch);
                            in_flight += 1;
                        }
                    }
                    NodeEvent::LinkClosed { link_id: lid, .. } if lid == link_id => {
                        break;
                    }
                    _ => {}
                },
            }
        }

        let elapsed_ms = started.elapsed().as_millis() as u64;
        // Tear the link down so the responder's LinkClosed fires and it reports.
        let _ = node.close_link(&link_id).await;
        drop(link);
        emit_result(
            sent,
            delivered,
            timeouts,
            delivered_bytes,
            elapsed_ms,
            &mut rtts,
        );
        return;
    }

    // single mechanism: windowed send_single_packet with proof-tracked delivery.
    // Map each send's packet_hash -> (send_time, size). Confirmation comes via
    // PacketDeliveryConfirmed, failure via DeliveryFailed.
    let mut pending: HashMap<[u8; 16], (Instant, u64)> = HashMap::new();
    let mut in_flight: usize = 0;

    for _ in 0..window {
        if send_single(
            &node,
            &dest_hash,
            &mut sizes,
            &scratch,
            &mut sent,
            &mut pending,
        )
        .await
        {
            in_flight += 1;
        }
    }

    loop {
        if in_flight == 0 {
            break;
        }
        let now = Instant::now();
        if now >= drain_deadline {
            break;
        }
        let wait = drain_deadline - now;
        let evt = tokio::time::timeout(wait, events.recv()).await;
        match evt {
            Err(_) => break,
            Ok(None) => break,
            Ok(Some(event)) => match event {
                NodeEvent::PacketDeliveryConfirmed { packet_hash } => {
                    if let Some((send_time, size)) = pending.remove(&packet_hash) {
                        in_flight -= 1;
                        delivered += 1;
                        delivered_bytes += size;
                        rtts.push(send_time.elapsed().as_millis() as u64);
                        if Instant::now() < deadline {
                            if send_single(
                                &node,
                                &dest_hash,
                                &mut sizes,
                                &scratch,
                                &mut sent,
                                &mut pending,
                            )
                            .await
                            {
                                in_flight += 1;
                            }
                        }
                    }
                }
                NodeEvent::DeliveryFailed { packet_hash, .. } => {
                    if pending.remove(&packet_hash).is_some() {
                        in_flight -= 1;
                        timeouts += 1;
                        if Instant::now() < deadline {
                            if send_single(
                                &node,
                                &dest_hash,
                                &mut sizes,
                                &scratch,
                                &mut sent,
                                &mut pending,
                            )
                            .await
                            {
                                in_flight += 1;
                            }
                        }
                    }
                }
                _ => {}
            },
        }
    }

    let elapsed_ms = started.elapsed().as_millis() as u64;
    emit_result(
        sent,
        delivered,
        timeouts,
        delivered_bytes,
        elapsed_ms,
        &mut rtts,
    );
}

async fn send_single(
    node: &Arc<reticulum_std::ReticulumNode>,
    dest_hash: &DestinationHash,
    sizes: &mut SizeSequence,
    scratch: &[u8],
    sent: &mut u64,
    pending: &mut HashMap<[u8; 16], (Instant, u64)>,
) -> bool {
    let size = sizes.next_len();
    let now = Instant::now();
    match node.send_single_packet(dest_hash, &scratch[..size]).await {
        Ok(packet_hash) => {
            *sent += 1;
            pending.insert(packet_hash, (now, size as u64));
            true
        }
        Err(_) => false,
    }
}

async fn wait_for_announce(
    events: &mut mpsc::Receiver<NodeEvent>,
) -> Option<(DestinationHash, [u8; 32], [u8; 64])> {
    // We connect to a single responder; the first valid announce that carries a
    // full identity key is the one we want. Any announce on this point-to-point
    // link is from our peer.
    let timeout = Duration::from_secs(20);
    loop {
        let evt = tokio::time::timeout(timeout, events.recv()).await;
        match evt {
            Err(_) => return None,
            Ok(None) => return None,
            Ok(Some(NodeEvent::AnnounceReceived { announce, .. })) => {
                let dest_hash = *announce.destination_hash();
                let pk = *announce.public_key();
                // public_key = X25519(0..32) + Ed25519(32..64); connect/link
                // need the Ed25519 signing (verifying) key.
                let mut signing = [0u8; 32];
                signing.copy_from_slice(&pk[32..64]);
                return Some((dest_hash, signing, pk));
            }
            Ok(Some(_)) => {}
        }
    }
}

async fn wait_for_link_established(
    events: &mut mpsc::Receiver<NodeEvent>,
    link_id: &reticulum_core::link::LinkId,
) -> bool {
    let timeout = Duration::from_secs(10);
    loop {
        let evt = tokio::time::timeout(timeout, events.recv()).await;
        match evt {
            Err(_) => return false,
            Ok(None) => return false,
            Ok(Some(NodeEvent::LinkEstablished { link_id: lid, .. })) if &lid == link_id => {
                return true;
            }
            Ok(Some(NodeEvent::LinkClosed { link_id: lid, .. })) if &lid == link_id => {
                return false;
            }
            Ok(Some(_)) => {}
        }
    }
}

fn emit_result(
    sent: u64,
    delivered: u64,
    timeouts: u64,
    delivered_bytes: u64,
    elapsed_ms: u64,
    rtts: &mut [u64],
) {
    rtts.sort_unstable();
    let payload_bytes = delivered_bytes;
    let mut seconds = elapsed_ms as f64 / 1000.0;
    if seconds <= 0.0 {
        seconds = 0.001;
    }
    println!(
        "RESULT sent={} delivered={} timeouts={} payload_bytes={} elapsed_ms={} delivered_per_sec={:.1} goodput_bytes_per_sec={:.0} rtt_p50_ms={} rtt_p99_ms={}",
        sent,
        delivered,
        timeouts,
        payload_bytes,
        elapsed_ms,
        delivered as f64 / seconds,
        payload_bytes as f64 / seconds,
        percentile(rtts, 0.50),
        percentile(rtts, 0.99)
    );
}
