//! The energy prong of the three-pronged bench (conformance, performance, energy): RAPL
//! package-domain joules bracketing a scenario run. RAPL counts the whole package — both
//! contestants, the orchestrator's sampling, and everything else on the box — so the
//! method is stated plainly in the rows: a quiet-box baseline is sampled immediately
//! before the run, the run's raw joules are bracketed around the contestants' lifetime,
//! and the net figure subtracts baseline-rate × wall-time. Pinned cores keep the
//! remainder honest. Counters are root-locked on post-Platypus kernels; when unreadable,
//! detection returns `None` and the bench simply files no energy rows.

use std::path::PathBuf;
use std::time::{Duration, Instant};

/// One readable RAPL domain (we meter `package-0`: every core, cache, and memory
/// controller the contestants can touch — `psys` adds screen/SoC noise, per-core
/// subdomains undercount).
pub struct RaplMeter {
    energy_path: PathBuf,
    max_range_uj: u64,
}

/// A counter reading plus the wall-clock instant it was taken.
pub struct EnergySnapshot {
    energy_uj: u64,
    at: Instant,
}

impl RaplMeter {
    /// The package-0 domain, if this process can read it. `None` means the kernel has
    /// the counters root-locked — `sudo chmod o+r /sys/class/powercap/intel-rapl*/energy_uj`
    /// opens them until reboot.
    pub fn detect() -> Option<Self> {
        let base = PathBuf::from("/sys/class/powercap/intel-rapl:0");
        let energy_path = base.join("energy_uj");
        std::fs::read_to_string(&energy_path).ok()?;
        let max_range_uj = std::fs::read_to_string(base.join("max_energy_range_uj"))
            .ok()?
            .trim()
            .parse()
            .ok()?;
        Some(Self {
            energy_path,
            max_range_uj,
        })
    }

    fn read_uj(&self) -> u64 {
        std::fs::read_to_string(&self.energy_path)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0)
    }

    pub fn snapshot(&self) -> EnergySnapshot {
        EnergySnapshot {
            energy_uj: self.read_uj(),
            at: Instant::now(),
        }
    }

    /// Joules accumulated since `since`, wrap-corrected against the domain's range.
    pub fn joules_since(&self, since: &EnergySnapshot) -> f64 {
        let now = self.read_uj();
        let delta_uj = if now >= since.energy_uj {
            now - since.energy_uj
        } else {
            now + self.max_range_uj - since.energy_uj
        };
        delta_uj as f64 / 1_000_000.0
    }

    pub fn seconds_since(&self, since: &EnergySnapshot) -> f64 {
        since.at.elapsed().as_secs_f64()
    }

    /// The quiet box's draw, sampled over `window` — run this before spawning any
    /// contestant; it is the rate the net figure subtracts.
    pub fn idle_watts(&self, window: Duration) -> f64 {
        let start = self.snapshot();
        std::thread::sleep(window);
        self.joules_since(&start) / self.seconds_since(&start).max(f64::EPSILON)
    }
}
