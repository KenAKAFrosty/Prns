//! Drive Leviculum's `reticulum-core` over the shared announce-256 corpus through its
//! real ingest path (`Transport::process_incoming` — parse + verify + store), best-of-N
//! min wall time. Prints a `RESULT resolved=<n> per_sec=<f>` line for run.sh to file as
//! result rows. Mirrors benchmarks/src/bin/bench_result.rs's methodology.

use std::time::Instant;

use rand_core::OsRng;
use reticulum_core::identity::Identity;
use reticulum_core::memory_storage::MemoryStorage;
use reticulum_core::traits::Clock;
use reticulum_core::transport::{Transport, TransportConfig};

const WARMUP: usize = 5;
const ITERS: usize = 50;

struct FixedClock(u64);
impl Clock for FixedClock {
    fn now_ms(&self) -> u64 {
        self.0
    }
}

fn from_hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("corpus is valid hex"))
        .collect()
}

fn build_transport() -> Transport<FixedClock, MemoryStorage> {
    let identity = Identity::generate(&mut OsRng);
    Transport::new(
        TransportConfig::default(),
        FixedClock(1_700_000_000_000),
        MemoryStorage::with_defaults(),
        identity,
    )
}

fn ingest_all(transport: &mut Transport<FixedClock, MemoryStorage>, corpus: &[Vec<u8>]) {
    for raw in corpus {
        let _ = transport.process_incoming(0, raw);
    }
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: leviculum-announce-bench <corpus.hex>");
    let text = std::fs::read_to_string(&path).expect("read corpus");
    let corpus: Vec<Vec<u8>> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(from_hex)
        .collect();
    let count = corpus.len();

    let mut conformance = build_transport();
    ingest_all(&mut conformance, &corpus);
    let routes = conformance.path_count();

    let mut best = f64::INFINITY;
    for i in 0..(WARMUP + ITERS) {
        let mut transport = build_transport();
        let start = Instant::now();
        ingest_all(&mut transport, &corpus);
        let secs = start.elapsed().as_secs_f64();
        if i >= WARMUP {
            best = best.min(secs);
        }
    }
    let per_sec = count as f64 / best;

    println!("Leviculum / announce-256: routes {routes}/{count}, {per_sec:.0} announce/s");
    println!("RESULT resolved={routes} per_sec={per_sec:.3}");
}
