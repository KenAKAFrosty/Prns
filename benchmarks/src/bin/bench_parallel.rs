//! personal-rns' driver for the `announce-parallel` scenario: the announce-256 ingest
//! path is ~97% independent per-announce Ed25519 verify, so it should scale across cores.
//! Shard 2560 distinct announces evenly across `t` worker threads — each a *fresh* engine
//! running the real parse → verify → store path — and time the whole batch, best-of-N min
//! wall. Swept single-thread vs all of the host's logical cores; both figures are filed as
//! throughput rows tagged with their thread count, for `render_results`' parallel table.
//!
//! Conformance is the thread-count-independent correctness check (every route resolved),
//! so it's measured once single-threaded, exactly as `bench_result` does for announce-256.
//!
//! Run: `cargo run --release --bin bench_parallel`

use std::process::Command;
use std::thread;
use std::time::Instant;

use benchmarks::{
    ingest_all, ingest_and_settle, load_corpus, new_engine, scenario_dir, write_rows, Axis,
    ResultRow,
};
use personal_rns::engine::EngineState;
use personal_rns::routing::storage::GrowableHeap;

const SCENARIO: &str = "announce-parallel";
const VERSION: u32 = 1;
const SETTLE_TICKS: usize = 64;
const WARMUP: usize = 5;
const ITERS: usize = 30;

fn split(all: &[Vec<u8>], t: usize) -> Vec<Vec<Vec<u8>>> {
    let chunk = all.len().div_ceil(t);
    all.chunks(chunk).map(<[Vec<u8>]>::to_vec).collect()
}

/// Best-of-N announces-per-second with the corpus sharded across `threads` fresh engines.
/// Engines and packet bytes are built off the clock so only the parse+verify+store work
/// is timed.
fn throughput_at(corpus: &[Vec<u8>], threads: usize) -> f64 {
    let total = corpus.len();
    let chunks = split(corpus, threads);
    let mut best = f64::INFINITY;
    for i in 0..(WARMUP + ITERS) {
        let work: Vec<(EngineState<GrowableHeap>, Vec<Vec<u8>>)> = chunks
            .iter()
            .map(|chunk| (new_engine::<GrowableHeap>(), chunk.clone()))
            .collect();
        let start = Instant::now();
        let handles: Vec<_> = work
            .into_iter()
            .map(|(mut engine, mut chunk)| {
                thread::spawn(move || {
                    ingest_all(&mut engine, &mut chunk);
                    engine.route_count()
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
    let corpus = load_corpus(&scenario_dir(SCENARIO));
    let count = corpus.len();

    let mut settle_input = corpus.clone();
    let routes = ingest_and_settle::<GrowableHeap>(&mut settle_input, SETTLE_TICKS);

    let cores = thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let (lo, hi) = (1usize, cores.max(1));
    let per_sec_lo = throughput_at(&corpus, lo);
    let per_sec_hi = if hi == lo { per_sec_lo } else { throughput_at(&corpus, hi) };

    let (commit, toolchain, host) = stamp();
    let row = |axis, metric: &str, value: f64, unit: &str, threads: Option<u32>| ResultRow {
        scenario: SCENARIO.to_string(),
        scenario_version: VERSION,
        implementation: "Prns".to_string(),
        commit: commit.clone(),
        toolchain: toolchain.clone(),
        host: host.clone(),
        axis,
        metric: metric.to_string(),
        value: Some(value),
        unit: unit.to_string(),
        threads,
    };

    let rows = vec![
        row(Axis::Conformance, "routes_resolved", routes as f64, "count", None),
        row(Axis::Throughput, "ingest_announces_per_sec", per_sec_lo, "announce/s", Some(lo as u32)),
        row(Axis::Throughput, "ingest_announces_per_sec", per_sec_hi, "announce/s", Some(hi as u32)),
    ];
    write_rows(&host, SCENARIO, "personal-rns", &rows);

    println!("personal-rns / {SCENARIO}: routes {routes}/{count}");
    println!("  {lo:>2} thread   {per_sec_lo:>10.0} announce/s");
    println!("  {hi:>2} threads  {per_sec_hi:>10.0} announce/s");
    println!("  stamp: {commit} | {toolchain} | {host}");
}

fn stamp() -> (String, String, String) {
    let commit = run("git", &["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let toolchain = run("rustc", &["--version"])
        .map(|v| v.trim_start_matches("rustc ").to_string())
        .unwrap_or_else(|| "unknown".into());
    let host = rustc_host().unwrap_or_else(|| "unknown".into());
    (commit, toolchain, host)
}

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn rustc_host() -> Option<String> {
    run("rustc", &["-vV"])?
        .lines()
        .find_map(|l| l.strip_prefix("host: ").map(str::to_string))
}
