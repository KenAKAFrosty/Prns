//! The energy prong of the three-pronged bench (conformance, performance, energy): package
//! power bracketing a scenario run, behind one seam with a backend per platform. The method
//! is the same shape on every host — a quiet-box idle baseline sampled immediately before the
//! run, the run's raw energy bracketed around the contestants' lifetime, and the filed net
//! figure subtracting baseline-rate × wall-time — only the counter underneath differs:
//!
//! - **Linux** reads RAPL package-domain joules directly (`/sys/class/powercap/intel-rapl:0`),
//!   a free-running µJ counter we delta across the bracket.
//! - **macOS** integrates `powermetrics` CPU-power samples over the bracket (the same sampler
//!   `energy/measure.sh` uses for `announce-energy`): a streaming root process averages
//!   `CPU Power: N mW`, and energy is that average × wall-time.
//!
//! Either counter is privileged: RAPL is root-locked on post-Platypus kernels, and
//! `powermetrics` needs `sudo`. When the platform's counter isn't readable, [`PowerMeter::detect`]
//! returns `None`, [`unavailable_hint`] tells the operator how to open it, and the orchestrator
//! simply files no energy rows — every other axis still lands.

use std::time::Duration;

// `EnergyBracket` is `PowerMeter::start`'s return type — reachable through that signature,
// so it needs no name of its own at this level (the orchestrator holds it by inference).
#[cfg(target_os = "linux")]
pub use linux::{unavailable_hint, PowerMeter};
#[cfg(target_os = "macos")]
pub use macos::{unavailable_hint, PowerMeter};
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub use unsupported::{unavailable_hint, PowerMeter};

#[cfg(target_os = "linux")]
mod linux {
    use super::Duration;
    use std::path::PathBuf;
    use std::time::Instant;

    pub fn unavailable_hint() -> &'static str {
        "ENERGY unavailable: RAPL counters are root-locked — \
         `sudo chmod o+r /sys/class/powercap/intel-rapl*/energy_uj` opens them until reboot"
    }

    /// One readable RAPL domain (we meter `package-0`: every core, cache, and memory
    /// controller the contestants can touch — `psys` adds screen/SoC noise, per-core
    /// subdomains undercount).
    pub struct PowerMeter {
        energy_path: PathBuf,
        max_range_uj: u64,
    }

    /// An open bracket: the counter reading and wall-clock instant the run began.
    pub struct EnergyBracket<'m> {
        meter: &'m PowerMeter,
        start_uj: u64,
        at: Instant,
    }

    impl PowerMeter {
        /// The package-0 domain, if this process can read it. `None` means the kernel has
        /// the counters root-locked.
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

        /// Joules accumulated since `start_uj`, wrap-corrected against the domain's range.
        fn delta_joules(&self, start_uj: u64) -> f64 {
            let now = self.read_uj();
            let delta_uj = if now >= start_uj {
                now - start_uj
            } else {
                now + self.max_range_uj - start_uj
            };
            delta_uj as f64 / 1_000_000.0
        }

        /// The quiet box's draw, sampled over `window` — run this before spawning any
        /// contestant; it is the rate the net figure subtracts.
        pub fn idle_watts(&self, window: Duration) -> f64 {
            let start_uj = self.read_uj();
            let at = Instant::now();
            std::thread::sleep(window);
            self.delta_joules(start_uj) / at.elapsed().as_secs_f64().max(f64::EPSILON)
        }

        /// Open a bracket around the run; [`EnergyBracket::finish`] closes it.
        pub fn start(&self) -> EnergyBracket<'_> {
            EnergyBracket {
                meter: self,
                start_uj: self.read_uj(),
                at: Instant::now(),
            }
        }
    }

    impl EnergyBracket<'_> {
        /// `(raw joules over the bracket, wall seconds)`.
        pub fn finish(self) -> (f64, f64) {
            (
                self.meter.delta_joules(self.start_uj),
                self.at.elapsed().as_secs_f64(),
            )
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::Duration;
    use std::io::{BufRead, BufReader};
    use std::process::{Child, Command, Stdio};
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;
    use std::time::Instant;

    /// `powermetrics`' sample cadence — matches `energy/measure.sh`'s 250 ms closely enough
    /// while keeping enough samples in a ~10 s firehose for a steady average.
    const SAMPLE_MS: u64 = 200;

    pub fn unavailable_hint() -> &'static str {
        "ENERGY unavailable: macOS power counters need root — re-run the orchestrator under \
         `sudo` to include the energy axis (powermetrics, the same sampler as energy/measure.sh)"
    }

    /// The macOS power counter is `powermetrics`; metering it is a privilege gate, not a path.
    pub struct PowerMeter {
        _private: (),
    }

    /// An open bracket: a streaming `powermetrics` child whose `CPU Power` samples a reader
    /// thread folds into a running `(sum_mw, count)`, plus the instant the run began.
    pub struct EnergyBracket {
        sampler: Option<Child>,
        reader: Option<JoinHandle<()>>,
        acc: Arc<Mutex<(f64, u64)>>,
        at: Instant,
    }

    impl PowerMeter {
        /// `powermetrics` reads privileged SMC counters, so effective-root is the gate — the
        /// honest macOS mirror of RAPL being root-locked on Linux.
        pub fn detect() -> Option<Self> {
            (unsafe { libc::geteuid() } == 0).then_some(Self { _private: () })
        }

        /// The quiet box's draw, sampled over `window` via a bounded `powermetrics` run.
        pub fn idle_watts(&self, window: Duration) -> f64 {
            let samples = (window.as_millis() as u64 / SAMPLE_MS).max(1);
            let output = Command::new("powermetrics")
                .args([
                    "--samplers",
                    "cpu_power",
                    "-i",
                    &SAMPLE_MS.to_string(),
                    "-n",
                    &samples.to_string(),
                ])
                .stderr(Stdio::null())
                .output();
            match output {
                Ok(out) if out.status.success() => {
                    let (sum_mw, count) = String::from_utf8_lossy(&out.stdout)
                        .lines()
                        .filter_map(cpu_power_mw)
                        .fold((0.0, 0u64), |(s, c), mw| (s + mw, c + 1));
                    watts(sum_mw, count)
                }
                _ => 0.0,
            }
        }

        /// Open a bracket: spawn a streaming `powermetrics` and a reader thread that folds its
        /// `CPU Power` samples as they arrive (draining the pipe so the sampler never blocks).
        pub fn start(&self) -> EnergyBracket {
            let acc = Arc::new(Mutex::new((0.0f64, 0u64)));
            let mut child = Command::new("powermetrics")
                .args(["--samplers", "cpu_power", "-i", &SAMPLE_MS.to_string()])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .ok();
            let reader = child.as_mut().and_then(|c| c.stdout.take()).map(|stdout| {
                let acc = acc.clone();
                std::thread::spawn(move || {
                    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                        if let Some(mw) = cpu_power_mw(&line) {
                            let mut g = acc.lock().expect("power accumulator");
                            g.0 += mw;
                            g.1 += 1;
                        }
                    }
                })
            });
            EnergyBracket {
                sampler: child,
                reader,
                acc,
                at: Instant::now(),
            }
        }
    }

    impl EnergyBracket {
        /// `(raw joules over the bracket, wall seconds)` — average CPU power × wall-time, the
        /// same integration `energy/measure.sh` does (powermetrics gives power, not a counter).
        pub fn finish(mut self) -> (f64, f64) {
            let seconds = self.at.elapsed().as_secs_f64();
            if let Some(mut child) = self.sampler.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            if let Some(reader) = self.reader.take() {
                let _ = reader.join();
            }
            let (sum_mw, count) = *self.acc.lock().expect("power accumulator");
            (watts(sum_mw, count) * seconds, seconds)
        }
    }

    /// `CPU Power: N mW` → `N` (milliwatts), or `None` for any other line. Scans for the
    /// `mW` token and reads the value before it — the same shape `energy/measure.sh`'s awk
    /// uses, so spacing quirks in powermetrics output can't throw it off.
    fn cpu_power_mw(line: &str) -> Option<f64> {
        if !line.contains("CPU Power:") {
            return None;
        }
        let tokens: Vec<&str> = line.split_whitespace().collect();
        let mw_index = tokens.iter().position(|t| *t == "mW")?;
        tokens.get(mw_index.checked_sub(1)?)?.parse().ok()
    }

    /// Mean of `count` milliwatt samples, in watts; `0.0` when nothing was sampled.
    fn watts(sum_mw: f64, count: u64) -> f64 {
        if count > 0 {
            (sum_mw / count as f64) / 1_000.0
        } else {
            0.0
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod unsupported {
    use super::Duration;

    pub fn unavailable_hint() -> &'static str {
        "ENERGY unavailable: no power-counter backend for this platform"
    }

    pub struct PowerMeter {
        _private: (),
    }

    pub struct EnergyBracket {
        _private: (),
    }

    impl PowerMeter {
        pub fn detect() -> Option<Self> {
            None
        }
        pub fn idle_watts(&self, _window: Duration) -> f64 {
            0.0
        }
        pub fn start(&self) -> EnergyBracket {
            EnergyBracket { _private: () }
        }
    }

    impl EnergyBracket {
        pub fn finish(self) -> (f64, f64) {
            (0.0, 0.0)
        }
    }
}
