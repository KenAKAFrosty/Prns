//! Prns' sustained harness for the `announce-energy` scenario: run the real parse → verify →
//! store path under continuous all-cores load for a fixed wall-time, so an external sampler
//! (`powermetrics` / RAPL) can integrate package power over a long steady run. Prints the
//! sustained throughput; `energy/measure.sh` wraps the power sampling around it and files the
//! conformance + throughput + power + energy rows.
//!
//! The corpus is replicated to a working set so per-loop setup (fresh engine + byte clone)
//! amortizes to noise — verify dominates, and duplicates don't change verify cost. Throughput
//! is the sustained *average* under continuous load (the energy denominator), not a best-of-N peak.
//!
//! Run: `cargo run --release --bin sustained -- <corpus.hex> <secs> [working_set]`

use std::hint::black_box;
use std::thread;
use std::time::{Duration, Instant};

use benchmarks::{from_hex, ingest_all, new_engine};
use personal_rns::routing::storage::GrowableHeap;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: sustained <corpus.hex> <secs> [working_set]");
    let secs: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(60);
    let working_set: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(50_000);

    let text = std::fs::read_to_string(&path).expect("read corpus");
    let base: Vec<Vec<u8>> = text.lines().map(str::trim).filter(|l| !l.is_empty()).map(from_hex).collect();

    let resolved = {
        let mut engine = new_engine::<GrowableHeap>();
        let mut once = base.clone();
        ingest_all(&mut engine, &mut once);
        engine.route_count()
    };
    println!("CONFORMANCE resolved={resolved}");

    let corpus: Vec<Vec<u8>> = base.iter().cloned().cycle().take(working_set.max(base.len())).collect();
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
    println!("THROUGHPUT announces_per_sec={:.1} total={total} secs={elapsed:.2}", total as f64 / elapsed);
}
