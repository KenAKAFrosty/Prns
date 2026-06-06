//! Leviculum's `announce-parallel` harness: shard the corpus across `t` worker threads,
//! each a fresh `reticulum-core` Transport running the real parse → verify → store path,
//! best-of-N min wall. Conformance is the thread-count-independent route count from a
//! single-threaded pass. Swept single-thread vs all logical cores; prints the parallel
//! RESULT line for run-mt.sh. Mirrors benchmarks/src/bin/bench_parallel.rs.

use std::thread;
use std::time::Instant;

use rand_core::OsRng;
use reticulum_core::identity::Identity;
use reticulum_core::memory_storage::MemoryStorage;
use reticulum_core::traits::Clock;
use reticulum_core::transport::{Transport, TransportConfig};

const WARMUP: usize = 5;
const ITERS: usize = 30;

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

fn split(all: &[Vec<u8>], t: usize) -> Vec<Vec<Vec<u8>>> {
    let chunk = all.len().div_ceil(t);
    all.chunks(chunk).map(<[Vec<u8>]>::to_vec).collect()
}

fn build_transport() -> Transport<FixedClock, MemoryStorage> {
    Transport::new(
        TransportConfig::default(),
        FixedClock(1_700_000_000_000),
        MemoryStorage::with_defaults(),
        Identity::generate(&mut OsRng),
    )
}

fn throughput_at(all: &[Vec<u8>], t: usize) -> f64 {
    let total = all.len();
    let chunks = split(all, t);
    let mut best = f64::INFINITY;
    for i in 0..(WARMUP + ITERS) {
        let iter_chunks: Vec<Vec<Vec<u8>>> = chunks.to_vec();
        let start = Instant::now();
        let handles: Vec<_> = iter_chunks
            .into_iter()
            .map(|chunk| {
                thread::spawn(move || {
                    let mut transport = build_transport();
                    for raw in &chunk {
                        let _ = transport.process_incoming(0, raw);
                    }
                    transport.path_count()
                })
            })
            .collect();
        let routes: usize = handles.into_iter().map(|h| h.join().unwrap()).sum();
        std::hint::black_box(routes);
        let secs = start.elapsed().as_secs_f64();
        if i >= WARMUP {
            best = best.min(secs);
        }
    }
    total as f64 / best
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: leviculum-announce-mt <corpus.hex>");
    let text = std::fs::read_to_string(&path).expect("read corpus");
    let all: Vec<Vec<u8>> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(from_hex)
        .collect();

    let mut conformance = build_transport();
    for raw in &all {
        let _ = conformance.process_incoming(0, raw);
    }
    let resolved = conformance.path_count();

    let cores = thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let (lo, hi) = (1usize, cores.max(1));
    let lo_ps = throughput_at(&all, lo);
    let hi_ps = if hi == lo { lo_ps } else { throughput_at(&all, hi) };

    println!("Leviculum / announce-parallel: routes {resolved}/{}, {lo}t {lo_ps:.0}/s, {hi}t {hi_ps:.0}/s", all.len());
    println!("RESULT resolved={resolved} lo={lo} lo_per_sec={lo_ps:.3} hi={hi} hi_per_sec={hi_ps:.3}");
}
