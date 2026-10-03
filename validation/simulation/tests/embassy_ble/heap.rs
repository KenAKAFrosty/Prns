#[test]
#[ignore = "process-isolated Embassy fixture heap probe"]
fn measure_embassy_fixture_heap() {
    const CHILD: &str = "PRNS_EMBASSY_HEAP_PROBE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "heap::measure_embassy_fixture_heap",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        print!("{}", String::from_utf8_lossy(&output.stdout));
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let profiler = dhat::Profiler::builder().testing().build();
    let transcript = super::replay::run(42);
    let retained = dhat::HeapStats::get();
    drop(transcript);
    let released = dhat::HeapStats::get();
    let fixed = super::static_storage::footprint();
    drop(profiler);
    assert_eq!(fixed.blocks, 16);
    assert!(released.curr_bytes >= fixed.bytes);
    println!("embassy_nodes=2 static_fixture_blocks={} static_fixture_bytes={} peak_bytes={} total_bytes={} allocations={} returned_bytes={} after_transcript_drop_bytes={}", fixed.blocks, fixed.bytes, retained.max_bytes, retained.total_bytes, retained.total_blocks, retained.curr_bytes, released.curr_bytes);
}
