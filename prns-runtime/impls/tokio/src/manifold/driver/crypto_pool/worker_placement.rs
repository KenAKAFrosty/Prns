use super::{CryptoJobClass, CryptoWorkerPlacement};

pub(super) fn performance_core_count() -> Option<usize> {
    #[cfg(target_os = "linux")]
    {
        let (performance, _) = linux_hybrid_cpu_sets()?;
        cpu_set_len(&performance)
    }
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("sysctl")
            .arg("-n")
            .arg("hw.perflevel0.logicalcpu")
            .output()
            .ok()?;
        output.status.success().then_some(())?;
        String::from_utf8(output.stdout).ok()?.trim().parse().ok()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CryptoWorkerRole {
    Shared,
    #[cfg(target_os = "linux")]
    Interactive,
    #[cfg(target_os = "linux")]
    Bulk,
}

impl CryptoWorkerRole {
    pub(super) fn accepts(self, _class: CryptoJobClass) -> bool {
        match self {
            Self::Shared => true,
            #[cfg(target_os = "linux")]
            Self::Interactive => _class != CryptoJobClass::Bulk,
            #[cfg(target_os = "linux")]
            Self::Bulk => _class == CryptoJobClass::Bulk,
        }
    }
}

#[derive(Clone)]
pub(super) enum CryptoWorkerAffinity {
    Unrestricted,
    #[cfg(target_os = "linux")]
    Linux {
        class: LinuxCpuClass,
        core_sets: std::sync::Arc<LinuxHybridCoreSets>,
    },
}

pub(super) struct CryptoWorkerAffinityError;

#[cfg(target_os = "linux")]
#[derive(Clone, Copy)]
pub(super) enum LinuxCpuClass {
    Performance,
    Efficiency,
}

#[cfg(target_os = "linux")]
pub(super) struct LinuxHybridCoreSets {
    performance: nix::sched::CpuSet,
    efficiency: nix::sched::CpuSet,
}

impl CryptoWorkerAffinity {
    pub(super) fn apply_to_current_thread(&self) -> Result<(), CryptoWorkerAffinityError> {
        match self {
            Self::Unrestricted => Ok(()),
            #[cfg(target_os = "linux")]
            Self::Linux { class, core_sets } => nix::sched::sched_setaffinity(
                nix::unistd::Pid::from_raw(0),
                match class {
                    LinuxCpuClass::Performance => &core_sets.performance,
                    LinuxCpuClass::Efficiency => &core_sets.efficiency,
                },
            )
            .map_err(|_| CryptoWorkerAffinityError),
        }
    }
}

pub(super) enum CryptoWorkerLayout {
    Shared,
    #[cfg(target_os = "linux")]
    Hybrid {
        interactive_workers: usize,
        core_sets: std::sync::Arc<LinuxHybridCoreSets>,
    },
}

impl CryptoWorkerLayout {
    pub(super) fn resolve(placement: CryptoWorkerPlacement, worker_count: usize) -> Self {
        if placement == CryptoWorkerPlacement::SchedulerManaged || worker_count < 2 {
            return Self::Shared;
        }
        #[cfg(target_os = "linux")]
        {
            if let Some((performance, efficiency)) = linux_hybrid_cpu_sets() {
                let Some(performance_cpus) = cpu_set_len(&performance) else {
                    return Self::Shared;
                };
                let Some(efficiency_cpus) = cpu_set_len(&efficiency) else {
                    return Self::Shared;
                };
                let interactive_workers =
                    interactive_worker_count(worker_count, performance_cpus, efficiency_cpus);
                return Self::Hybrid {
                    interactive_workers,
                    core_sets: std::sync::Arc::new(LinuxHybridCoreSets {
                        performance,
                        efficiency,
                    }),
                };
            }
        }
        Self::Shared
    }

    pub(super) fn role(&self, _worker: usize) -> CryptoWorkerRole {
        match self {
            Self::Shared => CryptoWorkerRole::Shared,
            #[cfg(target_os = "linux")]
            Self::Hybrid {
                interactive_workers,
                ..
            } if _worker < *interactive_workers => CryptoWorkerRole::Interactive,
            #[cfg(target_os = "linux")]
            Self::Hybrid { .. } => CryptoWorkerRole::Bulk,
        }
    }

    pub(super) fn affinity(&self, _worker: usize) -> CryptoWorkerAffinity {
        match self {
            Self::Shared => CryptoWorkerAffinity::Unrestricted,
            #[cfg(target_os = "linux")]
            Self::Hybrid {
                interactive_workers,
                core_sets,
            } => CryptoWorkerAffinity::Linux {
                class: if _worker < *interactive_workers {
                    LinuxCpuClass::Performance
                } else {
                    LinuxCpuClass::Efficiency
                },
                core_sets: core_sets.clone(),
            },
        }
    }

    pub(super) fn requires_affinity(&self) -> bool {
        match self {
            Self::Shared => false,
            #[cfg(target_os = "linux")]
            Self::Hybrid { .. } => true,
        }
    }
}

#[cfg(target_os = "linux")]
fn interactive_worker_count(
    worker_count: usize,
    performance_cpus: usize,
    efficiency_cpus: usize,
) -> usize {
    let desired = worker_count.div_ceil(3).min(worker_count.saturating_sub(1));
    let minimum = worker_count.saturating_sub(efficiency_cpus).max(1);
    let maximum = performance_cpus.min(worker_count.saturating_sub(1));
    if minimum > maximum {
        return desired;
    }
    desired.clamp(minimum, maximum)
}

#[cfg(target_os = "linux")]
fn linux_hybrid_cpu_sets() -> Option<(nix::sched::CpuSet, nix::sched::CpuSet)> {
    use nix::sched::sched_getaffinity;
    use nix::unistd::Pid;

    let original = sched_getaffinity(Pid::from_raw(0)).ok()?;
    let performance_ids = linux_performance_cpu_ids(&original)?;
    linux_hybrid_cpu_sets_from(original, &performance_ids)
}

#[cfg(target_os = "linux")]
fn linux_hybrid_cpu_sets_from(
    original: nix::sched::CpuSet,
    performance_ids: &[usize],
) -> Option<(nix::sched::CpuSet, nix::sched::CpuSet)> {
    use nix::sched::CpuSet;

    let mut performance = CpuSet::new();
    let mut efficiency = CpuSet::new();
    let mut performance_count = 0usize;
    let mut efficiency_count = 0usize;
    for cpu in 0..CpuSet::count() {
        if !original.is_set(cpu).ok()? {
            continue;
        }
        if performance_ids.binary_search(&cpu).is_ok() {
            performance.set(cpu).ok()?;
            performance_count += 1;
        } else {
            efficiency.set(cpu).ok()?;
            efficiency_count += 1;
        }
    }
    (performance_count > 0 && efficiency_count > 0).then_some((performance, efficiency))
}

#[cfg(target_os = "linux")]
fn cpu_set_len(cpus: &nix::sched::CpuSet) -> Option<usize> {
    let mut count = 0usize;
    for cpu in 0..nix::sched::CpuSet::count() {
        count += usize::from(cpus.is_set(cpu).ok()?);
    }
    (count > 0).then_some(count)
}

#[cfg(target_os = "linux")]
fn linux_performance_cpu_ids(allowed: &nix::sched::CpuSet) -> Option<Vec<usize>> {
    std::fs::read_to_string("/sys/devices/cpu_core/cpus")
        .ok()
        .and_then(|raw| parse_linux_cpu_list(&raw))
        .or_else(|| linux_highest_capacity_cpu_ids(allowed))
}

#[cfg(target_os = "linux")]
fn linux_highest_capacity_cpu_ids(allowed: &nix::sched::CpuSet) -> Option<Vec<usize>> {
    let mut capacities = Vec::new();
    for cpu in 0..nix::sched::CpuSet::count() {
        if !allowed.is_set(cpu).ok()? {
            continue;
        }
        let capacity =
            std::fs::read_to_string(format!("/sys/devices/system/cpu/cpu{cpu}/cpu_capacity"))
                .ok()
                .and_then(|raw| raw.trim().parse::<usize>().ok());
        if let Some(capacity) = capacity {
            capacities.push((cpu, capacity));
        }
    }
    let highest = capacities.iter().map(|(_, capacity)| *capacity).max()?;
    let performance: Vec<usize> = capacities
        .iter()
        .filter_map(|(cpu, capacity)| (*capacity == highest).then_some(*cpu))
        .collect();
    (performance.len() < capacities.len()).then_some(performance)
}

#[cfg(target_os = "linux")]
fn parse_linux_cpu_list(raw: &str) -> Option<Vec<usize>> {
    let mut cpus = Vec::new();
    for span in raw.trim().split(',') {
        let mut bounds = span.split('-');
        let first = bounds.next()?.trim().parse::<usize>().ok()?;
        let last = bounds
            .next()
            .map(str::trim)
            .map(str::parse::<usize>)
            .transpose()
            .ok()?
            .unwrap_or(first);
        if bounds.next().is_some() || first > last || last >= nix::sched::CpuSet::count() {
            return None;
        }
        cpus.extend(first..=last);
    }
    cpus.sort_unstable();
    cpus.dedup();
    (!cpus.is_empty()).then_some(cpus)
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    use super::{
        interactive_worker_count, linux_hybrid_cpu_sets_from, parse_linux_cpu_list,
        CryptoWorkerRole,
    };
    #[cfg(target_os = "linux")]
    use crate::manifold::driver::crypto_pool::CryptoJobClass;
    #[cfg(target_os = "linux")]
    use nix::sched::CpuSet;

    #[cfg(target_os = "linux")]
    #[test]
    fn interactive_worker_share_preserves_both_roles() {
        let counts: Vec<usize> = (1..=8)
            .map(|workers| interactive_worker_count(workers, usize::MAX, usize::MAX))
            .collect();
        assert_eq!(counts, vec![0, 1, 1, 2, 2, 2, 3, 3]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn worker_share_respects_each_cpu_class_capacity() {
        assert_eq!(interactive_worker_count(8, 8, 8), 3);
        assert_eq!(interactive_worker_count(8, 8, 2), 6);
        assert_eq!(interactive_worker_count(6, 2, 8), 2);
        assert_eq!(interactive_worker_count(7, 8, 1), 6);
        assert_eq!(interactive_worker_count(4, 1, 1), 2);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn worker_roles_accept_only_their_scheduling_classes() {
        assert!(CryptoWorkerRole::Shared.accepts(CryptoJobClass::Verify));
        assert!(CryptoWorkerRole::Shared.accepts(CryptoJobClass::Latency));
        assert!(CryptoWorkerRole::Shared.accepts(CryptoJobClass::Bulk));
        assert!(CryptoWorkerRole::Interactive.accepts(CryptoJobClass::Verify));
        assert!(CryptoWorkerRole::Interactive.accepts(CryptoJobClass::Latency));
        assert!(!CryptoWorkerRole::Interactive.accepts(CryptoJobClass::Bulk));
        assert!(!CryptoWorkerRole::Bulk.accepts(CryptoJobClass::Verify));
        assert!(!CryptoWorkerRole::Bulk.accepts(CryptoJobClass::Latency));
        assert!(CryptoWorkerRole::Bulk.accepts(CryptoJobClass::Bulk));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn hybrid_cpu_sets_intersect_the_process_affinity() {
        let mut allowed = CpuSet::new();
        let mut expected_performance = CpuSet::new();
        let mut expected_efficiency = CpuSet::new();
        for cpu in [2, 4, 8] {
            assert_eq!(allowed.set(cpu), Ok(()));
        }
        for cpu in [2, 4] {
            assert_eq!(expected_performance.set(cpu), Ok(()));
        }
        assert_eq!(expected_efficiency.set(8), Ok(()));
        assert_eq!(
            linux_hybrid_cpu_sets_from(allowed, &[0, 1, 2, 3, 4, 5, 6, 7]),
            Some((expected_performance, expected_efficiency))
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn homogeneous_process_affinity_rejects_hybrid_placement() {
        let mut performance_only = CpuSet::new();
        assert_eq!(performance_only.set(2), Ok(()));
        assert_eq!(
            linux_hybrid_cpu_sets_from(performance_only, &[0, 1, 2, 3]),
            None
        );

        let mut efficiency_only = CpuSet::new();
        assert_eq!(efficiency_only.set(8), Ok(()));
        assert_eq!(
            linux_hybrid_cpu_sets_from(efficiency_only, &[0, 1, 2, 3]),
            None
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cpu_lists_expand_ranges_and_remove_duplicates() {
        assert_eq!(
            parse_linux_cpu_list("0-3,2,8-9\n"),
            Some(vec![0, 1, 2, 3, 8, 9])
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn malformed_or_reversed_cpu_lists_are_rejected() {
        assert_eq!(parse_linux_cpu_list("3-1"), None);
        assert_eq!(parse_linux_cpu_list("0-1-2"), None);
        assert_eq!(parse_linux_cpu_list(""), None);
    }
}
