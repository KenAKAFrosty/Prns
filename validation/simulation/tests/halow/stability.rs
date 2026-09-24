#[cfg(feature = "heap-profile")]
mod heap {
    use super::super::{fixture, workloads};
    const WORKER: &str = "stability::heap::thirty_two_cycles_release_halow_owners";
    const CYCLES: u64 = 32;
    const RETENTION_BUDGET: usize = 64 * 1024;
    const PEAK_BUDGET: usize = 32 * 1024 * 1024;
    #[test]
    #[ignore = "isolated HaLoW allocator and ownership qualification"]
    fn thirty_two_cycles_release_halow_owners() {
        if std::env::var_os("PRNS_HALOW_HEAP_WORKER").is_none() {
            let output =
                std::process::Command::new(std::env::current_exe().expect("probe executable"))
                    .args([
                        "--ignored",
                        "--exact",
                        WORKER,
                        "--nocapture",
                        "--test-threads=1",
                    ])
                    .env("PRNS_HALOW_HEAP_WORKER", "1")
                    .output()
                    .expect("isolated heap process");
            assert!(
                output.status.success(),
                "heap probe: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            print!("{}", String::from_utf8_lossy(&output.stdout));
            return;
        }
        let profiler = dhat::Profiler::builder().testing().build();
        for cycle in 0..CYCLES {
            let medium = fixture::medium();
            fixture::with_lab(medium.clone(), cycle, fixture::Topology::Shared, |lab| {
                workloads::overlap(lab);
                workloads::receive_pressure(lab);
                workloads::lifecycle(lab);
                lab.drained_metrics();
            });
            let snapshot = medium.snapshot();
            assert_eq!(
                (
                    snapshot.radios,
                    snapshot.queued,
                    snapshot.pending,
                    snapshot.armed_faults
                ),
                (0, 0, 0, 0)
            );
            drop(snapshot);
            drop(medium);
            let stats = dhat::HeapStats::get();
            assert!(
                stats.curr_bytes <= RETENTION_BUDGET,
                "cycle {cycle} retains {} bytes",
                stats.curr_bytes
            );
        }
        let stats = dhat::HeapStats::get();
        assert!(
            stats.max_bytes <= PEAK_BUDGET,
            "peak {} exceeds explicit budget {PEAK_BUDGET}",
            stats.max_bytes
        );
        println!(
            "{}",
            serde_json::json!({"cycles":CYCLES, "peak_bytes":stats.max_bytes, "retained_bytes":stats.curr_bytes, "retention_budget":RETENTION_BUDGET, "peak_budget":PEAK_BUDGET})
        );
        drop(profiler);
    }
}
