use super::*;

pub(super) struct RoleProcess {
    pub(super) child: Child,
    pub(super) lines: std_mpsc::Receiver<String>,
    #[cfg(target_os = "linux")]
    cpu_seconds: std::sync::Arc<std::sync::Mutex<f64>>,
    #[cfg(target_os = "linux")]
    peak_rss_bytes: std::sync::Arc<std::sync::Mutex<u64>>,
}

pub(super) struct RoleMetrics {
    pub(super) cpu_seconds: f64,
    pub(super) peak_rss_bytes: u64,
}
pub(super) fn spawn_role(
    base: Command,
    manifest: &std::path::Path,
    role: &str,
    addr: &str,
    args: &Args,
) -> RoleProcess {
    let mut command = base;
    command.arg(manifest).arg(role).arg(addr);
    if let Some(ms) = args.duration_ms {
        command.arg(ms.to_string());
    }
    let mut child = command
        .stdout(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("spawn {role}: {e}"));

    let stdout = child.stdout.take().expect("piped stdout");
    let (line_tx, lines) = std_mpsc::channel();
    let tag = role.to_string();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            println!("[{tag}] {line}");
            let _ = line_tx.send(line);
        }
    });

    #[cfg(target_os = "linux")]
    {
        let (cpu_seconds, peak_rss_bytes) = spawn_proc_sampler(child.id());
        RoleProcess {
            child,
            lines,
            cpu_seconds,
            peak_rss_bytes,
        }
    }
    #[cfg(not(target_os = "linux"))]
    RoleProcess { child, lines }
}

#[cfg(target_os = "linux")]
fn spawn_proc_sampler(
    pid: u32,
) -> (
    std::sync::Arc<std::sync::Mutex<f64>>,
    std::sync::Arc<std::sync::Mutex<u64>>,
) {
    let cpu_seconds = std::sync::Arc::new(std::sync::Mutex::new(0.0));
    let peak_rss_bytes = std::sync::Arc::new(std::sync::Mutex::new(0u64));
    let cpu = cpu_seconds.clone();
    let rss = peak_rss_bytes.clone();
    std::thread::spawn(move || loop {
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            return;
        };
        let after_comm = stat.rsplit(") ").next().unwrap_or("");
        let fields: Vec<&str> = after_comm.split_whitespace().collect();
        if let (Some(utime), Some(stime)) = (fields.get(11), fields.get(12)) {
            let ticks: u64 = utime.parse::<u64>().unwrap_or(0) + stime.parse::<u64>().unwrap_or(0);
            *cpu.lock().expect("cpu sample") = ticks as f64 / 100.0;
        }
        if let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/status")) {
            for line in status.lines() {
                if let Some(kb) = line.strip_prefix("VmHWM:") {
                    let kb: u64 = kb
                        .trim()
                        .trim_end_matches(" kB")
                        .trim()
                        .parse()
                        .unwrap_or(0);
                    *rss.lock().expect("rss sample") = kb * 1024;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    });
    (cpu_seconds, peak_rss_bytes)
}

impl RoleProcess {
    #[cfg(target_os = "linux")]
    pub(super) fn finalize(mut self) -> RoleMetrics {
        let cpu_seconds = *self.cpu_seconds.lock().expect("cpu");
        let peak_rss_bytes = *self.peak_rss_bytes.lock().expect("rss");
        let _ = self.child.wait();
        RoleMetrics {
            cpu_seconds,
            peak_rss_bytes,
        }
    }

    #[cfg(target_os = "macos")]
    pub(super) fn finalize(self) -> RoleMetrics {
        use std::mem::MaybeUninit;
        let pid = self.child.id() as libc::pid_t;
        let mut status: libc::c_int = 0;
        let mut usage = MaybeUninit::<libc::rusage>::zeroed();
        // SAFETY: pid is this live child, and status and usage are valid writable outputs for wait4.
        let reaped = unsafe { libc::wait4(pid, &mut status, 0, usage.as_mut_ptr()) };
        std::mem::forget(self);
        if reaped < 0 {
            return RoleMetrics {
                cpu_seconds: 0.0,
                peak_rss_bytes: 0,
            };
        }
        // SAFETY: a successful wait4 call initialized the rusage output.
        let usage = unsafe { usage.assume_init() };
        let secs = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1_000_000.0;
        RoleMetrics {
            cpu_seconds: secs(usage.ru_utime) + secs(usage.ru_stime),
            peak_rss_bytes: usage.ru_maxrss.max(0) as u64, // macOS reports ru_maxrss in bytes
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn finalize(mut self) -> RoleMetrics {
        let _ = self.child.wait();
        RoleMetrics {
            cpu_seconds: 0.0,
            peak_rss_bytes: 0,
        }
    }
}

impl Drop for RoleProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub(super) fn await_line(process: &RoleProcess, prefix: &str, within: Duration) -> String {
    let deadline = std::time::Instant::now() + within;
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match process.lines.recv_timeout(left) {
            Ok(line) if line.starts_with(prefix) => return line,
            Ok(_) => {}
            Err(_) => panic!("no {prefix:?} line within {within:?}"),
        }
    }
}
