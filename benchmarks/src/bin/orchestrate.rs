//! The impl-neutral orchestrator for the live scenarios: spawn one process per role, point
//! them at each other over localhost TCP/UDP, collect the line protocol, and file result rows
//! — throughput, conformance, latency, memory, and energy — into the one substrate. A *run* is
//! an assignment of implementations to the scenario's roles; `self/self` is both the ceiling
//! measurement and the harness ceiling itself, and other pairings join by naming a different
//! participation binary for a role. Energy is bracketed on every run, so efficiency
//! (millijoules per delivered message) falls out of the realistic firehose itself. CPU and
//! peak RSS are sampled *outside* the contestants (Linux from `/proc`, macOS from each child's
//! `wait4` rusage), so a participation binary can't flatter itself.
//!
//! usage: orchestrate [scenario] [--initiator self|reference|…] [--responder …]
//!                     [--duration-ms N] [--unpinned]
//!
//! The reproducibility profile is ON by default: Linux pins each role to one physical core's
//! SMT siblings (`taskset`); Apple silicon has no per-core affinity (the arm64 API is a
//! documented no-op), so the contestants run on the Performance cluster by default on a quiet
//! box, with the P/E split recorded in `host.json`. `--unpinned` skips the profile and prints
//! without filing — unprofiled runs proved non-reproducible on hybrid-core silicon.

use std::ffi::OsString;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc as std_mpsc;
use std::time::Duration;

use benchmarks::scenario_dir;
use benchmarks::{
    energy_unavailable_hint, load_host, load_or_create_submitter_id, write_rows, Axis, DeviceId,
    PowerMeter, ResultRow, SubmitterId,
};

/// One physical core's SMT siblings per role, read from the live topology — pinned runs
/// are per-physical-core figures by construction. Falls back to the first two thread
/// pairs when /sys is absent. Linux-only: it feeds `taskset`, which Apple silicon has no
/// equivalent of.
#[cfg(target_os = "linux")]
fn role_cores(role: &str) -> String {
    let sets = benchmarks::load_host(&rustc_host_triple())
        .and_then(|h| h.pinned_sibling_sets)
        .unwrap_or_else(|| vec!["0,1".into(), "2,3".into()]);
    if role == "responder" {
        sets.first().cloned().unwrap_or_else(|| "0,1".into())
    } else {
        sets.get(1).cloned().unwrap_or_else(|| "2,3".into())
    }
}

fn rustc_host_triple() -> String {
    command_line("rustc", &["-vV"])
        .and_then(|v| {
            v.lines()
                .find_map(|l| l.strip_prefix("host: ").map(str::to_string))
        })
        .unwrap_or_else(|| "unknown-host".into())
}

struct RoleProcess {
    child: Child,
    lines: std_mpsc::Receiver<String>,
    /// Linux samples CPU + peak RSS by polling `/proc/<pid>` into these; macOS reaps the
    /// same two figures from the child's `wait4` rusage at the end, so it carries no live
    /// sampler state.
    #[cfg(target_os = "linux")]
    cpu_seconds: std::sync::Arc<std::sync::Mutex<f64>>,
    #[cfg(target_os = "linux")]
    peak_rss_bytes: std::sync::Arc<std::sync::Mutex<u64>>,
}

/// What the orchestrator measures about a role from the outside: total CPU seconds and the
/// lifetime peak RSS, gathered without the participation binary's cooperation.
struct RoleMetrics {
    cpu_seconds: f64,
    peak_rss_bytes: u64,
}

fn sibling_binary(name: &str) -> std::path::PathBuf {
    let mut path = std::env::current_exe().expect("own path");
    path.set_file_name(name);
    path
}

/// The reference's pinned venv python (falling back to `python3`) running one of its
/// `reference/*.py` participation scripts under `-u` — unbuffered, so the line protocol
/// streams a line at a time.
fn reference_python(script: &str) -> Command {
    let reference = Path::new(env!("CARGO_MANIFEST_DIR")).join("reference");
    let venv_python = reference.join(".venv").join("bin").join("python");
    let python: OsString = if venv_python.exists() {
        venv_python.into_os_string()
    } else {
        OsString::from("python3")
    };
    let mut c = Command::new(python);
    c.arg("-u");
    c.arg(reference.join(script));
    c
}

/// A participating implementation: its registry name, the slug its result files key on, and
/// the display label its rows carry. Its interop command is `Some` only when the impl actually
/// fields a node for the live scenarios.
struct Implementation {
    name: &'static str,
    slug: &'static str,
    label: &'static str,
}

fn implementation(name: &str) -> Implementation {
    match name {
        "self" => Implementation {
            name: "self",
            slug: "personal-rns",
            label: "Prns",
        },
        "reference" => Implementation {
            name: "reference",
            slug: "rns-1.3.1",
            label: "RNS 1.3.1",
        },
        other => panic!("unknown implementation {other:?} (self|reference)"),
    }
}

impl Implementation {
    /// The two-node interop participation command (initiator/responder over the wire), or
    /// `None` if this impl fields no interop node.
    fn interop_command(&self) -> Option<Command> {
        match self.name {
            "self" => Some(Command::new(sibling_binary("scenario_node"))),
            "reference" => Some(reference_python("scenario_node.py")),
            _ => None,
        }
    }
}

/// The wrapper that applies the reproducibility profile to a role's command, or `None` to
/// launch it bare. Linux prepends `taskset -c <sibling-set>` when profiling; Apple silicon
/// has no per-core pin (the arm64 affinity API is a no-op), so its profile is topological —
/// the Performance cluster on a quiet box — and there is nothing to wrap.
#[cfg(target_os = "linux")]
fn role_launch_wrapper(role: &str, profile: bool) -> Option<(OsString, Vec<OsString>)> {
    profile.then(|| {
        (
            OsString::from("taskset"),
            vec![OsString::from("-c"), OsString::from(role_cores(role))],
        )
    })
}

#[cfg(not(target_os = "linux"))]
fn role_launch_wrapper(_role: &str, _profile: bool) -> Option<(OsString, Vec<OsString>)> {
    None
}

fn spawn_role(
    base: Command,
    manifest: &std::path::Path,
    role: &str,
    addr: &str,
    args: &Args,
) -> RoleProcess {
    let mut command = match role_launch_wrapper(role, args.pin) {
        Some((wrapper, wrapper_args)) => {
            let mut c = Command::new(wrapper);
            c.args(wrapper_args);
            c.arg(base.get_program());
            c.args(base.get_args());
            c
        }
        None => base,
    };
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
        return RoleProcess {
            child,
            lines,
            cpu_seconds,
            peak_rss_bytes,
        };
    }
    #[cfg(not(target_os = "linux"))]
    return RoleProcess { child, lines };
}

/// Poll `/proc/<pid>` every 100 ms for the child's CPU ticks and RSS high-water mark,
/// feeding shared cells the orchestrator reads once the run ends. The thread returns the
/// moment the process exits and `/proc/<pid>/stat` disappears.
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
    /// Wait for the role to exit and harvest its CPU + peak-RSS figures. Linux reads the
    /// poller's last values, then reaps; macOS reaps with `wait4` and reads the kernel's
    /// own `ru_maxrss` (bytes) + `ru_utime`/`ru_stime` — an exact lifetime peak, no polling.
    #[cfg(target_os = "linux")]
    fn finalize(mut self) -> RoleMetrics {
        let cpu_seconds = *self.cpu_seconds.lock().expect("cpu");
        let peak_rss_bytes = *self.peak_rss_bytes.lock().expect("rss");
        let _ = self.child.wait();
        RoleMetrics {
            cpu_seconds,
            peak_rss_bytes,
        }
    }

    #[cfg(target_os = "macos")]
    fn finalize(self) -> RoleMetrics {
        use std::mem::MaybeUninit;
        let pid = self.child.id() as libc::pid_t;
        let mut status: libc::c_int = 0;
        let mut usage = MaybeUninit::<libc::rusage>::zeroed();
        let reaped = unsafe { libc::wait4(pid, &mut status, 0, usage.as_mut_ptr()) };
        if reaped < 0 {
            return RoleMetrics {
                cpu_seconds: 0.0,
                peak_rss_bytes: 0,
            };
        }
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

fn await_line(process: &RoleProcess, prefix: &str, within: Duration) -> String {
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

fn field(line: &str, key: &str) -> Option<f64> {
    line.split_whitespace()
        .find_map(|kv| kv.strip_prefix(&format!("{key}=")))
        .and_then(|v| v.parse().ok())
}

struct Args {
    scenario: String,
    initiator: String,
    responder: String,
    duration_ms: Option<u64>,
    pin: bool,
}

fn parse_args() -> Args {
    let mut args = Args {
        scenario: "single-firehose".into(),
        initiator: "self".into(),
        responder: "self".into(),
        duration_ms: None,
        pin: true,
    };
    let mut argv = std::env::args().skip(1);
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--initiator" => args.initiator = argv.next().expect("impl name"),
            "--responder" => args.responder = argv.next().expect("impl name"),
            "--duration-ms" => {
                args.duration_ms = Some(argv.next().and_then(|v| v.parse().ok()).expect("ms"));
            }
            "--unpinned" => args.pin = false,
            other if !other.starts_with("--") => args.scenario = other.into(),
            other => panic!("unknown flag {other}"),
        }
    }
    args
}

fn command_line(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Two distinct OS-assigned UDP ports — datagrams have no connect, so a UDP pairing's
/// both ends are fixed before either node starts (the reference's `forward_ip:port`
/// model). Both sockets are held until both ports are read, so they can't collide.
fn udp_port_pair() -> (u16, u16) {
    let first = std::net::UdpSocket::bind("127.0.0.1:0").expect("probes a udp port");
    let second = std::net::UdpSocket::bind("127.0.0.1:0").expect("probes a udp port");
    (
        first.local_addr().expect("bound").port(),
        second.local_addr().expect("bound").port(),
    )
}

/// The reproducibility stamp every row carries: the host triple, the engine commit and
/// toolchain that produced the figure, and the device + submitter join keys.
struct RunStamp {
    host: String,
    commit: String,
    toolchain: String,
    device_id: Option<DeviceId>,
    submitter_id: Option<SubmitterId>,
}

fn run_stamp(pin: bool) -> RunStamp {
    let host = rustc_host_triple();
    assert!(
        !(pin && host == "unknown-host"),
        "host triple unresolved — `rustc` is not on PATH (common under `sudo`, which resets it). \
         Re-run as `sudo env \"PATH=$PATH\" ...` so rows don't file under `unknown-host`.",
    );
    RunStamp {
        // The device id rides along from `host.json` (run `describe_host` to register a
        // machine); the submitter id is this checkout's own. Both stamp every row.
        device_id: load_host(&host).and_then(|descriptor| descriptor.device_id),
        submitter_id: Some(load_or_create_submitter_id()),
        commit: command_line("git", &["rev-parse", "--short", "HEAD"]).unwrap_or_default(),
        toolchain: command_line("rustc", &["--version"]).unwrap_or_default(),
        host,
    }
}

fn main() {
    let args = parse_args();
    let manifest = scenario_dir(&args.scenario).join("manifest.json");
    assert!(manifest.exists(), "no manifest at {}", manifest.display());
    let manifest_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest).expect("reads the manifest"))
            .expect("parses the manifest");
    run_interop(&args, &manifest_json, &manifest);
}

/// Spawn one process per role over localhost TCP/UDP and collect delivery,
/// latency, memory, and energy from the protocol's own proofs. Files under the `<initiator>--
/// <responder>` pairing key; `--unpinned` prints without filing.
fn run_interop(args: &Args, manifest_json: &serde_json::Value, manifest: &std::path::Path) {
    let wire = manifest_json["profile"]["wire"].as_str().unwrap_or("tcp");
    let version = manifest_json["version"].as_u64().unwrap_or(1) as u32;

    let initiator_impl = implementation(&args.initiator);
    let responder_impl = implementation(&args.responder);
    let pairing_slug = format!("{}--{}", initiator_impl.slug, responder_impl.slug);
    let pairing_label = format!("{} \u{2192} {}", initiator_impl.label, responder_impl.label);
    let interop_command = |subject: &Implementation| {
        subject
            .interop_command()
            .unwrap_or_else(|| panic!("implementation {:?} fields no interop node", subject.name))
    };

    let (responder_addr, initiator_addr) = if wire == "udp" {
        let (responder_port, initiator_port) = udp_port_pair();
        (
            format!("127.0.0.1:{responder_port}>127.0.0.1:{initiator_port}"),
            Some(format!(
                "127.0.0.1:{initiator_port}>127.0.0.1:{responder_port}"
            )),
        )
    } else {
        ("127.0.0.1:0".to_string(), None)
    };

    // The energy prong: baseline the quiet box BEFORE any contestant exists, then bracket the
    // contestants' whole lifetime; the baseline-net figure is the honest one.
    let meter = PowerMeter::detect();
    if meter.is_none() {
        println!("{}", energy_unavailable_hint());
    }
    let idle_watts = meter
        .as_ref()
        .map(|m| m.idle_watts(Duration::from_millis(1500)));
    let bracket = meter.as_ref().map(|m| m.start());

    let responder = spawn_role(
        interop_command(&responder_impl),
        manifest,
        "responder",
        &responder_addr,
        args,
    );
    let ready = await_line(&responder, "READY", Duration::from_secs(10));
    let addr = initiator_addr.unwrap_or_else(|| {
        ready
            .split_whitespace()
            .find_map(|kv| kv.strip_prefix("addr="))
            .expect("responder READY carries addr")
            .to_string()
    });

    let initiator = spawn_role(
        interop_command(&initiator_impl),
        manifest,
        "initiator",
        &addr,
        args,
    );
    let window = Duration::from_millis(args.duration_ms.unwrap_or(10_000) + 30_000);
    let result = await_line(&initiator, "RESULT", window);
    let responder_result = await_line(&responder, "RESULT", Duration::from_secs(10));

    let energy = bracket.map(|b| b.finish());

    let initiator_metrics = initiator.finalize();
    let responder_metrics = responder.finalize();
    let initiator_cpu = initiator_metrics.cpu_seconds;
    let initiator_rss = initiator_metrics.peak_rss_bytes;
    let responder_cpu = responder_metrics.cpu_seconds;
    let responder_rss = responder_metrics.peak_rss_bytes;

    let sent = field(&result, "sent").unwrap_or(0.0);
    let delivered = field(&result, "delivered").unwrap_or(0.0);
    let timeouts = field(&result, "timeouts").unwrap_or(f64::NAN);
    let responder_delivered = field(&responder_result, "delivered").unwrap_or(0.0);
    assert!(
        delivered <= responder_delivered && responder_delivered <= sent,
        "delivery accounting holds (initiator-proven <= responder-seen <= sent): \
         {delivered} <= {responder_delivered} <= {sent} — a proof can conclude after \
         the initiator's receipt timeout, so the responder may see more than settles",
    );

    let stamp = run_stamp(args.pin);
    let row = |axis: Axis, metric: &str, value: Option<f64>, unit: &str| ResultRow {
        scenario: args.scenario.clone(),
        scenario_version: version,
        implementation: pairing_label.clone(),
        commit: stamp.commit.clone(),
        toolchain: stamp.toolchain.clone(),
        host: stamp.host.clone(),
        axis,
        metric: metric.into(),
        value,
        unit: unit.into(),
        device_id: stamp.device_id,
        submitter_id: stamp.submitter_id,
    };
    let mut rows = vec![
        row(
            Axis::Conformance,
            "settled_clean",
            Some(f64::from(sent == delivered && timeouts == 0.0)),
            "bool",
        ),
        row(Axis::Conformance, "sent", Some(sent), "msgs"),
        row(Axis::Conformance, "delivered", Some(delivered), "msgs"),
        row(
            Axis::Conformance,
            "responder_delivered",
            Some(responder_delivered),
            "msgs",
        ),
        row(Axis::Conformance, "timed_out", Some(timeouts), "msgs"),
        row(
            Axis::Throughput,
            "delivered_per_sec",
            field(&result, "delivered_per_sec"),
            "msgs/s",
        ),
        row(
            Axis::Throughput,
            "goodput_bytes_per_sec",
            field(&result, "goodput_bytes_per_sec"),
            "B/s",
        ),
        row(
            Axis::Latency,
            "rtt_p50_ms",
            field(&result, "rtt_p50_ms"),
            "ms",
        ),
        row(
            Axis::Latency,
            "rtt_p99_ms",
            field(&result, "rtt_p99_ms"),
            "ms",
        ),
        row(
            Axis::Memory,
            "initiator_peak_rss_bytes",
            Some(initiator_rss as f64),
            "bytes",
        ),
        row(
            Axis::Memory,
            "responder_peak_rss_bytes",
            Some(responder_rss as f64),
            "bytes",
        ),
    ];
    if let (Some((raw_joules, wall_seconds)), Some(idle_watts)) = (energy, idle_watts) {
        let net_joules = (raw_joules - idle_watts * wall_seconds).max(0.0);
        let per_delivered_mj = (delivered > 0.0).then(|| net_joules * 1_000.0 / delivered);
        rows.push(row(
            Axis::Energy,
            "package_joules_raw",
            Some(raw_joules),
            "J",
        ));
        rows.push(row(
            Axis::Energy,
            "idle_baseline_watts",
            Some(idle_watts),
            "W",
        ));
        rows.push(row(Axis::Energy, "net_joules", Some(net_joules), "J"));
        rows.push(row(
            Axis::Energy,
            "net_millijoules_per_delivered",
            per_delivered_mj,
            "mJ/msg",
        ));
        println!(
            "\nSUMMARY energy raw={raw_joules:.1}J over {wall_seconds:.1}s \
             (idle {idle_watts:.2}W) | net={net_joules:.1}J | {:.2} mJ/msg",
            per_delivered_mj.unwrap_or(f64::NAN),
        );
    }
    println!(
        "\nSUMMARY scenario={} pairing={pairing_label} host={}\n\
         SUMMARY conformance sent={sent:.0} delivered={delivered:.0} \
         responder_seen={responder_delivered:.0} timed_out={timeouts:.0} settled_clean={}\n\
         SUMMARY initiator cpu={initiator_cpu:.2}s peak_rss={:.1}MiB | \
         responder cpu={responder_cpu:.2}s peak_rss={:.1}MiB",
        args.scenario,
        stamp.host,
        sent == delivered && timeouts == 0.0,
        initiator_rss as f64 / (1024.0 * 1024.0),
        responder_rss as f64 / (1024.0 * 1024.0),
    );
    if args.pin {
        write_rows(&stamp.host, &args.scenario, &pairing_slug, &rows);
        println!(
            "SUMMARY rows filed under results/{}/{}/{pairing_slug}.jsonl",
            stamp.host, args.scenario,
        );
    } else {
        println!("UNPINNED run: rows printed, not filed (re-run without --unpinned to file)");
    }
}
