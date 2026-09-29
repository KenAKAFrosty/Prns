use super::*;

#[test]
#[ignore = "process-isolated Embassy paired-fleet allocation probe"]
fn measure_embassy_fleet_heap() {
    const CHILD_SIZE: &str = "PRNS_EMBASSY_FLEET_HEAP_NODES";
    let Some(size) = std::env::var_os(CHILD_SIZE) else {
        for nodes in ["8", "16"] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--ignored",
                    "--exact",
                    "replay::fleet::heap::measure_embassy_fleet_heap",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(CHILD_SIZE, nodes)
                .output()
                .unwrap();
            print!("{}", String::from_utf8_lossy(&output.stdout));
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        return;
    };
    let nodes = match size.to_str() {
        Some("8") => 8,
        Some("16") => 16,
        _ => unreachable!("only declared bounded fleet sizes are measured"),
    };
    let profiler = dhat::Profiler::builder().testing().build();
    let transcript = match nodes {
        8 => run::<8>(ManualTaskScheduling::Cyclic, 42),
        16 => run::<16>(ManualTaskScheduling::Cyclic, 42),
        _ => unreachable!(),
    };
    let returned = dhat::HeapStats::get();
    drop(transcript);
    let released = dhat::HeapStats::get();
    let fixed = crate::static_storage::footprint();
    drop(profiler);
    assert_eq!(fixed.blocks, nodes * 8);
    assert!(released.curr_bytes >= fixed.bytes);
    println!("embassy_nodes={nodes} static_fixture_blocks={} static_fixture_bytes={} peak_bytes={} total_bytes={} allocations={} returned_bytes={} after_transcript_drop_bytes={}", fixed.blocks, fixed.bytes, returned.max_bytes, returned.total_bytes, returned.total_blocks, returned.curr_bytes, released.curr_bytes);
}
