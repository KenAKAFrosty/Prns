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

/// A built external-port interop node: `external/<impl>/interop/<binary>`, produced by
/// `build.sh` against that port's pinned upstream.
fn external_node(impl_dir: &str, binary: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("external")
        .join(impl_dir)
        .join("interop")
        .join(binary)
}

/// The reference's pinned venv python (falling back to `python3`) running one of its
/// `reference/*.py` participation scripts under `-u` — unbuffered, so the line protocol
/// streams a line at a time.
fn reference_python(script: &str) -> Command {
    let reference = Path::new(env!("CARGO_MANIFEST_DIR")).join("reference");
    let python: OsString = std::env::var_os("RNS_REFERENCE_PYTHON")
        .filter(|p| Path::new(p).exists())
        .or_else(|| {
            [
                reference.join(".venv").join("bin").join("python"),
                reference.join(".venv").join("Scripts").join("python.exe"),
            ]
            .into_iter()
            .find(|p| p.exists())
            .map(|p| p.into_os_string())
        })
        .unwrap_or_else(|| OsString::from("python3"));
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
    /// The interop roles this impl can field. A pairing is only run when the initiator
    /// declares `initiator` and the responder declares `responder` — an impl that fields a
    /// partial node (e.g. rns-cr, which sends one-shot singles but cannot prove them as a
    /// responder, nor field links) declares only the side it can honestly drive.
    interop_roles: &'static [&'static str],
    /// The interop mechanisms this impl can field on the roles above, or `None` for a full impl
    /// that fields every mechanism (so new scenarios reach self/reference without a registry
    /// edit). Only a partial external node whitelists the mechanisms it actually implements.
    interop_mechanisms: Option<&'static [&'static str]>,
    /// When set, this impl only interoperates with itself — its wire sub-protocol for the
    /// mechanism diverges from the others, so cross-impl pairings are skipped. "link" is not one
    /// protocol across the family: go proves single-style link packets, Leviculum carries a
    /// Channel multiplexer, and Prns/LXMF-rs exchange plain link data. LXMF-rs's link only lines
    /// up with itself (and one-directionally with Prns), so it runs as a self-pair ceiling.
    interop_self_only: bool,
}

const BOTH_ROLES: &[&str] = &["initiator", "responder"];
const BOTH_MECHANISMS: &[&str] = &["single", "link"];

fn implementation(name: &str) -> Implementation {
    match name {
        "self" => Implementation {
            name: "self",
            slug: "personal-rns",
            label: "Prns",
            interop_roles: BOTH_ROLES,
            interop_mechanisms: None,
            interop_self_only: false,
        },
        "reference" => Implementation {
            name: "reference",
            slug: "rns-1.3.5",
            label: "RNS 1.3.5",
            interop_roles: BOTH_ROLES,
            interop_mechanisms: None,
            interop_self_only: false,
        },
        "go-reticulum" => Implementation {
            name: "go-reticulum",
            slug: "go-reticulum",
            label: "go-reticulum",
            interop_roles: BOTH_ROLES,
            interop_mechanisms: Some(BOTH_MECHANISMS),
            interop_self_only: false,
        },
        "leviculum" => Implementation {
            name: "leviculum",
            slug: "leviculum",
            label: "Leviculum 0.6.3",
            interop_roles: BOTH_ROLES,
            interop_mechanisms: Some(BOTH_MECHANISMS),
            interop_self_only: false,
        },
        // rns-cr (Crystal) fields only a single-mechanism initiator at its current commit: its
        // single responder never proves incoming packets (an unimplemented strategy) and its
        // link layer never resolves a data-packet proof, so it can drive neither role over a
        // link nor the responder side of a single. We run it where it is honest — and only
        // there — rather than patch its protocol.
        "rns-cr" => Implementation {
            name: "rns-cr",
            slug: "rns-cr",
            label: "rns-cr 0.1.0",
            interop_roles: &["initiator"],
            interop_mechanisms: Some(&["single"]),
            interop_self_only: false,
        },
        // LXMF-rs fields links (both roles) but not single-packet proofs, and its link wire —
        // plain link data, no surfaced per-message proof — interoperates only with itself (and
        // one-directionally with Prns). go's single-style link proofs and Leviculum's Channel
        // multiplexer don't line up with it, so it runs as a self-pair link ceiling. See the
        // `interop_self_only` note on the struct field for the wider "link isn't one protocol"
        // dynamic this surfaces.
        "lxmf-rs" => Implementation {
            name: "lxmf-rs",
            slug: "lxmf-rs",
            label: "LXMF-rs 0.2.0",
            interop_roles: BOTH_ROLES,
            interop_mechanisms: Some(&["link"]),
            interop_self_only: true,
        },
        other => {
            panic!(
                "unknown implementation {other:?} \
                 (self|reference|go-reticulum|leviculum|rns-cr|lxmf-rs)"
            )
        }
    }
}

/// Why a pairing cannot be honestly run, or `None` when both sides can field their role for the
/// scenario's mechanism. Lets the matrix sweep every cell while skipping the ones an impl's
/// partial node would only hang or falsify.
fn unsupported_pairing(
    initiator: &Implementation,
    responder: &Implementation,
    mechanism: &str,
) -> Option<String> {
    if initiator.interop_mechanisms.is_some_and(|m| !m.contains(&mechanism)) {
        return Some(format!("{} fields no {mechanism} node", initiator.name));
    }
    if !initiator.interop_roles.contains(&"initiator") {
        return Some(format!("{} fields no initiator", initiator.name));
    }
    if responder.interop_mechanisms.is_some_and(|m| !m.contains(&mechanism)) {
        return Some(format!("{} fields no {mechanism} node", responder.name));
    }
    if !responder.interop_roles.contains(&"responder") {
        return Some(format!("{} fields no responder", responder.name));
    }
    if (initiator.interop_self_only || responder.interop_self_only)
        && initiator.name != responder.name
    {
        let odd = if initiator.interop_self_only { initiator.name } else { responder.name };
        return Some(format!(
            "{odd}'s {mechanism} wire interoperates only with itself (the mechanism is not one \
             protocol across impls)"
        ));
    }
    None
}

impl Implementation {
    /// The two-node interop participation command (initiator/responder over the wire), or
    /// `None` if this impl fields no interop node.
    fn interop_command(&self) -> Option<Command> {
        match self.name {
            "self" => Some(Command::new(sibling_binary("scenario_node"))),
            "reference" => Some(reference_python("scenario_node.py")),
            "go-reticulum" => Some(Command::new(external_node("go-reticulum", "go-node"))),
            "leviculum" => Some(Command::new(external_node("leviculum", "leviculum-node"))),
            "rns-cr" => Some(Command::new(external_node("rns-cr", "rnscr-node"))),
            "lxmf-rs" => Some(Command::new(external_node("lxmf-rs", "lxmf-node"))),
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
        std::mem::forget(self);
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

/// Any exit path that abandons a role — the no-RESULT panic included — must not orphan its
/// child: an orphaned responder keeps announcing into later runs' measurements. A finalized
/// role was already reaped, so the kill is a no-op there (macOS reaps outside std's
/// bookkeeping and forgets self instead — its pid may already be recycled).
impl Drop for RoleProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
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
    relay: String,
    duration_ms: Option<u64>,
    pin: bool,
}

fn parse_args() -> Args {
    let mut args = Args {
        scenario: "single-firehose".into(),
        initiator: "self".into(),
        responder: "self".into(),
        relay: "self".into(),
        duration_ms: None,
        pin: true,
    };
    let mut argv = std::env::args().skip(1);
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--initiator" => args.initiator = argv.next().expect("impl name"),
            "--responder" => args.responder = argv.next().expect("impl name"),
            "--relay" => args.relay = argv.next().expect("impl name"),
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
    if manifest_json["profile"]["topology"].as_str() == Some("relay") {
        run_relay_interop(&args, &manifest_json, &manifest);
    } else {
        run_interop(&args, &manifest_json, &manifest);
    }
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

    let mechanism = manifest_json["profile"]["mechanism"].as_str().unwrap_or("single");
    if let Some(reason) = unsupported_pairing(&initiator_impl, &responder_impl, mechanism) {
        println!("SKIP scenario={} pairing={pairing_label} reason={reason}", args.scenario);
        return;
    }
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

    // A manifest with a wire_shape gets the shaped pipe spliced between the endpoints:
    // infrastructure, not a contestant — the initiator dials the pipe, the pipe dials the
    // responder, and every wire byte is counted on the way through.
    let (pipe, addr) = if manifest_json["profile"]["wire_shape"].is_object() {
        assert!(wire != "udp", "wire_shape shapes tcp scenarios only");
        let pipe_bin = std::env::current_exe()
            .expect("own path")
            .parent()
            .expect("bin dir")
            .join("shaped_pipe");
        let pipe = spawn_role(Command::new(pipe_bin), manifest, "pipe", &addr, args);
        let pipe_ready = await_line(&pipe, "READY", Duration::from_secs(10));
        let pipe_addr = pipe_ready
            .split_whitespace()
            .find_map(|kv| kv.strip_prefix("addr="))
            .expect("pipe READY carries addr")
            .to_string();
        (Some(pipe), pipe_addr)
    } else {
        (None, addr)
    };

    let initiator = spawn_role(
        interop_command(&initiator_impl),
        manifest,
        "initiator",
        &addr,
        args,
    );
    let scenario_duration_ms = args
        .duration_ms
        .or_else(|| manifest_json["profile"]["duration_ms"].as_u64())
        .unwrap_or(10_000);
    let window = Duration::from_millis(scenario_duration_ms + 30_000);
    let result = await_line(&initiator, "RESULT", window);
    let responder_result = await_line(&responder, "RESULT", Duration::from_secs(10));
    let wire_line = pipe
        .as_ref()
        .map(|p| await_line(p, "WIRE", Duration::from_secs(15)));

    let energy = bracket.map(|b| b.finish());

    let initiator_metrics = initiator.finalize();
    let responder_metrics = responder.finalize();

    file_results(
        args,
        version,
        &pairing_slug,
        &pairing_label,
        CollectedRun {
            result: &result,
            responder_result: &responder_result,
            wire_line: wire_line.as_deref(),
            energy,
            idle_watts,
            initiator: initiator_metrics,
            responder: responder_metrics,
            relay: None,
        },
    );
}

struct CollectedRun<'a> {
    result: &'a str,
    responder_result: &'a str,
    wire_line: Option<&'a str>,
    energy: Option<(f64, f64)>,
    idle_watts: Option<f64>,
    initiator: RoleMetrics,
    responder: RoleMetrics,
    relay: Option<RoleMetrics>,
}

fn file_results(
    args: &Args,
    version: u32,
    pairing_slug: &str,
    pairing_label: &str,
    run: CollectedRun<'_>,
) {
    let result = run.result;
    let responder_result = run.responder_result;
    let wire_line = run.wire_line;
    let energy = run.energy;
    let idle_watts = run.idle_watts;
    let initiator_cpu = run.initiator.cpu_seconds;
    let initiator_rss = run.initiator.peak_rss_bytes;
    let responder_cpu = run.responder.cpu_seconds;
    let responder_rss = run.responder.peak_rss_bytes;
    let relay = run.relay;

    // The scenarios speak per-mechanism vocabularies for the same three
    // facts: what went out (sent/cycles), what the initiator saw settle
    // (delivered/settled/cycles), and what the responder counted
    // (delivered/received/served).
    let sent = field(result, "sent")
        .or_else(|| field(result, "cycles"))
        .unwrap_or(0.0);
    let delivered = field(result, "delivered")
        .or_else(|| field(result, "settled"))
        .or_else(|| field(result, "cycles"))
        .unwrap_or(0.0);
    let timeouts = field(result, "timeouts")
        .or_else(|| field(result, "failures"))
        .unwrap_or(f64::NAN);
    let raced = field(result, "raced").unwrap_or(0.0);
    let responder_delivered = field(responder_result, "delivered")
        .or_else(|| field(responder_result, "received"))
        .or_else(|| field(responder_result, "served"))
        .unwrap_or(0.0);
    let died = field(result, "died").unwrap_or(0.0) > 0.0;
    if died {
        eprintln!(
            "verdict: the initiator declared the responder DEAD mid-run — conformance filed, \
             throughput/latency/energy withheld (a dead run's last gasp is not a measurement)"
        );
    }
    // A debug-built participant runs its crypto ~10x slower, so its throughput, latency, and memory
    // are not measurements — only its conformance counts survive. Either side being debug taints the
    // initiator's measured RTT (it waits on the slow side's proofs), so both lines are checked.
    let perf_valid = !result.contains("build=debug") && !responder_result.contains("build=debug");
    if !perf_valid {
        eprintln!(
            "verdict: a participant is a DEBUG build (build=debug) — crypto ~10x slower; \
             conformance filed, throughput/latency/memory/energy withheld (debug perf is not a \
             measurement; rebuild --release)"
        );
    }
    assert!(
        delivered <= sent,
        "delivery accounting holds (initiator-proven <= sent): {delivered} <= {sent}",
    );
    if delivered > responder_delivered {
        // Hash-proved mechanisms settle on the receiver's own proof, so the
        // initiator's count is the strong one; the reference's responder can
        // under-count by a conclusion callback racing its quiet-exit print.
        eprintln!(
            "conformance note: responder counted {responder_delivered} of {delivered} \
             proven deliveries — known reference conclusion-callback exit race",
        );
    } else {
        assert!(
            responder_delivered <= sent,
            "delivery accounting holds (responder-seen <= sent): {responder_delivered} <= {sent}",
        );
    }

    let stamp = run_stamp(args.pin);
    let row = |axis: Axis, metric: &str, value: Option<f64>, unit: &str| ResultRow {
        scenario: args.scenario.clone(),
        scenario_version: version,
        implementation: pairing_label.to_string(),
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
    let elapsed_seconds = field(result, "elapsed_ms")
        .map(|ms| ms / 1_000.0)
        .filter(|seconds| *seconds > 0.0);
    let delivered_per_sec = field(result, "delivered_per_sec")
        .or_else(|| field(result, "requests_per_sec"))
        .or_else(|| field(result, "cycles_per_sec"))
        .or_else(|| elapsed_seconds.map(|seconds| delivered / seconds));
    let rtt_p50_ms = field(result, "rtt_p50_ms").or_else(|| field(result, "transfer_p50_ms"));
    let rtt_p99_ms = field(result, "rtt_p99_ms").or_else(|| field(result, "transfer_p99_ms"));

    let mut rows = vec![
        row(
            Axis::Conformance,
            "settled_clean",
            Some(f64::from(sent == delivered + timeouts + raced)),
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
        row(Axis::Conformance, "raced", Some(raced), "msgs"),
        row(
            Axis::Throughput,
            "delivered_per_sec",
            delivered_per_sec.filter(|_| !died && perf_valid),
            "msgs/s",
        ),
        row(
            Axis::Throughput,
            "goodput_bytes_per_sec",
            field(result, "goodput_bytes_per_sec").filter(|_| !died && perf_valid),
            "B/s",
        ),
        row(
            Axis::Latency,
            "rtt_p50_ms",
            rtt_p50_ms.filter(|_| !died && perf_valid),
            "ms",
        ),
        row(
            Axis::Latency,
            "rtt_p99_ms",
            rtt_p99_ms.filter(|_| !died && perf_valid),
            "ms",
        ),
        row(
            Axis::Memory,
            "initiator_peak_rss_bytes",
            Some(initiator_rss as f64).filter(|_| perf_valid),
            "bytes",
        ),
        row(
            Axis::Memory,
            "responder_peak_rss_bytes",
            Some(responder_rss as f64).filter(|_| perf_valid),
            "bytes",
        ),
        // Per-role CPU seconds, sampled from outside each process — the raw signal that lets a
        // package-domain energy figure (which the meter can only read for the whole SoC, both
        // roles at once) be apportioned to sender vs receiver by their CPU-time share.
        row(Axis::Energy, "initiator_cpu_seconds", Some(initiator_cpu), "s"),
        row(Axis::Energy, "responder_cpu_seconds", Some(responder_cpu), "s"),
    ];
    if let Some(relay) = &relay {
        rows.push(row(
            Axis::Memory,
            "relay_peak_rss_bytes",
            Some(relay.peak_rss_bytes as f64).filter(|_| perf_valid),
            "bytes",
        ));
        rows.push(row(Axis::Energy, "relay_cpu_seconds", Some(relay.cpu_seconds), "s"));
    }
    if let Some(after_reconnect) = field(result, "delivered_after_reconnect") {
        rows.push(row(
            Axis::Conformance,
            "route_survived",
            Some(f64::from(after_reconnect > 0.0)),
            "bool",
        ));
    }
    if let (Some((raw_joules, wall_seconds)), Some(idle_watts)) = (energy, idle_watts) {
        let net_joules = raw_joules - idle_watts * wall_seconds;
        let measurable = net_joules > 0.0;
        let per_delivered_mj = (measurable && delivered > 0.0 && !died && perf_valid)
            .then(|| net_joules * 1_000.0 / delivered);
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
        rows.push(row(
            Axis::Energy,
            "net_joules",
            measurable.then_some(net_joules),
            "J",
        ));
        rows.push(row(
            Axis::Energy,
            "net_millijoules_per_delivered",
            per_delivered_mj,
            "mJ/msg",
        ));
        // Apportion the package energy to each role by its CPU-time share. The meter is
        // package-domain (and on Linux RAPL there is no per-process counter at all), so
        // CPU-share is the honest cross-platform proxy for splitting a pairing's cost into
        // sender and receiver — exact only insofar as power tracks CPU time, which a sign-heavy
        // initiator vs a verify-heavy responder bends, so read it as attribution, not ground
        // truth. The combined figure above remains the measured one.
        let total_cpu = initiator_cpu + responder_cpu;
        let initiator_share = if total_cpu > 0.0 {
            initiator_cpu / total_cpu
        } else {
            0.5
        };
        rows.push(row(
            Axis::Energy,
            "initiator_net_millijoules_per_delivered",
            per_delivered_mj.map(|mj| mj * initiator_share),
            "mJ/msg",
        ));
        rows.push(row(
            Axis::Energy,
            "responder_net_millijoules_per_delivered",
            per_delivered_mj.map(|mj| mj * (1.0 - initiator_share)),
            "mJ/msg",
        ));
        if measurable {
            let combined = per_delivered_mj.unwrap_or(f64::NAN);
            println!(
                "\nSUMMARY energy raw={raw_joules:.1}J over {wall_seconds:.1}s \
                 (idle {idle_watts:.2}W) | net={net_joules:.1}J | {combined:.2} mJ/msg \
                 (initiator {:.2} / responder {:.2}, by cpu {:.0}%/{:.0}%)",
                combined * initiator_share,
                combined * (1.0 - initiator_share),
                initiator_share * 100.0,
                (1.0 - initiator_share) * 100.0,
            );
        } else {
            println!(
                "\nSUMMARY energy raw={raw_joules:.1}J over {wall_seconds:.1}s ran BELOW the \
                 idle baseline ({idle_watts:.2}W) — net energy unmeasurable this run \
                 (baseline drift), filed as pending",
            );
        }
    }
    if let Some(wire_line) = &wire_line {
        let wire_total = field(wire_line, "a_to_b_bytes").unwrap_or(0.0)
            + field(wire_line, "b_to_a_bytes").unwrap_or(0.0);
        let payload = field(result, "payload_bytes").or_else(|| {
            match (
                field(result, "request_bytes"),
                field(result, "response_bytes"),
            ) {
                (Some(requests), Some(responses)) => Some(requests + responses),
                _ => None,
            }
        });
        let efficiency = payload
            .filter(|_| wire_total > 0.0 && !died)
            .map(|p| p / wire_total);
        rows.push(row(
            Axis::Throughput,
            "wire_bytes_total",
            Some(wire_total),
            "bytes",
        ));
        rows.push(row(
            Axis::Throughput,
            "payload_per_wire_byte",
            efficiency,
            "ratio",
        ));
        println!(
            "\nSUMMARY wire bytes={wire_total:.0} | payload/wire={}",
            efficiency
                .map(|e| format!("{e:.3}"))
                .unwrap_or_else(|| "unmeasured".into()),
        );
    }
    println!(
        "\nSUMMARY scenario={} pairing={pairing_label} host={}\n\
         SUMMARY conformance sent={sent:.0} delivered={delivered:.0} \
         responder_seen={responder_delivered:.0} timed_out={timeouts:.0} raced={raced:.0} settled_clean={}\n\
         SUMMARY initiator cpu={initiator_cpu:.2}s peak_rss={:.1}MiB | \
         responder cpu={responder_cpu:.2}s peak_rss={:.1}MiB",
        args.scenario,
        stamp.host,
        sent == delivered + timeouts + raced,
        initiator_rss as f64 / (1024.0 * 1024.0),
        responder_rss as f64 / (1024.0 * 1024.0),
    );
    if let Some(relay) = &relay {
        println!(
            "SUMMARY relay cpu={:.2}s peak_rss={:.1}MiB",
            relay.cpu_seconds,
            relay.peak_rss_bytes as f64 / (1024.0 * 1024.0),
        );
    }
    if let Some(after_reconnect) = field(result, "delivered_after_reconnect") {
        println!(
            "SUMMARY tunnel route_survived={} delivered_after_reconnect={after_reconnect:.0}",
            after_reconnect > 0.0,
        );
    }
    if args.pin {
        write_rows(&stamp.host, &args.scenario, pairing_slug, &rows);
        println!(
            "SUMMARY rows filed under results/{}/{}/{pairing_slug}.jsonl",
            stamp.host, args.scenario,
        );
    } else {
        println!("UNPINNED run: rows printed, not filed (re-run without --unpinned to file)");
    }
}

fn run_relay_interop(args: &Args, manifest_json: &serde_json::Value, manifest: &Path) {
    let version = manifest_json["version"].as_u64().unwrap_or(1) as u32;
    let initiator_impl = implementation(&args.initiator);
    let responder_impl = implementation(&args.responder);
    let relay_impl = implementation(&args.relay);
    let mechanism = manifest_json["profile"]["mechanism"].as_str().unwrap_or("single");
    if let Some(reason) = unsupported_pairing(&initiator_impl, &responder_impl, mechanism) {
        println!(
            "SKIP scenario={} relay={} initiator={} responder={} reason={reason}",
            args.scenario, relay_impl.label, initiator_impl.label, responder_impl.label,
        );
        return;
    }

    let endpoints_are_self = args.initiator == "self" && args.responder == "self";
    let pairing_slug = if endpoints_are_self {
        relay_impl.slug.to_string()
    } else {
        format!(
            "{}--{}--{}",
            relay_impl.slug, initiator_impl.slug, responder_impl.slug
        )
    };
    let pairing_label = if endpoints_are_self {
        format!("{} (relay)", relay_impl.label)
    } else {
        format!(
            "{} (relay) {}/{}",
            relay_impl.label, initiator_impl.label, responder_impl.label
        )
    };

    let node = |subject: &Implementation| {
        subject
            .interop_command()
            .unwrap_or_else(|| panic!("implementation {:?} fields no interop node", subject.name))
    };

    let meter = PowerMeter::detect();
    if meter.is_none() {
        println!("{}", energy_unavailable_hint());
    }
    let idle_watts = meter
        .as_ref()
        .map(|m| m.idle_watts(Duration::from_millis(1500)));
    let bracket = meter.as_ref().map(|m| m.start());

    let mut relay = spawn_role(node(&relay_impl), manifest, "relay", "127.0.0.1:0", args);
    let relay_ready = await_line(&relay, "READY", Duration::from_secs(10));
    let endpoints = relay_ready
        .split_whitespace()
        .find_map(|kv| kv.strip_prefix("addr="))
        .expect("relay READY carries addr=<side_a>><side_b>");
    let (addr_a, addr_b) = endpoints
        .split_once('>')
        .expect("relay READY addr is <side_a>><side_b>");

    let responder = spawn_role(node(&responder_impl), manifest, "responder", addr_a, args);
    await_line(&responder, "READY", Duration::from_secs(10));
    let initiator = spawn_role(node(&initiator_impl), manifest, "initiator", addr_b, args);

    let scenario_duration_ms = args
        .duration_ms
        .or_else(|| manifest_json["profile"]["duration_ms"].as_u64())
        .unwrap_or(10_000);
    let window = Duration::from_millis(scenario_duration_ms + 30_000);
    let result = await_line(&initiator, "RESULT", window);
    let responder_result = await_line(&responder, "RESULT", Duration::from_secs(10));

    let energy = bracket.map(|b| b.finish());
    let initiator_metrics = initiator.finalize();
    let responder_metrics = responder.finalize();
    let _ = relay.child.kill();
    let relay_metrics = relay.finalize();

    file_results(
        args,
        version,
        &pairing_slug,
        &pairing_label,
        CollectedRun {
            result: &result,
            responder_result: &responder_result,
            wire_line: None,
            energy,
            idle_watts,
            initiator: initiator_metrics,
            responder: responder_metrics,
            relay: Some(relay_metrics),
        },
    );
}
