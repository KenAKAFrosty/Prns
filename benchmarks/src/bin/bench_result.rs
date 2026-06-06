//! personal-rns' driver for the result substrate: replay the `announce-256` corpus
//! through the engine and emit the two cross-comparable figures — conformance
//! (routes resolved) and ingest throughput — as rows for `render_results`. The
//! reference's `reference/driver.py` emits the matching rows for RNS.
//!
//! GrowableHeap is the storage here because this scenario resolves 256 routes (an
//! unbounded, heap-backed table, the fair counterpart to RNS on a PC); the embedded
//! no-alloc `FixedInline` path is the memory axis' story, not throughput's.
//!
//! Run: `cargo run --release --bin bench_result`

use std::process::Command;
use std::time::Instant;

use benchmarks::{
    ingest_all, ingest_and_settle, load_corpus, new_engine, scenario_dir, write_rows, Axis,
    ResultRow,
};
use personal_rns::routing::storage::GrowableHeap;

const SCENARIO: &str = "announce-256";
const VERSION: u32 = 1;
const SETTLE_TICKS: usize = 64;
const WARMUP: usize = 5;
const ITERS: usize = 50;

fn main() {
    let corpus = load_corpus(&scenario_dir(SCENARIO));
    let count = corpus.len();

    let mut settle_input = corpus.clone();
    let routes = ingest_and_settle::<GrowableHeap>(&mut settle_input, SETTLE_TICKS);

    let mut best_secs = f64::INFINITY;
    for i in 0..(WARMUP + ITERS) {
        let mut engine = new_engine::<GrowableHeap>();
        let mut input = corpus.clone();
        let start = Instant::now();
        ingest_all(&mut engine, &mut input);
        let secs = start.elapsed().as_secs_f64();
        if i >= WARMUP {
            best_secs = best_secs.min(secs);
        }
    }
    let per_sec = count as f64 / best_secs;

    let (commit, toolchain, host) = stamp();
    let row = |axis, metric: &str, value: f64, unit: &str| ResultRow {
        scenario: SCENARIO.to_string(),
        scenario_version: VERSION,
        implementation: "personal-rns".to_string(),
        commit: commit.clone(),
        toolchain: toolchain.clone(),
        host: host.clone(),
        axis,
        metric: metric.to_string(),
        value: Some(value),
        unit: unit.to_string(),
    };

    let rows = vec![
        row(Axis::Conformance, "routes_resolved", routes as f64, "count"),
        row(Axis::Throughput, "ingest_announces_per_sec", per_sec, "announce/s"),
    ];
    write_rows(&host, SCENARIO, "personal-rns", &rows);

    println!("personal-rns / {SCENARIO}: routes {routes}/{count}, ingest {per_sec:.0} announce/s");
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
