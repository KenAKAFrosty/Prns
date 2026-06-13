//! Heap-allocation microscope: the `Cycle` endpoint roundtrip under dhat. The
//! default run prints allocations-per-roundtrip (testing mode); the `heap` arg
//! dumps `dhat-heap.json` for the call-site viewer. Owns its own global
//! allocator, so only this example binary is instrumented.

use std::env;

use benchmarks::microscope::Cycle;

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

const ROUNDTRIPS: usize = 2000;

fn run_roundtrips(cycle: &mut Cycle, count: usize) {
    for _ in 0..count {
        cycle.seal();
        cycle.deliver_prove();
        cycle.settle();
    }
}

fn main() {
    if env::args().nth(1).as_deref() == Some("heap") {
        let _profiler = dhat::Profiler::new_heap();
        let mut cycle = Cycle::new();
        run_roundtrips(&mut cycle, ROUNDTRIPS);
        eprintln!(
            "wrote dhat-heap.json ({ROUNDTRIPS} roundtrips) — open at \
             https://nnethercote.github.io/dh_view/dh_view.html"
        );
        return;
    }

    let _profiler = dhat::Profiler::builder().testing().build();
    let mut cycle = Cycle::new();
    run_roundtrips(&mut cycle, 1);
    let before = dhat::HeapStats::get();
    run_roundtrips(&mut cycle, ROUNDTRIPS);
    let after = dhat::HeapStats::get();

    let blocks = after.total_blocks - before.total_blocks;
    let bytes = after.total_bytes - before.total_bytes;
    let live_delta = after.curr_blocks as i64 - before.curr_blocks as i64;

    println!("endpoint SINGLE roundtrip — dhat heap, {ROUNDTRIPS} cycles post-warmup");
    println!(
        "  allocations: {blocks} blocks  ({:.2}/roundtrip)",
        blocks as f64 / ROUNDTRIPS as f64
    );
    println!(
        "  bytes:       {bytes} bytes  ({:.1}/roundtrip)",
        bytes as f64 / ROUNDTRIPS as f64
    );
    println!(
        "  live blocks: {} -> {} (delta {live_delta}; dedup retention, not a leak)",
        before.curr_blocks, after.curr_blocks
    );
    println!("  peak live:   {} blocks / {} bytes", after.max_blocks, after.max_bytes);
    println!("  (run with `heap` arg to dump dhat-heap.json for the viewer)");
}
