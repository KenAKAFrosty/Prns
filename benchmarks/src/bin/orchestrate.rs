//! The impl-neutral orchestrator for live scenarios: spawn one process per role, point
//! them at each other over localhost TCP, collect the line protocol, and file result
//! rows into the substrate. A *run* is an assignment of implementations to the
//! scenario's roles — `self/self` is both the ceiling measurement and the harness
//! ceiling itself; other pairings join by naming a different participation binary for a
//! role. CPU and peak RSS are sampled from `/proc` *outside* the contestants, so a
//! participation binary can't flatter itself.
//!
//! usage: orchestrate [scenario] [--duration-ms N] [--pin]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc as std_mpsc;
use std::time::Duration;

use benchmarks::scenario_dir;
use benchmarks::{write_rows, Axis, ResultRow};

const RESPONDER_CORES: &str = "0,1";
const INITIATOR_CORES: &str = "2,3";

struct RoleProcess {
    child: Child,
    lines: std_mpsc::Receiver<String>,
    cpu_seconds: std::sync::Arc<std::sync::Mutex<f64>>,
    peak_rss_bytes: std::sync::Arc<std::sync::Mutex<u64>>,
}

fn node_binary() -> std::path::PathBuf {
    let mut path = std::env::current_exe().expect("own path");
    path.set_file_name("scenario_node");
    path
}

fn spawn_role(manifest: &std::path::Path, role: &str, addr: &str, args: &Args) -> RoleProcess {
    let mut command = if args.pin {
        let mut c = Command::new("taskset");
        c.arg("-c").arg(if role == "responder" {
            RESPONDER_CORES
        } else {
            INITIATOR_CORES
        });
        c.arg(node_binary());
        c
    } else {
        Command::new(node_binary())
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

    let pid = child.id();
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

    RoleProcess {
        child,
        lines,
        cpu_seconds,
        peak_rss_bytes,
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
    duration_ms: Option<u64>,
    pin: bool,
}

fn parse_args() -> Args {
    let mut args = Args {
        scenario: "link-firehose".into(),
        duration_ms: None,
        pin: false,
    };
    let mut argv = std::env::args().skip(1);
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--duration-ms" => {
                args.duration_ms = Some(argv.next().and_then(|v| v.parse().ok()).expect("ms"));
            }
            "--pin" => args.pin = true,
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

fn main() {
    let args = parse_args();
    let manifest = scenario_dir(&args.scenario).join("manifest.json");
    assert!(manifest.exists(), "no manifest at {}", manifest.display());

    let mut responder = spawn_role(&manifest, "responder", "127.0.0.1:0", &args);
    let ready = await_line(&responder, "READY", Duration::from_secs(10));
    let addr = ready
        .split_whitespace()
        .find_map(|kv| kv.strip_prefix("addr="))
        .expect("responder READY carries addr")
        .to_string();

    let mut initiator = spawn_role(&manifest, "initiator", &addr, &args);
    let window = Duration::from_millis(args.duration_ms.unwrap_or(10_000) + 30_000);
    let result = await_line(&initiator, "RESULT", window);
    let responder_result = await_line(&responder, "RESULT", Duration::from_secs(10));

    let initiator_cpu = *initiator.cpu_seconds.lock().expect("cpu");
    let initiator_rss = *initiator.peak_rss_bytes.lock().expect("rss");
    let responder_cpu = *responder.cpu_seconds.lock().expect("cpu");
    let responder_rss = *responder.peak_rss_bytes.lock().expect("rss");
    let _ = initiator.child.wait();
    let _ = responder.child.wait();

    let sent = field(&result, "sent").unwrap_or(0.0);
    let delivered = field(&result, "delivered").unwrap_or(0.0);
    let timeouts = field(&result, "timeouts").unwrap_or(f64::NAN);
    let responder_delivered = field(&responder_result, "delivered").unwrap_or(0.0);
    assert_eq!(
        delivered, responder_delivered,
        "both ends agree on what was delivered — the conformance gate",
    );

    let host = command_line("rustc", &["-vV"])
        .and_then(|v| {
            v.lines()
                .find_map(|l| l.strip_prefix("host: ").map(str::to_string))
        })
        .unwrap_or_else(|| "unknown-host".into());
    let commit = command_line("git", &["rev-parse", "--short", "HEAD"]).unwrap_or_default();
    let toolchain = command_line("rustc", &["--version"]).unwrap_or_default();

    let row = |axis: Axis, metric: &str, value: Option<f64>, unit: &str| ResultRow {
        scenario: args.scenario.clone(),
        scenario_version: 1,
        implementation: "Prns (self/self)".into(),
        commit: commit.clone(),
        toolchain: toolchain.clone(),
        host: host.clone(),
        axis,
        metric: metric.into(),
        value,
        unit: unit.into(),
    };
    let rows = vec![
        row(
            Axis::Conformance,
            "settled_clean",
            Some(f64::from(sent == delivered && timeouts == 0.0)),
            "bool",
        ),
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
    write_rows(&host, &args.scenario, "personal-rns.self-self", &rows);

    println!(
        "\nSUMMARY scenario={} pairing=self/self host={host}\n\
         SUMMARY initiator cpu={initiator_cpu:.2}s peak_rss={:.1}MiB | \
         responder cpu={responder_cpu:.2}s peak_rss={:.1}MiB\n\
         SUMMARY rows filed under results/{host}/{}/personal-rns.self-self.jsonl",
        args.scenario,
        initiator_rss as f64 / (1024.0 * 1024.0),
        responder_rss as f64 / (1024.0 * 1024.0),
        args.scenario,
    );
}
