//! Forward-path no-per-packet-allocation gate. A transport relay switching blind
//! ciphertext must not allocate per forwarded packet — its only heap traffic is
//! the amortized growth of the dedup and reverse-route stores. This drives the
//! relay past a warmup high-water mark, then measures only the relay ingest
//! (`Forward::forward`, the initiator seal excluded from the window) across a
//! window: a per-packet regression would show one-block-per-forward, so the gate
//! demands the window allocate far fewer blocks than it has forwards.

use benchmarks::microscope::Forward;

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

const WARMUP: usize = 4096;
const WINDOW: usize = 4096;

#[test]
fn forward_path_allocates_amortized_not_per_packet() {
    let _profiler = dhat::Profiler::builder().testing().build();

    let mut forward = Forward::new();
    for _ in 0..WARMUP {
        forward.seal_single();
        assert!(forward.forward(), "relay forwarded during warmup");
    }

    let mut blocks = 0u64;
    for _ in 0..WINDOW {
        forward.seal_single();
        let before = dhat::HeapStats::get();
        let forwarded = forward.forward();
        let after = dhat::HeapStats::get();
        assert!(forwarded, "relay forwarded the single");
        blocks += after.total_blocks - before.total_blocks;
    }

    assert!(
        blocks * 8 < WINDOW as u64,
        "relay forward must not allocate per packet: {blocks} block(s) over {WINDOW} forwards \
         (amortized growth is a handful; a per-packet regression would be >= {WINDOW})"
    );
}
