use std::time::Duration;

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

    fn rapl_domains() -> Vec<(PathBuf, PathBuf)> {
        let mut domains = std::fs::read_dir("/sys/class/powercap")
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .map(|path| (path.join("energy_uj"), path.join("max_energy_range_uj")))
            .filter(|(energy, range)| energy.exists() && range.exists())
            .collect::<Vec<_>>();
        domains.sort();
        domains
    }

    pub fn unavailable_hint() -> String {
        let paths = rapl_domains()
            .into_iter()
            .map(|(energy, _)| energy.display().to_string())
            .collect::<Vec<_>>();
        if paths.is_empty() {
            return "ENERGY unavailable: no RAPL energy_uj counters were exposed under /sys/class/powercap"
                .into();
        }
        format!(
            "ENERGY unavailable: detected unreadable counters {}; grant this user temporary read access with `sudo chmod o+r {}`",
            paths.join(", "),
            paths.join(" ")
        )
    }

    /// One readable RAPL domain (we meter `package-0`: every core, cache, and memory
    /// controller the contestants can touch — `psys` adds screen/SoC noise, per-core
    /// subdomains undercount).
    pub struct PowerMeter {
        energy_path: PathBuf,
        max_range_uj: u64,
    }

    pub struct EnergyBracket<'m> {
        meter: &'m PowerMeter,
        start_uj: u64,
        at: Instant,
    }

    impl PowerMeter {
        pub fn detect() -> Option<Self> {
            rapl_domains()
                .into_iter()
                .find_map(|(energy_path, range_path)| {
                    std::fs::read_to_string(&energy_path).ok()?;
                    let max_range_uj = std::fs::read_to_string(range_path)
                        .ok()?
                        .trim()
                        .parse()
                        .ok()?;
                    Some(Self {
                        energy_path,
                        max_range_uj,
                    })
                })
        }

        fn read_uj(&self) -> u64 {
            std::fs::read_to_string(&self.energy_path)
                .ok()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0)
        }

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

        pub fn start(&self) -> EnergyBracket<'_> {
            EnergyBracket {
                meter: self,
                start_uj: self.read_uj(),
                at: Instant::now(),
            }
        }
    }

    impl EnergyBracket<'_> {
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

    pub fn unavailable_hint() -> String {
        "ENERGY unavailable: macOS power counters need authorization — run \
         `cargo benchmark --energy`; only powermetrics is launched through sudo"
            .into()
    }

    pub struct PowerMeter {
        via_sudo: bool,
    }

    pub struct EnergyBracket {
        sampler: Option<Child>,
        reader: Option<JoinHandle<()>>,
        acc: Arc<Mutex<(f64, u64)>>,
        at: Instant,
    }

    impl PowerMeter {
        /// The frontend authorizes sudo once, then marks only the powermetrics subprocess for
        /// privilege. Cargo, Python, caches, participants, and result files stay user-owned.
        pub fn detect() -> Option<Self> {
            if unsafe { libc::geteuid() } == 0 {
                return Some(Self { via_sudo: false });
            }
            (std::env::var_os("BENCHMARK_POWER_VIA_SUDO").as_deref()
                == Some(std::ffi::OsStr::new("1")))
            .then_some(Self { via_sudo: true })
        }

        fn command(&self) -> Command {
            if self.via_sudo {
                let mut command = Command::new("sudo");
                command.args(["-n", "powermetrics"]);
                command
            } else {
                Command::new("powermetrics")
            }
        }

        pub fn idle_watts(&self, window: Duration) -> f64 {
            let samples = (window.as_millis() as u64 / SAMPLE_MS).max(1);
            let output = self
                .command()
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

        pub fn start(&self) -> EnergyBracket {
            let acc = Arc::new(Mutex::new((0.0f64, 0u64)));
            let mut child = self
                .command()
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

    fn watts(sum_mw: f64, count: u64) -> f64 {
        if count > 0 {
            (sum_mw / count as f64) / 1_000.0
        } else {
            0.0
        }
    }

    #[cfg(test)]
    mod tests {
        use super::cpu_power_mw;

        #[test]
        fn reads_cpu_power_without_counting_combined_power() {
            assert_eq!(cpu_power_mw("CPU Power: 1086 mW"), Some(1086.0));
            assert_eq!(
                cpu_power_mw("Combined Power (CPU + GPU + ANE): 1086 mW"),
                None
            );
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod unsupported {
    use super::Duration;

    pub fn unavailable_hint() -> String {
        if cfg!(windows) {
            "ENERGY unsupported on Windows; throughput, RTT, conformance, and initiator/responder peak working-set evidence remain available".into()
        } else {
            "ENERGY unavailable: no power-counter backend for this platform".into()
        }
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
