//! Heap-allocation microscope: the relay forwarding path under dhat. The
//! initiator seal that mints each fresh SINGLE is kept *outside* the measured
//! window — only `Forward::forward` (the relay ingesting and re-emitting the
//! packet) is bracketed — so the per-forward figure is the transport hot path
//! alone. The `heap` arg dumps `dhat-heap.json` (seal + forward, separable by
//! call site in the viewer). Owns its own global allocator.

use std::env;

use benchmarks::microscope::Forward;

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

const FORWARDS: usize = 2000;

fn main() {
    if env::args().nth(1).as_deref() == Some("heap") {
        let _profiler = dhat::Profiler::new_heap();
        let mut forward = Forward::new();
        for _ in 0..FORWARDS {
            forward.seal_single();
            assert!(forward.forward(), "relay forwarded the single");
        }
        eprintln!(
            "wrote dhat-heap.json ({FORWARDS} forwards incl. initiator seals) — open at \
             https://nnethercote.github.io/dh_view/dh_view.html"
        );
        return;
    }

    let _profiler = dhat::Profiler::builder().testing().build();
    let mut forward = Forward::new();
    for _ in 0..4 {
        forward.seal_single();
        assert!(forward.forward(), "relay forwarded the single during warmup");
    }

    let mut blocks = 0u64;
    let mut bytes = 0u64;
    for _ in 0..FORWARDS {
        forward.seal_single();
        let before = dhat::HeapStats::get();
        let forwarded = forward.forward();
        let after = dhat::HeapStats::get();
        assert!(forwarded, "relay forwarded the single");
        blocks += after.total_blocks - before.total_blocks;
        bytes += after.total_bytes - before.total_bytes;
    }

    println!("relay forward path — dhat heap, {FORWARDS} forwards (initiator seal excluded)");
    println!(
        "  allocations: {blocks} blocks  ({:.3}/forward)",
        blocks as f64 / FORWARDS as f64
    );
    println!(
        "  bytes:       {bytes} bytes  ({:.1}/forward)",
        bytes as f64 / FORWARDS as f64
    );
    println!("  (run with `heap` arg to dump dhat-heap.json for the viewer)");
}
