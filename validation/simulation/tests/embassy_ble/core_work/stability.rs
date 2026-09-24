#[cfg(feature = "heap-profile")]
mod heap {
    use super::super::{
        fixture,
        profile::{self, Profile},
        workloads,
    };
    use std::process::Command;
    const PROFILE: &str = "PRNS_CORE_STABILITY_PROFILE";
    const WORKER: &str = "core_work::stability::heap::thirty_two_cycles_release_dynamic_owners";
    const CYCLES: u64 = 32;
    const AUXILIARY_RETENTION_BUDGET: usize = 64 * 1024;
    const PEAK_BUDGET: usize = 32 * 1024 * 1024;
    #[test]
    #[ignore = "process-isolated core resource and heap qualification"]
    fn thirty_two_cycles_release_dynamic_owners() {
        let Some(input) = std::env::var_os(PROFILE) else {
            for profile in profile::all() {
                let output = Command::new(std::env::current_exe().expect("probe executable"))
                    .args([
                        "--ignored",
                        "--exact",
                        WORKER,
                        "--nocapture",
                        "--test-threads=1",
                    ])
                    .env(
                        PROFILE,
                        serde_json::to_string(&profile).expect("profile encoding"),
                    )
                    .output()
                    .expect("isolated probe");
                assert!(
                    output.status.success(),
                    "probe {profile:?}: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                print!("{}", String::from_utf8_lossy(&output.stdout));
            }
            return;
        };
        let profile: Profile = serde_json::from_str(input.to_str().expect("profile argument"))
            .expect("profile schema");
        let profiler = dhat::Profiler::builder().testing().build();
        let mut intentional_static_bytes = 0;
        let mut intentional_static_blocks = 0;
        for cycle in 0..CYCLES {
            let (_, trace) = fixture::with_triple(profile.clone(), cycle, |triple| {
                workloads::overlap(triple, cycle as u8);
                workloads::refused_response(triple);
                workloads::reply_fault(triple, workloads::ReplyFault::Cancel);
                workloads::watch(triple);
                workloads::worker(triple, workloads::WorkerDisposition::CloseLink);
                triple.restart(fixture::TARGET);
                workloads::inventory(triple);
                for node in 0..3 {
                    super::super::campaign::execute::metrics(triple, node);
                }
                let timers = triple.tasks.timer_stats();
                assert!(timers.pending <= timers.capacity);
                assert!(timers.peak <= timers.capacity);
            });
            intentional_static_bytes += trace.retained_static.bytes;
            intentional_static_blocks += trace.retained_static.blocks;
            drop(trace);
            let stats = dhat::HeapStats::get();
            assert!(
                stats.curr_bytes <= intentional_static_bytes + AUXILIARY_RETENTION_BUDGET,
                "cycle {cycle}, current {}, static {}, bounded auxiliary {}",
                stats.curr_bytes,
                intentional_static_bytes,
                AUXILIARY_RETENTION_BUDGET
            );
        }
        let stats = dhat::HeapStats::get();
        // Leaked Embassy fixture wiring accumulates across fresh fixtures. The peak budget applies to dynamic work above that explicitly measured retention.
        assert!(
            stats.max_bytes <= intentional_static_bytes + PEAK_BUDGET,
            "peak {}, measured static {}, dynamic budget {}",
            stats.max_bytes,
            intentional_static_bytes,
            PEAK_BUDGET
        );
        println!("profile={profile:?} cycles={CYCLES} peak_bytes={} retained_bytes={} intentional_static_bytes={intentional_static_bytes} intentional_static_blocks={intentional_static_blocks}", stats.max_bytes, stats.curr_bytes);
        drop(profiler);
    }
}
