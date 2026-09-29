use super::*;

#[test]
#[ignore = "isolated heap probe: run this exact test with --test-threads=1"]
fn measure_fleet_heap() {
    measure::<8>();
    measure::<32>();
    measure::<128>();
}

fn measure<const NODES: usize>() {
    let profiler = dhat::Profiler::builder().testing().build();
    let mut samples = Vec::with_capacity(phases::Phase::ALL.len());
    let transcript = run_observed::<NODES>(
        Inputs {
            scheduling: ManualTaskScheduling::Seeded {
                seed: SimulationSeed::new(7),
            },
            host_seed: 11,
            payload_marker: 42,
        },
        |phase| samples.push((phase, dhat::HeapStats::get())),
    );
    let retained = dhat::HeapStats::get();
    drop(transcript);
    let released = dhat::HeapStats::get();
    drop(profiler);
    for (phase, stats) in samples {
        println!("nodes={NODES} phase={phase:?} live_bytes={} peak_bytes={} total_bytes={} allocations={}", stats.curr_bytes, stats.max_bytes, stats.total_bytes, stats.total_blocks);
    }
    println!("nodes={NODES} returned_bytes={} after_transcript_drop_bytes={} peak_bytes={} total_bytes={} allocations={}", retained.curr_bytes, released.curr_bytes, retained.max_bytes, retained.total_bytes, retained.total_blocks);
}
