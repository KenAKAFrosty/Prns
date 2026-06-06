// Leviculum's sustained energy harness: sustained parse+verify+store across all logical cores
// for a fixed wall-time. usage: <corpus.hex> <secs> [working_set]

use std::hint::black_box;
use std::thread;
use std::time::{Duration, Instant};

use rand_core::OsRng;
use reticulum_core::identity::Identity;
use reticulum_core::memory_storage::MemoryStorage;
use reticulum_core::traits::Clock;
use reticulum_core::transport::{Transport, TransportConfig};

struct FixedClock(u64);
impl Clock for FixedClock {
    fn now_ms(&self) -> u64 {
        self.0
    }
}

fn from_hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

fn build() -> Transport<FixedClock, MemoryStorage> {
    Transport::new(
        TransportConfig::default(),
        FixedClock(1_700_000_000_000),
        MemoryStorage::with_defaults(),
        Identity::generate(&mut OsRng),
    )
}

fn main() {
    let mut a = std::env::args().skip(1);
    let path = a.next().expect("usage: <corpus.hex> <secs> [working_set]");
    let secs: u64 = a.next().and_then(|s| s.parse().ok()).unwrap_or(60);
    let ws: usize = a.next().and_then(|s| s.parse().ok()).unwrap_or(50_000);

    let text = std::fs::read_to_string(&path).expect("read corpus");
    let base: Vec<Vec<u8>> = text.lines().map(str::trim).filter(|l| !l.is_empty()).map(from_hex).collect();

    let resolved = {
        let mut tr = build();
        for raw in &base {
            let _ = tr.process_incoming(0, raw);
        }
        tr.path_count()
    };
    println!("CONFORMANCE resolved={resolved}");

    let corpus: Vec<Vec<u8>> = base.iter().cloned().cycle().take(ws.max(base.len())).collect();
    let threads = thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let chunk = corpus.len().div_ceil(threads);
    let shards: Vec<Vec<Vec<u8>>> = corpus.chunks(chunk).map(<[Vec<u8>]>::to_vec).collect();

    let deadline = Instant::now() + Duration::from_secs(secs);
    let start = Instant::now();
    let total: usize = thread::scope(|sc| {
        shards
            .iter()
            .map(|shard| {
                sc.spawn(move || {
                    let mut c = 0usize;
                    while Instant::now() < deadline {
                        let mut tr = build();
                        for raw in shard {
                            let _ = tr.process_incoming(0, raw);
                        }
                        black_box(tr.path_count());
                        c += shard.len();
                    }
                    c
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|h| h.join().unwrap())
            .sum()
    });
    let el = start.elapsed().as_secs_f64();
    println!("THROUGHPUT announces_per_sec={:.1} total={total} secs={el:.2}", total as f64 / el);
}
