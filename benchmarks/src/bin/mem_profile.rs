//! Memory axis — static footprint + per-workload allocation snapshot (host route: dhat).
//!
//! Run: `cargo run --release --bin mem_profile`
//! (also writes `dhat-heap.json` for the DHAT viewer).

use benchmarks::{ingest_and_settle, load_corpus, scenario_dir, Cap};
use personal_rns::engine::EngineState;
use personal_rns::routing::storage::{EngineStorage, GrowableHeap};

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

const SETTLE_TICKS: usize = 64;

fn measure<S: EngineStorage>(label: &str, announces: &[Vec<u8>]) {
    // Clone the input buffers OUTSIDE the measured window — the engine wants `&mut`,
    // so the per-packet copy is harness cost, not engine cost.
    let mut owned = announces.to_vec();
    let before = dhat::HeapStats::get();
    let routes = ingest_and_settle::<S>(&mut owned, SETTLE_TICKS);
    let after = dhat::HeapStats::get();
    println!(
        "  {label:<14} routes={routes:<4} allocations={:<7} bytes={}",
        after.total_blocks - before.total_blocks,
        after.total_bytes - before.total_bytes,
    );
}

fn main() {
    let _profiler = dhat::Profiler::new_heap();
    let announces = load_corpus(&scenario_dir("announce-energy"));

    println!("engine static footprint (inline, no heap):");
    println!(
        "  size_of EngineState<Cap>          = {} bytes",
        core::mem::size_of::<EngineState<Cap>>()
    );
    println!(
        "  size_of EngineState<GrowableHeap> = {} bytes (handle; rows live on the heap)",
        core::mem::size_of::<EngineState<GrowableHeap>>()
    );

    println!(
        "\nheap allocation over {} announces + {SETTLE_TICKS} ticks:",
        announces.len()
    );
    measure::<GrowableHeap>("GrowableHeap", &announces);
    measure::<Cap>("FixedInline", &announces);

    let end = dhat::HeapStats::get();
    println!(
        "\nwhole-process peak heap: {} bytes (breakdown in dhat-heap.json)",
        end.max_bytes
    );
}
