//! Benchmark/perf **scenarios** for the Personal Reticulum engine: the host-neutral,
//! storage-generic workloads every measurement axis drives. The scenario is the
//! constant; the measurement backend is the routable seam — dhat on the host
//! (`src/bin`), criterion for timing (`benches`), and `esp_alloc::HEAP.stats()` on the
//! microcontroller when that route lands. Nothing in this file measures anything.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::vec::Vec;

use personal_rns::engine::{EngineState, InstantMillis, ReannounceSchedule, SelfAnnounceConfig};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::{InboundPacket, InterfaceId};
use personal_rns::routing::announce::defaults::JitterSeed;
use personal_rns::routing::announce::SelfAnnounceEntropy;
use personal_rns::routing::storage::{EngineStorage, FixedInline, GrowableHeap};

mod results;
pub use results::{
    load_all_rows, load_host, results_dir, write_host, write_rows, Axis, Comparability,
    HostDescriptor, ResultRow,
};

pub type Cap = FixedInline<64, 64, 4096, 4, 512, 64, 8, 8, 8, 128, 8, 8>;

const JITTER: JitterSeed = JitterSeed(0x5151_5151_5151_5151);

/// A distinct, deterministic identity key per `seed` (any 64 bytes are valid seeds).
pub fn node_key(seed: u16) -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
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

/// `count` distinct, validly-signed announces — one per throwaway sender identity.
/// A host-side fixture builder (the senders allocate); call it before measuring.
pub fn announce_fixtures(count: usize) -> Vec<Vec<u8>> {
    let mut announces = Vec::with_capacity(count);
    for k in 0..count {
        let config = SelfAnnounceConfig {
            app_name: "lxmf",
            aspects: &["delivery"],
            app_data: b"benchmarks",
            schedule: ReannounceSchedule::every(10_000),
        };
        let mut sender = EngineState::<GrowableHeap>::announcing(&node_key(k as u16 ^ 0xC300), config)
            .expect("valid self-announce config");
        let entropy =
            SelfAnnounceEntropy::new([(k as u8).wrapping_add(0x40); SelfAnnounceEntropy::LEN]);
        let mut buf = [0u8; 512];
        let written = sender
            .write_due_self_announce(InstantMillis(1_000), entropy, &mut buf)
            .expect("self-announce serializes");
        if let Some(len) = written {
            announces.push(buf[..len].to_vec());
        }
    }
    announces
}

/// A fresh receiver engine over storage `S`.
pub fn new_engine<S: EngineStorage>() -> EngineState<S> {
    EngineState::<S>::new(&node_key(0x11))
}

/// Ingest every packet into `engine` over one interface.
pub fn ingest_all<S: EngineStorage>(engine: &mut EngineState<S>, packets: &mut [Vec<u8>]) {
    let interface = InterfaceId::new([0xAA; 16]);
    for (i, packet) in packets.iter_mut().enumerate() {
        let _ = engine.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(2_000 + i as u64),
                source_interface: interface,
                bytes: packet.as_mut_slice(),
            },
            JITTER,
        );
    }
}

/// The monomorphic workload: fresh engine, ingest the announces, settle `cycles` ticks.
/// Returns the resulting route count. Same code for every storage + measurement axis.
pub fn ingest_and_settle<S: EngineStorage>(packets: &mut [Vec<u8>], cycles: usize) -> usize {
    let mut engine = new_engine::<S>();
    ingest_all(&mut engine, packets);
    for cycle in 0..cycles {
        let _ = engine.tick(InstantMillis(20_000 + (cycle as u64) * 1_000), JitterSeed(0));
    }
    engine.route_count()
}

/// Tick `engine` `ticks` times, advancing the clock `step_ms` per tick, invoking
/// `on_sample(tick, &engine)` every `sample_every` ticks (and on the last). The
/// callback is the measurement seam — this loop allocates nothing of its own.
pub fn tick_soak<S: EngineStorage>(
    engine: &mut EngineState<S>,
    base_ms: u64,
    step_ms: u64,
    ticks: u64,
    sample_every: u64,
    mut on_sample: impl FnMut(u64, &EngineState<S>),
) {
    for t in 1..=ticks {
        let _ = engine.tick(InstantMillis(base_ms + t * step_ms), JitterSeed(0));
        if t % sample_every == 0 || t == ticks {
            on_sample(t, engine);
        }
    }
}

// --- Scenario corpus: the language-neutral seam ---
// A scenario's *input* is shared data on disk (a hex-per-line wire corpus + a JSON
// manifest), so any implementation — any language — can replay the exact same bytes.
// `announce_fixtures` is the generator; the `gen_corpus` bin writes it out; every
// measurement backend `load`s it. That's what keeps "run it yourself" honest.

/// The on-disk home of scenario `name` (e.g. "announce-256"), relative to this crate.
pub fn scenario_dir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios").join(name)
}

/// Load a scenario's wire packets from `<dir>/packets.hex` (one hex-encoded packet per
/// line) — the same bytes another implementation would replay.
pub fn load_corpus(dir: &Path) -> Vec<Vec<u8>> {
    let path = dir.join("packets.hex");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(from_hex)
        .collect()
}

/// Lowercase hex encode.
pub fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Decode a hex string into bytes.
pub fn from_hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("corpus is valid hex"))
        .collect()
}
