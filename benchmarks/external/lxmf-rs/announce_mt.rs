//! LXMF-rs' `announce-parallel` harness: shard the corpus across `t` worker threads, each
//! parsing + validating (rns-core's `DestinationAnnounce::validate` is stateless), best-of-N
//! min wall. Conformance is the distinct routes learned in a single-threaded pass. Swept
//! single-thread vs all logical cores; prints the parallel RESULT line for run-mt.sh.

use std::collections::HashSet;
use std::thread;
use std::time::Instant;

use rns_core::destination::DestinationAnnounce;
use rns_core::packet::Packet;

const WARMUP: usize = 5;
const ITERS: usize = 30;

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

fn ingest(chunk: &[Vec<u8>]) -> usize {
    let mut learned: HashSet<Vec<u8>> = HashSet::new();
    for raw in chunk {
        if let Ok(pkt) = Packet::from_bytes(raw) {
            if let Ok(info) = DestinationAnnounce::validate(&pkt) {
                learned.insert(info.destination.desc.address_hash.as_slice().to_vec());
            }
        }
    }
    learned.len()
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
            .map(|chunk| thread::spawn(move || ingest(&chunk)))
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
    let path = std::env::args().nth(1).expect("usage: announce_mt <corpus.hex>");
    let text = std::fs::read_to_string(&path).expect("read corpus");
    let all: Vec<Vec<u8>> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(from_hex)
        .collect();

    let resolved = ingest(&all);

    let cores = thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let (lo, hi) = (1usize, cores.max(1));
    let lo_ps = throughput_at(&all, lo);
    let hi_ps = if hi == lo { lo_ps } else { throughput_at(&all, hi) };

    println!("LXMF-rs / announce-parallel: routes {resolved}/{}, {lo}t {lo_ps:.0}/s, {hi}t {hi_ps:.0}/s", all.len());
    println!("RESULT resolved={resolved} lo={lo} lo_per_sec={lo_ps:.3} hi={hi} hi_per_sec={hi_ps:.3}");
}
