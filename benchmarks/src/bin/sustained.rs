//! Prns' sustained node for the `announce-energy` scenario shape: run the real
//! parse → verify → store path under continuous all-cores load for a fixed wall-time,
//! so the orchestrator's energy bracket (`powermetrics` / RAPL) can integrate package
//! power over a long steady run. Single process, no network — the verify-bound core
//! efficiency number, distinct from the two-node interop firehose.
//!
//! The contract every sustained node speaks (ours and each external port): argv is
//! `<corpus.hex> <secs> [working_set]`, and stdout carries two lines the orchestrator
//! parses —
//!   `CONFORMANCE resolved=<routes>`           (a clean single pass proves the work)
//!   `THROUGHPUT announces_per_sec=<r> total=<n> secs=<s>`   (the sustained average)
//!
//! The corpus is replicated to a working set so per-loop setup (fresh engine + byte
//! clone) amortizes to noise — verify dominates, and duplicates don't change verify cost.
//!
//! Run: `cargo run --release --bin sustained -- <corpus.hex> <secs> [working_set]`

use std::hint::black_box;
use std::thread;
use std::time::{Duration, Instant};

use personal_rns::engine::{EngineState, InstantMillis};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::{
    AnnounceBandwidthCap, EgressCapability, InboundPacket, IngressCapability, InterfaceCapabilities,
    InterfaceConfig, InterfaceId, InterfaceMode, TransportCapability,
};
use personal_rns::routing::announce::defaults::JitterSeed;
use personal_rns::routing::storage::{EngineStorage, GrowableHeap};

const JITTER: JitterSeed = JitterSeed(0x5151_5151_5151_5151);
const SOURCE_INTERFACE: InterfaceId = InterfaceId::new([0xAA; 16]);

fn node_key(seed: u16) -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    let mut key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    for (i, byte) in key.iter_mut().enumerate() {
        *byte = (seed as u8)
            .wrapping_mul(31)
            .wrapping_add((seed >> 8) as u8)
            .wrapping_add(i as u8)
            .wrapping_add(1);
    }
    key
}

fn new_engine<S: EngineStorage>() -> EngineState<S> {
    EngineState::<S>::new(node_key(0x11))
}

/// One enabled interface keyed to the corpus's source, so ingest resolves a route
/// instead of dropping the announce for an unknown interface.
fn interface_view() -> [InterfaceConfig; 1] {
    [InterfaceConfig {
        id: SOURCE_INTERFACE,
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
        },
        mode: InterfaceMode::Full,
        bitrate_bps: None,
        hardware_mtu: None,
        announce_rate_limit: None,
        announce_bandwidth_cap: AnnounceBandwidthCap::Unlimited,
        airtime_duty_cycle: None,
    }]
}

fn ingest_all<S: EngineStorage>(engine: &mut EngineState<S>, packets: &mut [Vec<u8>]) {
    let view = interface_view();
    for (i, packet) in packets.iter_mut().enumerate() {
        let _ = engine.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(2_000 + i as u64),
                source_interface: SOURCE_INTERFACE,
                bytes: packet.as_mut_slice(),
            },
            JITTER,
            &view,
        );
    }
}

fn from_hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("corpus is valid hex"))
        .collect()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: sustained <corpus.hex> <secs> [working_set]");
    let secs: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(60);
    let working_set: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(50_000);

    let text = std::fs::read_to_string(&path).expect("read corpus");
    let base: Vec<Vec<u8>> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(from_hex)
        .collect();

    let mut conformance_engine = new_engine::<GrowableHeap>();
    let mut conformance_corpus = base.clone();
    ingest_all(&mut conformance_engine, &mut conformance_corpus);
    println!("CONFORMANCE resolved={}", conformance_engine.route_count());

    let corpus: Vec<Vec<u8>> = base
        .iter()
        .cloned()
        .cycle()
        .take(working_set.max(base.len()))
        .collect();
    let threads = thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let chunk = corpus.len().div_ceil(threads);
    let shards: Vec<Vec<Vec<u8>>> = corpus.chunks(chunk).map(<[Vec<u8>]>::to_vec).collect();

    let deadline = Instant::now() + Duration::from_secs(secs);
    let start = Instant::now();
    let total: usize = thread::scope(|scope| {
        shards
            .iter()
            .map(|shard| {
                scope.spawn(move || {
                    let mut count = 0usize;
                    while Instant::now() < deadline {
                        let mut engine = new_engine::<GrowableHeap>();
                        let mut work = shard.clone();
                        ingest_all(&mut engine, &mut work);
                        black_box(engine.route_count());
                        count += shard.len();
                    }
                    count
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|h| h.join().unwrap())
            .sum()
    });
    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "THROUGHPUT announces_per_sec={:.1} total={total} secs={elapsed:.2}",
        total as f64 / elapsed
    );
}
