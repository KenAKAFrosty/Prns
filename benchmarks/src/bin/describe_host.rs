//! Describe the machine this host runs on — CPU model, core counts, memory, OS — and write
//! `results/<host>/host.json`, the per-host reproducibility descriptor beside the figure
//! rows. The figure drivers stamp the host *triple*; this records what silicon that triple
//! actually was, so a throughput number reproduces (an M1 and an M4 Max are both
//! `aarch64-apple-darwin`). Run once per machine, before committing that host's results.
//!
//! Run: `cargo run --release --bin describe_host`

use std::process::Command;

use benchmarks::{load_host, load_or_create_submitter_id, write_host, DeviceId, HostDescriptor};
use sysinfo::System;

fn main() {
    let host = rustc_host().unwrap_or_else(|| "unknown".into());

    // The device id is minted once and preserved across runs — re-describing a machine must
    // not change its identity, or its older figures stop joining to it.
    let device_id = load_host(&host)
        .and_then(|existing| existing.device_id)
        .unwrap_or_else(DeviceId::generate);
    let submitter_id = load_or_create_submitter_id();

    let mut sys = System::new();
    sys.refresh_cpu_all();
    sys.refresh_memory();

    let descriptor = HostDescriptor {
        host: host.clone(),
        device_id: Some(device_id),
        cpu_model: sys
            .cpus()
            .first()
            .map(|cpu| cpu.brand().trim().to_string())
            .filter(|brand| !brand.is_empty()),
        physical_cores: sys.physical_core_count().map(|n| n as u32),
        logical_cores: nonzero(sys.cpus().len() as u32),
        total_memory_bytes: (sys.total_memory() > 0).then(|| sys.total_memory()),
        os_version: System::long_os_version().filter(|v| !v.is_empty()),
        kernel_version: System::kernel_version().filter(|v| !v.is_empty()),
        cpu_governor: sysfs("devices/system/cpu/cpu0/cpufreq/scaling_governor"),
        cpu_max_mhz: sysfs("devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq")
            .and_then(|khz| khz.parse::<u32>().ok())
            .map(|khz| khz / 1000),
        pinned_sibling_sets: sibling_sets(),
        performance_cores: sysctl_u32("hw.perflevel0.physicalcpu"),
        efficiency_cores: sysctl_u32("hw.perflevel1.physicalcpu"),
    };
    write_host(&descriptor);

    println!("described host `{host}` -> results/{host}/host.json");
    println!(
        "  cpu     {}",
        descriptor.cpu_model.as_deref().unwrap_or("unknown")
    );
    println!(
        "  cores   {} physical / {} logical",
        opt(descriptor.physical_cores),
        opt(descriptor.logical_cores),
    );
    println!(
        "  memory  {}",
        descriptor
            .total_memory_bytes
            .map(gib)
            .unwrap_or_else(|| "unknown".into()),
    );
    println!(
        "  os      {}",
        descriptor.os_version.as_deref().unwrap_or("unknown")
    );
    println!(
        "  kernel  {}",
        descriptor.kernel_version.as_deref().unwrap_or("unknown")
    );
    println!(
        "  freq    {} MHz max, governor {}",
        opt(descriptor.cpu_max_mhz),
        descriptor.cpu_governor.as_deref().unwrap_or("unknown"),
    );
    println!("  profile {}", describe_profile(&descriptor));
    println!("  device  {}", device_id.0);
    println!(
        "  submitter {} (this checkout — .submitter.json, gitignored)",
        submitter_id.0
    );
}

/// How the orchestrator makes this host's figures reproducible, in one line — the Linux
/// `taskset` sibling sets, or the Apple-silicon Performance cluster (arm64 has no per-core
/// affinity, so the P/E split is the reproducibility fact instead).
fn describe_profile(d: &HostDescriptor) -> String {
    if let Some(sets) = &d.pinned_sibling_sets {
        return format!("taskset sibling sets {}", sets.join(" | "));
    }
    if let Some(performance) = d.performance_cores {
        let efficiency = d
            .efficiency_cores
            .map(|e| format!(", {e} efficiency"))
            .unwrap_or_default();
        return format!(
            "Performance cluster — {performance} cores{efficiency} (arm64 has no per-core affinity)"
        );
    }
    "unknown".into()
}

/// One integer `sysctl` value (macOS topology like `hw.perflevel0.physicalcpu`); `None`
/// elsewhere or when the key is absent.
fn sysctl_u32(key: &str) -> Option<u32> {
    let out = Command::new("sysctl").arg("-n").arg(key).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// One trimmed line out of /sys (Linux); None elsewhere or when absent.
fn sysfs(path: &str) -> Option<String> {
    std::fs::read_to_string(format!("/sys/{path}"))
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// The first two distinct physical cores' SMT sibling sets — what the orchestrator pins
/// the two roles to, recorded so a filed figure names the exact CPUs it ran on.
fn sibling_sets() -> Option<Vec<String>> {
    let first = sysfs("devices/system/cpu/cpu0/topology/thread_siblings_list")?;
    let taken: Vec<u32> = first
        .split(|c| c == ',' || c == '-')
        .filter_map(|n| n.parse().ok())
        .collect();
    let next = (0..64).filter(|n| !taken.contains(n)).find_map(|n| {
        sysfs(&format!(
            "devices/system/cpu/cpu{n}/topology/thread_siblings_list"
        ))
    })?;
    Some(vec![first, next])
}

fn nonzero(n: u32) -> Option<u32> {
    (n > 0).then_some(n)
}

fn opt(n: Option<u32>) -> String {
    n.map(|v| v.to_string()).unwrap_or_else(|| "?".into())
}

fn gib(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
}

/// The rustc target triple — the same canonical host id the figure drivers stamp, so the
/// descriptor files under the same `results/<host>/` dir.
fn rustc_host() -> Option<String> {
    let out = Command::new("rustc").arg("-vV").output().ok()?;
    out.status.success().then(|| {
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .find_map(|l| l.strip_prefix("host: ").map(str::to_string))
    })?
}
