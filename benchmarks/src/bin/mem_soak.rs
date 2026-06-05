//! Memory axis — long-run tick soak (host route: dhat). Ingest a fixed announce set
//! once, then tick across a long *simulated* horizon and watch whether live memory and
//! per-tick allocations stay flat. Drift = a long-running edge.
//!
//! Run:   `cargo run --release --bin mem_soak`
//! Crank: `MEM_SOAK_TICKS=50000000 MEM_SOAK_STEP_MS=100 cargo run --release --bin mem_soak`

use std::fmt::Write as _;

use benchmarks::{ingest_all, load_corpus, new_engine, scenario_dir, tick_soak};
use personal_rns::routing::storage::GrowableHeap;

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

fn env<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// Format into a reused buffer (no per-call allocation, so it doesn't pollute the
/// allocation counts the soak is measuring).
fn human_ms(buf: &mut String, ms: u64) {
    buf.clear();
    let s = ms / 1_000;
    let (d, h, m) = (s / 86_400, (s % 86_400) / 3_600, (s % 3_600) / 60);
    let _ = write!(buf, "{d}d {h:02}h {m:02}m");
}

fn main() {
    let _profiler = dhat::Profiler::new_heap();

    let ticks: u64 = env("MEM_SOAK_TICKS", 5_000_000);
    let step_ms: u64 = env("MEM_SOAK_STEP_MS", 250);
    let samples: u64 = env("MEM_SOAK_SAMPLES", 12);
    let sample_every = (ticks / samples).max(1);

    let mut announces = load_corpus(&scenario_dir("announce-256"));
    let mut engine = new_engine::<GrowableHeap>();
    ingest_all(&mut engine, &mut announces);

    let mut time_buf = String::with_capacity(24);
    human_ms(&mut time_buf, ticks * step_ms);
    println!(
        "soak: {ticks} ticks @ +{step_ms}ms each (~{time_buf} simulated), {} routes ingested\n",
        engine.route_count()
    );
    println!(
        "{:>11}  {:>12}  {:>9}  {:>11}  {:>9}  {:>6}  {:>5}  {:>7}",
        "tick", "sim_time", "Δallocs", "live_bytes", "live_blk", "routes", "held", "pending"
    );

    let base = 3_000u64;
    let mut last_allocs = dhat::HeapStats::get().total_blocks;
    let (mut min_live, mut max_live) = (usize::MAX, 0usize);

    tick_soak(&mut engine, base, step_ms, ticks, sample_every, |t, engine| {
        let stats = dhat::HeapStats::get();
        let delta = stats.total_blocks - last_allocs;
        last_allocs = stats.total_blocks;
        min_live = min_live.min(stats.curr_bytes);
        max_live = max_live.max(stats.curr_bytes);
        human_ms(&mut time_buf, base + t * step_ms);
        println!(
            "{:>11}  {:>12}  {:>9}  {:>11}  {:>9}  {:>6}  {:>5}  {:>7}",
            t,
            time_buf,
            delta,
            stats.curr_bytes,
            stats.curr_blocks,
            engine.route_count(),
            engine.held_announce_count(),
            engine.pending_announce_rebroadcast_count(),
        );
    });

    let drift = max_live - min_live;
    println!("\nlive_bytes over the soak: min={min_live} max={max_live} drift={drift} bytes");
    println!(
        "verdict: {}",
        if drift == 0 {
            "FLAT — no growth across the soak.".to_string()
        } else {
            format!("{drift} bytes of movement — inspect the trend (expiry frees memory; monotonic growth is a leak).")
        }
    );
}
