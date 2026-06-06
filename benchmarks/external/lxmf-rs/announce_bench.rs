//! Drive LXMF-rs (`reticulum-rs-core`) over the shared announce-256 corpus: parse
//! (`Packet::from_bytes`) + validate (`DestinationAnnounce::validate`, the Ed25519
//! verify) + store the recovered destination, best-of-N min wall time. `validate()`
//! verifies but doesn't store (storage is a separate crate), so we add the HashSet
//! insert to match the parse+verify+store the other impls' validate_announce does.
//! run.sh copies this into the cloned crate's examples/ and runs it. Prints a
//! `RESULT resolved=<n> per_sec=<f>` line for run.sh to file.

use std::collections::HashSet;
use std::time::Instant;

use rns_core::destination::DestinationAnnounce;
use rns_core::packet::Packet;

const WARMUP: usize = 5;
const ITERS: usize = 50;

fn from_hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("corpus is valid hex"))
        .collect()
}

fn ingest_all(corpus: &[Vec<u8>]) -> usize {
    let mut learned: HashSet<Vec<u8>> = HashSet::new();
    for raw in corpus {
        if let Ok(pkt) = Packet::from_bytes(raw) {
            if let Ok(info) = DestinationAnnounce::validate(&pkt) {
                learned.insert(info.destination.desc.address_hash.as_slice().to_vec());
            }
        }
    }
    learned.len()
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: announce_bench <corpus.hex>");
    let text = std::fs::read_to_string(&path).expect("read corpus");
    let corpus: Vec<Vec<u8>> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(from_hex)
        .collect();
    let count = corpus.len();

    let resolved = ingest_all(&corpus);

    let mut best = f64::INFINITY;
    for i in 0..(WARMUP + ITERS) {
        let start = Instant::now();
        ingest_all(&corpus);
        let secs = start.elapsed().as_secs_f64();
        if i >= WARMUP {
            best = best.min(secs);
        }
    }
    let per_sec = count as f64 / best;

    println!("LXMF-rs / announce-256: resolved {resolved}/{count}, {per_sec:.0} announce/s");
    println!("RESULT resolved={resolved} per_sec={per_sec:.3}");
}
