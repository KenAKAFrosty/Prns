// LXMF-rs' sustained energy harness: sustained parse + stateless validate across all logical
// cores for a fixed wall-time. usage: <corpus.hex> <secs> [working_set]

use std::hint::black_box;
use std::thread;
use std::time::{Duration, Instant};

use rns_core::destination::DestinationAnnounce;
use rns_core::packet::Packet;

fn from_hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

fn work(shard: &[Vec<u8>]) -> usize {
    let mut n = 0usize;
    for raw in shard {
        if let Ok(pkt) = Packet::from_bytes(raw) {
            if DestinationAnnounce::validate(&pkt).is_ok() {
                n += 1;
            }
        }
    }
    n
}

fn main() {
    let mut a = std::env::args().skip(1);
    let path = a.next().expect("usage: <corpus.hex> <secs> [working_set]");
    let secs: u64 = a.next().and_then(|s| s.parse().ok()).unwrap_or(60);
    let ws: usize = a.next().and_then(|s| s.parse().ok()).unwrap_or(50_000);

    let text = std::fs::read_to_string(&path).expect("read corpus");
    let base: Vec<Vec<u8>> = text.lines().map(str::trim).filter(|l| !l.is_empty()).map(from_hex).collect();
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
                        black_box(work(shard));
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
