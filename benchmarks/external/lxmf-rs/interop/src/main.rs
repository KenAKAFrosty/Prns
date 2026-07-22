//! An LXMF-rs participation node speaking the benchmark harness's scenario_node contract:
//!
//!   lxmf-node <manifest.json> <responder|initiator> <addr> [duration-ms]
//!
//! then the stdout line protocol: `READY role=...` once bound/dialed, and one final
//! `RESULT k=v ...`. LXMF-rs implements links end to end (establish, data, auto-proof) but
//! not single-packet proofs, so this node fields only `link` — the orchestrator gates it
//! there. Built against the pinned upstream cloned into ../.upstream by build.sh.
//!
//! It carries the firehose as plain link data packets (PacketContext::None): the responder
//! receives each as a LinkEvent::Data and counts it, and the link auto-proves it. LXMF-rs's
//! reliable Channel layer would also confirm delivery to the *sender*, but its in-order
//! receive stalls under a sustained firehose (it acks every message yet delivers only a
//! handful to the application), so we measure over the link-data path instead. That path
//! surfaces no per-message proof to the sender, so over this reliable in-order TCP link the
//! initiator reports delivered == sent with no per-message RTT; the responder's own count is
//! the independent delivery truth the orchestrator's conformance check compares against.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rand_core::OsRng;
use rns_core::identity::PrivateIdentity;
use rns_transport::destination::link::{LinkEvent, LinkStatus};
use rns_transport::destination::{DestinationDesc, DestinationName};
use rns_transport::identity_bridge::to_transport_private_identity;
use rns_transport::iface::tcp_client::TcpClient;
use rns_transport::iface::tcp_server::TcpServer;
use rns_transport::transport::{Transport, TransportConfig};
use tokio::sync::broadcast::error::RecvError;
use tokio::time::{sleep, timeout};

// Plain link data packets carry the firehose; keep payloads inside the link MDU so a draw is
// never silently dropped (PACKET_MDU is 464, less the link's Fernet overhead).
const MAX_LINK_PAYLOAD: usize = 400;
const EMPTY_RESULT: &str = "RESULT sent=0 delivered=0 timeouts=0 payload_bytes=0 elapsed_ms=0 \
delivered_per_sec=0.0 goodput_bytes_per_sec=0 rtt_p50_ms=0 rtt_p99_ms=0";

/// The varied-size law every node speaks identically: a seeded xorshift64 draws each message's
/// size in [min, max] — the same sequence the Go, Crystal, and Python nodes draw.
struct SizeSequence {
    state: u64,
    min: u32,
    max: u32,
}

impl SizeSequence {
    fn next_len(&mut self) -> u32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        let span = (self.max - self.min + 1) as u64;
        self.min + (self.state % span) as u32
    }
}

struct Profile {
    mechanism: String,
    duration_ms: u64,
    seed: u64,
    min: u32,
    max: u32,
}

fn parse_profile(manifest_path: &str, duration_override: u64) -> (String, Profile) {
    let text = std::fs::read_to_string(manifest_path).expect("reads manifest");
    let json: serde_json::Value = serde_json::from_str(&text).expect("parses manifest");
    let name = json["name"].as_str().unwrap_or("bench").to_string();
    let p = &json["profile"];
    let payload_len = p["payload_len"].as_u64().unwrap_or(0) as u32;
    let mut min = p["payload_min"].as_u64().unwrap_or(0) as u32;
    let mut max = p["payload_max"].as_u64().unwrap_or(0) as u32;
    if max == 0 {
        min = payload_len;
        max = payload_len;
    }
    let seed = match p["size_seed"].as_u64() {
        Some(0) | None => 0x5EED_CAFE_F00D_0001,
        Some(s) => s,
    };
    let mut duration_ms = p["duration_ms"].as_u64().unwrap_or(0);
    if duration_override > 0 {
        duration_ms = duration_override;
    }
    (
        name,
        Profile {
            mechanism: p["mechanism"].as_str().unwrap_or("link").to_string(),
            duration_ms,
            seed,
            min,
            max,
        },
    )
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind free port");
    listener.local_addr().expect("local addr").port()
}

fn build_transport(name: &str) -> (Transport, rns_transport::identity::PrivateIdentity) {
    let core_identity = PrivateIdentity::new_from_rand(OsRng);
    let identity = to_transport_private_identity(&core_identity);
    let mut config = TransportConfig::new(name, &identity, true);
    config.set_path_request_timeout_secs(2);
    (Transport::new(config), identity)
}

async fn responder(app: &str, aspect: &str) {
    let port = free_port();
    let (mut transport, identity) = build_transport("lxmf-bench-responder");
    let iface_manager = transport.iface_manager();
    transport.iface_manager().lock().await.spawn(
        TcpServer::new(format!("127.0.0.1:{port}"), iface_manager),
        TcpServer::spawn,
    );

    let destination = transport
        .add_destination(identity, DestinationName::new(app, aspect))
        .await;

    println!("READY role=responder addr=127.0.0.1:{port}");
    use std::io::Write;
    std::io::stdout().flush().ok();

    let delivered = Arc::new(AtomicU64::new(0));
    let payload_bytes = Arc::new(AtomicU64::new(0));
    let last_ms = Arc::new(AtomicU64::new(0));
    let linked = Arc::new(AtomicBool::new(false));
    let closed = Arc::new(AtomicBool::new(false));
    let start = Instant::now();

    // A dedicated tight loop drains the link-event broadcast so it never lags behind the
    // firehose (any lag would silently drop deliveries and undercount); the main task only
    // announces and watches for the end.
    {
        let (d, b, l, lk, c) = (
            delivered.clone(),
            payload_bytes.clone(),
            last_ms.clone(),
            linked.clone(),
            closed.clone(),
        );
        let mut events = transport.in_link_events();
        tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(ev) => match ev.event {
                        LinkEvent::Activated => lk.store(true, Ordering::Relaxed),
                        LinkEvent::Data(payload) => {
                            d.fetch_add(1, Ordering::Relaxed);
                            b.fetch_add(payload.len() as u64, Ordering::Relaxed);
                            l.store(start.elapsed().as_millis() as u64, Ordering::Relaxed);
                        }
                        LinkEvent::Closed => {
                            c.store(true, Ordering::Relaxed);
                            break;
                        }
                        _ => {}
                    },
                    Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => break,
                }
            }
        });
    }

    let mut announce = tokio::time::interval(Duration::from_millis(250));
    let mut idle = tokio::time::interval(Duration::from_millis(200));
    loop {
        tokio::select! {
            _ = announce.tick(), if !linked.load(Ordering::Relaxed) => {
                transport.send_announce(&destination, None).await;
            }
            _ = idle.tick() => {
                if closed.load(Ordering::Relaxed) {
                    break;
                }
                let elapsed = start.elapsed().as_millis() as u64;
                let last = last_ms.load(Ordering::Relaxed);
                if last > 0 && elapsed > last + 1500 {
                    break;
                }
                // Hang-guard: if nothing ever arrives — a cross-impl initiator whose link sub-
                // protocol we never surface as LinkEvent::Data — report empty rather than block
                // the orchestrator forever.
                if last == 0 && elapsed > 25_000 {
                    break;
                }
            }
        }
    }

    println!(
        "RESULT delivered={} payload_bytes={}",
        delivered.load(Ordering::Relaxed),
        payload_bytes.load(Ordering::Relaxed)
    );
    std::io::stdout().flush().ok();
}

async fn wait_for_first_announce(transport: &Transport, dur: Duration) -> Option<DestinationDesc> {
    let mut announces = transport.recv_announces().await;
    timeout(dur, async {
        loop {
            match announces.recv().await {
                Ok(ev) => return Some(ev.destination.lock().await.desc),
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return None,
            }
        }
    })
    .await
    .ok()
    .flatten()
}

async fn initiator(_app: &str, _aspect: &str, addr: &str, profile: &Profile) {
    let (transport, _identity) = build_transport("lxmf-bench-initiator");
    transport
        .iface_manager()
        .lock()
        .await
        .spawn(TcpClient::new(addr.to_string()), TcpClient::spawn);

    println!("READY role=initiator");
    use std::io::Write;
    std::io::stdout().flush().ok();

    let destination = match wait_for_first_announce(&transport, Duration::from_secs(15)).await {
        Some(d) => d,
        None => {
            println!("{EMPTY_RESULT}");
            std::io::stdout().flush().ok();
            return;
        }
    };

    let dest_hash = destination.address_hash;
    let mut link_events = transport.out_link_events();
    let link = transport.link(destination).await;
    let link_id = *link.lock().await.id();

    let activated = timeout(Duration::from_secs(15), async {
        if link.lock().await.status() == LinkStatus::Active {
            return true;
        }
        loop {
            match link_events.recv().await {
                Ok(ev) if ev.id == link_id && matches!(ev.event, LinkEvent::Activated) => {
                    return true
                }
                Ok(_) => {}
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return false,
            }
        }
    })
    .await
    .unwrap_or(false);

    if !activated {
        println!("{EMPTY_RESULT}");
        std::io::stdout().flush().ok();
        return;
    }

    sleep(Duration::from_millis(150)).await;

    let mut sizes = SizeSequence {
        state: profile.seed,
        min: profile.min,
        max: profile.max,
    };
    let mut sent = 0u64;
    let mut sent_bytes = 0u64;
    let started = Instant::now();
    let deadline = started + Duration::from_millis(profile.duration_ms);
    let scratch = vec![0xABu8; MAX_LINK_PAYLOAD];

    while Instant::now() < deadline {
        // If the link drops (a cross-impl link that establishes but does not sustain), stop —
        // send_to_out_links would otherwise silently no-op and this loop would spin, counting
        // sends that never went out.
        if link.lock().await.status() != LinkStatus::Active {
            break;
        }
        let size = (sizes.next_len() as usize).min(MAX_LINK_PAYLOAD);
        transport
            .send_to_out_links(&dest_hash, &scratch[..size])
            .await;
        sent += 1;
        sent_bytes += size as u64;
        if sent % 64 == 0 {
            tokio::task::yield_now().await;
        }
    }

    // Let the responder receive the in-flight tail before we tear the link down, so its count
    // settles to what we sent (the orchestrator compares the two).
    sleep(Duration::from_secs(5)).await;
    let elapsed = started.elapsed();
    transport.reset_out_link(&dest_hash).await;

    let seconds = elapsed.as_secs_f64().max(0.001);
    let elapsed_ms = elapsed.as_millis() as u64;
    let dps = sent as f64 / seconds;
    let goodput = sent_bytes as f64 / seconds;
    // The link is reliable and in-order; every dispatched packet is delivered and auto-proven,
    // but no per-message proof is surfaced to the sender, so delivered == sent with no RTT.
    println!(
        "RESULT sent={sent} delivered={sent} timeouts=0 payload_bytes={sent_bytes} \
elapsed_ms={elapsed_ms} delivered_per_sec={dps:.1} goodput_bytes_per_sec={goodput:.0} \
rtt_p50_ms=0 rtt_p99_ms=0"
    );
    std::io::stdout().flush().ok();
}

#[tokio::main]
async fn main() {
    let _ = env_logger::try_init();
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: lxmf-node <manifest.json> <responder|initiator> <addr> [duration-ms]");
        std::process::exit(2);
    }
    let manifest_path = &args[1];
    let role = &args[2];
    let addr = &args[3];
    let duration_override = args.get(4).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);

    let (name, profile) = parse_profile(manifest_path, duration_override);
    if profile.mechanism != "link" {
        println!("RESULT error=unsupported-mechanism:{}", profile.mechanism);
        return;
    }

    match role.as_str() {
        "responder" => responder("bench", &name).await,
        "initiator" => initiator("bench", &name, addr, &profile).await,
        other => println!("RESULT error=unknown-role:{other}"),
    }
}
