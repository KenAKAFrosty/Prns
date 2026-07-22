use super::*;
use std::io::Write as _;

pub(super) struct RoleProcess {
    pub(super) child: Child,
    pub(super) lines: std_mpsc::Receiver<String>,
    stdin: std::process::ChildStdin,
    measurement_cpu_start: f64,
    measurement_cpu_end: Option<f64>,
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
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("spawn {role}: {error}"));

    let stdin = child.stdin.take().expect("piped stdin");
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
            stdin,
            measurement_cpu_start: 0.0,
            measurement_cpu_end: None,
            cpu_seconds,
            peak_rss_bytes,
        }
    }
    #[cfg(not(target_os = "linux"))]
    RoleProcess {
        child,
        lines,
        stdin,
        measurement_cpu_start: 0.0,
        measurement_cpu_end: None,
    }
}

impl RoleProcess {
    pub(super) fn start_startup(&mut self) {
        self.stdin
            .write_all(b"STARTUP\n")
            .expect("send startup command");
        self.stdin.flush().expect("flush startup command");
    }

    pub(super) fn mark_measurement_start(&mut self) {
        self.measurement_cpu_start = self.current_cpu_seconds();
    }

    pub(super) fn start_measurement(&mut self) {
        self.stdin
            .write_all(b"START\n")
            .expect("send measurement command");
        self.stdin.flush().expect("flush measurement command");
    }

    pub(super) fn set_collection_target(&mut self, transfers: u64, bytes: u64) {
        writeln!(self.stdin, "COLLECT {transfers} {bytes}").expect("send collection target");
        self.stdin.flush().expect("flush collection target");
    }

    pub(super) fn release_collection(&mut self) {
        self.stdin
            .write_all(b"COLLECTED\n")
            .expect("send collection release");
        self.stdin.flush().expect("flush collection release");
    }

    pub(super) fn mark_measurement_end(&mut self) {
        self.measurement_cpu_end = Some(self.current_cpu_seconds());
    }

    fn measured_cpu_seconds(&self, fallback_end: f64) -> f64 {
        (self.measurement_cpu_end.unwrap_or(fallback_end) - self.measurement_cpu_start).max(0.0)
    }

    #[cfg(target_os = "linux")]
    fn current_cpu_seconds(&self) -> f64 {
        *self.cpu_seconds.lock().expect("cpu sample")
    }

    #[cfg(target_os = "macos")]
    fn current_cpu_seconds(&self) -> f64 {
        macos_cpu_seconds(self.child.id())
    }

    #[cfg(windows)]
    fn current_cpu_seconds(&self) -> f64 {
        windows_metrics(&self.child).0
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    fn current_cpu_seconds(&self) -> f64 {
        0.0
    }

    #[cfg(target_os = "linux")]
    pub(super) fn finalize(mut self) -> RoleMetrics {
        let _ = self.child.wait();
        let total_cpu = *self.cpu_seconds.lock().expect("cpu");
        let cpu_seconds = self.measured_cpu_seconds(total_cpu);
        let peak_rss_bytes = *self.peak_rss_bytes.lock().expect("rss");
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
        // SAFETY: pid is this live child, and status and usage are valid writable outputs.
        let reaped = unsafe { libc::wait4(pid, &mut status, 0, usage.as_mut_ptr()) };
        if reaped < 0 {
            return RoleMetrics {
                cpu_seconds: 0.0,
                peak_rss_bytes: 0,
            };
        }
        // SAFETY: successful wait4 initialized the rusage output.
        let usage = unsafe { usage.assume_init() };
        let seconds = |time: libc::timeval| time.tv_sec as f64 + time.tv_usec as f64 / 1_000_000.0;
        let total_cpu = seconds(usage.ru_utime) + seconds(usage.ru_stime);
        RoleMetrics {
            cpu_seconds: self.measured_cpu_seconds(total_cpu),
            peak_rss_bytes: usage.ru_maxrss.max(0) as u64,
        }
    }

    #[cfg(windows)]
    pub(super) fn finalize(mut self) -> RoleMetrics {
        let _ = self.child.wait();
        let (total_cpu, peak_rss_bytes) = windows_metrics(&self.child);
        RoleMetrics {
            cpu_seconds: self.measured_cpu_seconds(total_cpu),
            peak_rss_bytes,
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    pub(super) fn finalize(mut self) -> RoleMetrics {
        let _ = self.child.wait();
        RoleMetrics {
            cpu_seconds: 0.0,
            peak_rss_bytes: 0,
        }
    }
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
    // SAFETY: sysconf is a read-only libc query.
    let ticks_per_second = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as f64;
    std::thread::spawn(move || loop {
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            return;
        };
        let after_comm = stat.rsplit(") ").next().unwrap_or("");
        let fields: Vec<&str> = after_comm.split_whitespace().collect();
        if let (Some(utime), Some(stime)) = (fields.get(11), fields.get(12)) {
            let ticks: u64 = utime.parse::<u64>().unwrap_or(0) + stime.parse::<u64>().unwrap_or(0);
            *cpu.lock().expect("cpu sample") = ticks as f64 / ticks_per_second;
        }
        if let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/status")) {
            for line in status.lines() {
                if let Some(kibibytes) = line.strip_prefix("VmHWM:") {
                    let kibibytes = kibibytes
                        .trim()
                        .trim_end_matches(" kB")
                        .trim()
                        .parse::<u64>()
                        .unwrap_or(0);
                    *rss.lock().expect("rss sample") = kibibytes * 1024;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    });
    (cpu_seconds, peak_rss_bytes)
}

#[cfg(target_os = "macos")]
fn macos_cpu_seconds(pid: u32) -> f64 {
    use std::mem::MaybeUninit;

    let mut usage = MaybeUninit::<libc::rusage_info_v2>::zeroed();
    // SAFETY: the buffer matches RUSAGE_INFO_V2 and pid names our child.
    let result = unsafe {
        libc::proc_pid_rusage(
            pid as libc::c_int,
            libc::RUSAGE_INFO_V2,
            usage.as_mut_ptr().cast(),
        )
    };
    if result != 0 {
        return 0.0;
    }
    // SAFETY: successful proc_pid_rusage initialized the buffer.
    let usage = unsafe { usage.assume_init() };
    macos_ticks_to_seconds(usage.ri_user_time + usage.ri_system_time)
}

#[cfg(target_os = "macos")]
fn macos_ticks_to_seconds(ticks: u64) -> f64 {
    #[repr(C)]
    struct MachTimebaseInfo {
        numer: u32,
        denom: u32,
    }
    unsafe extern "C" {
        fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
    }

    let mut info = MachTimebaseInfo { numer: 0, denom: 0 };
    // SAFETY: info is a valid writable mach_timebase_info record.
    let status = unsafe { mach_timebase_info(&mut info) };
    if status != 0 || info.denom == 0 {
        return 0.0;
    }
    ticks as f64 * f64::from(info.numer) / f64::from(info.denom) / 1_000_000_000.0
}

#[cfg(windows)]
fn windows_metrics(child: &Child) -> (f64, u64) {
    use std::mem::{size_of, zeroed};
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetProcessTimes;

    let handle = child.as_raw_handle() as *mut std::ffi::c_void;
    // SAFETY: all structures are plain Windows output records and the process handle is live.
    let mut creation: FILETIME = unsafe { zeroed() };
    let mut exit: FILETIME = unsafe { zeroed() };
    let mut kernel: FILETIME = unsafe { zeroed() };
    let mut user: FILETIME = unsafe { zeroed() };
    let times_ok =
        unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) } != 0;
    let filetime = |value: FILETIME| {
        ((u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime)) as f64
            / 10_000_000.0
    };
    let cpu = times_ok
        .then(|| filetime(kernel) + filetime(user))
        .unwrap_or(0.0);

    // SAFETY: zero is a valid initial representation for PROCESS_MEMORY_COUNTERS.
    let mut memory: PROCESS_MEMORY_COUNTERS = unsafe { zeroed() };
    memory.cb = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    let memory_ok = unsafe {
        K32GetProcessMemoryInfo(
            handle,
            &mut memory,
            size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        )
    } != 0;
    (
        cpu,
        memory_ok
            .then_some(memory.PeakWorkingSetSize as u64)
            .unwrap_or(0),
    )
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
