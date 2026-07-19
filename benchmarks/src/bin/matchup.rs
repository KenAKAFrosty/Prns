//! The Matchup runner: the shared-instance conformance grid. A `Matchup` names who sends, who
//! receives, which scenario, and the topology they meet in. The runner stands up the host, attaches
//! the two app clients over its bus, drives the scenario through the existing `READY`/`RESULT` line
//! protocol, and reports the conformance verdict. Our impl stays outside the measured boundary: the
//! daemons do the work, the runner is only the uniform load-and-measure harness.
//!
//! Loopback is the conformance topology: one host, two clients dialing in, traffic looped through the
//! host's engine. An impl that can't currently field a role (Prns as host or client both ride the
//! `local` feature, mid-reconstruction) yields a typed `Unavailable` reason and the cell is skipped
//! aloud, never silently dropped.
//!
//! usage: matchup [--scenario NAME] [--host IMPL] [--sender IMPL] [--receiver IMPL] [--duration-ms N]
//! where IMPL is `reference` or `prns`.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use benchmarks::{
    load_host, load_or_create_submitter_id, write_rows, Axis, DeviceId, ResultRow, SubmitterId,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum RnsImplementation {
    Prns,
    Reference,
}

impl RnsImplementation {
    fn label(self) -> &'static str {
        match self {
            RnsImplementation::Prns => "prns",
            RnsImplementation::Reference => "reference",
        }
    }
}

#[derive(Clone, Copy)]
enum Role {
    Sender,
    Receiver,
}

impl Role {
    fn protocol_role(self) -> &'static str {
        match self {
            Role::Sender => "initiator",
            Role::Receiver => "responder",
        }
    }
}

struct Unavailable(&'static str);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scenario {
    SingleFirehose,
    LinkFirehoseSmallPayload,
    ResourceSmall,
    ResourceTransfer,
    RequestResponse,
    EstablishmentChurn,
    ConcurrentLinks,
    ResourceFanout,
}

impl Scenario {
    fn dir_name(self) -> &'static str {
        match self {
            Scenario::SingleFirehose => "single-firehose",
            Scenario::LinkFirehoseSmallPayload => "link-firehose-small-payload",
            Scenario::ResourceSmall => "resource-small",
            Scenario::ResourceTransfer => "resource-transfer",
            Scenario::RequestResponse => "request-response",
            Scenario::EstablishmentChurn => "establishment-churn",
            Scenario::ConcurrentLinks => "concurrent-links",
            Scenario::ResourceFanout => "resource-fanout",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        SCENARIOS
            .iter()
            .copied()
            .find(|scenario| scenario.dir_name() == raw)
    }

    fn manifest(self) -> PathBuf {
        benchmarks::scenario_dir(self.dir_name()).join("manifest.json")
    }

    fn rides_high_level_runtime(self) -> bool {
        matches!(
            self,
            Scenario::SingleFirehose
                | Scenario::LinkFirehoseSmallPayload
                | Scenario::RequestResponse
                | Scenario::EstablishmentChurn
                | Scenario::ResourceSmall
                | Scenario::ResourceTransfer
                | Scenario::ConcurrentLinks
                | Scenario::ResourceFanout
        )
    }
}

struct Matchup {
    sender: RnsImplementation,
    receiver: RnsImplementation,
    scenario: Scenario,
    topology: Topology,
}

enum Topology {
    Loopback { host: RnsImplementation },
}

struct Bus {
    port: u16,
    control_port: u16,
    rpc_key: String,
}

impl Bus {
    fn allocate() -> Bus {
        let port = free_port();
        Bus {
            port,
            control_port: port + 1,
            rpc_key: "5a".repeat(32),
        }
    }
}

fn reference_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("reference")
}

fn reference_bin(name: &str) -> PathBuf {
    reference_dir().join(".venv").join("bin").join(name)
}

fn reference_python() -> PathBuf {
    let venv = reference_bin("python");
    if venv.exists() {
        venv
    } else {
        PathBuf::from("python3")
    }
}

fn sibling_binary(name: &str) -> PathBuf {
    let mut path = std::env::current_exe().expect("own path");
    path.set_file_name(name);
    path
}

impl RnsImplementation {
    fn host_command(self, bus: &Bus, dir: &Path) -> Result<Command, Unavailable> {
        match self {
            RnsImplementation::Reference => {
                let config = format!(
                    "[reticulum]\n  enable_transport = Yes\n  share_instance = Yes\n  \
                     shared_instance_type = tcp\n  shared_instance_port = {}\n  \
                     instance_control_port = {}\n  rpc_key = {}\n  panic_on_interface_error = No\n\n\
                     [logging]\n  loglevel = 1\n",
                    bus.port, bus.control_port, bus.rpc_key
                );
                std::fs::write(dir.join("config"), config).expect("write host config");
                let mut command = Command::new(reference_bin("rnsd"));
                command.arg("--config").arg(dir);
                Ok(command)
            }
            RnsImplementation::Prns => {
                let mut command = Command::new(sibling_binary("matchup_host"));
                command.env("MATCHUP_LOCAL_PORT", bus.port.to_string());
                Ok(command)
            }
        }
    }

    fn client_command(
        self,
        scenario: Scenario,
        role: Role,
        duration_ms: u64,
        bus: &Bus,
    ) -> Result<Command, Unavailable> {
        let manifest = scenario.manifest();
        match self {
            RnsImplementation::Reference => {
                let mut command = Command::new(reference_python());
                command
                    .arg("-u")
                    .arg(reference_dir().join("scenario_node.py"))
                    .arg(&manifest)
                    .arg(role.protocol_role())
                    .arg("shared")
                    .arg(duration_ms.to_string())
                    .env("RNS_BENCH_SHARED_PORT", bus.port.to_string())
                    .env("RNS_BENCH_SHARED_RPC_KEY", &bus.rpc_key);
                Ok(command)
            }
            RnsImplementation::Prns => {
                if !scenario.rides_high_level_runtime() {
                    return Err(Unavailable(
                        "prns client: resource needs a responder-side resource strategy, not yet a recipe knob",
                    ));
                }
                let mut command = Command::new(sibling_binary("scenario_node"));
                command
                    .arg(&manifest)
                    .arg(role.protocol_role())
                    .arg("shared")
                    .arg(duration_ms.to_string())
                    .env("MATCHUP_SHARED_PORT", bus.port.to_string());
                Ok(command)
            }
        }
    }
}

struct Proc {
    child: Child,
    lines: mpsc::Receiver<String>,
}

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn(mut command: Command, tag: &str) -> Proc {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|e| panic!("spawn {tag}: {e}"));
    let stdout = child.stdout.take().expect("piped stdout");
    let (line_tx, lines) = mpsc::channel();
    let label = tag.to_string();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            eprintln!("[{label}] {line}");
            let _ = line_tx.send(line);
        }
    });
    Proc { child, lines }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral")
        .local_addr()
        .expect("local addr")
        .port()
}

fn wait_for_bus(bus: &Bus, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if std::net::TcpStream::connect(("127.0.0.1", bus.port)).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    false
}

fn await_line(proc: &Proc, prefix: &str, within: Duration) -> Option<String> {
    let deadline = Instant::now() + within;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        match proc.lines.recv_timeout(left) {
            Ok(line) if line.starts_with(prefix) => return Some(line),
            Ok(_) => {}
            Err(_) => return None,
        }
    }
}

fn field(line: &str, key: &str) -> Option<f64> {
    line.split_whitespace()
        .find_map(|kv| kv.strip_prefix(&format!("{key}=")))
        .and_then(|v| v.parse().ok())
}

fn command_line(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn rustc_host_triple() -> String {
    command_line("rustc", &["-vV"])
        .and_then(|version| {
            version
                .lines()
                .find_map(|line| line.strip_prefix("host: ").map(str::to_string))
        })
        .unwrap_or_else(|| "unknown-host".into())
}

struct RunStamp {
    host: String,
    commit: String,
    toolchain: String,
    device_id: Option<DeviceId>,
    submitter_id: Option<SubmitterId>,
}

fn run_stamp() -> RunStamp {
    let host = rustc_host_triple();
    RunStamp {
        device_id: load_host(&host).and_then(|descriptor| descriptor.device_id),
        submitter_id: Some(load_or_create_submitter_id()),
        commit: command_line("git", &["rev-parse", "--short", "HEAD"]).unwrap_or_default(),
        toolchain: command_line("rustc", &["--version"]).unwrap_or_default(),
        host,
    }
}

fn scenario_version(scenario: Scenario) -> u32 {
    std::fs::read_to_string(scenario.manifest())
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|manifest| manifest.get("version").and_then(serde_json::Value::as_u64))
        .unwrap_or(0) as u32
}

fn emit_rows(
    stamp: &RunStamp,
    scenario: Scenario,
    host: RnsImplementation,
    client: RnsImplementation,
    sent: f64,
    delivered: f64,
    clean: bool,
) {
    let implementation = format!("{}-host/{}-client", host.label(), client.label());
    let impl_slug = format!("matchup-{}-host-{}-client", host.label(), client.label());
    let version = scenario_version(scenario);
    let row = |metric: &str, value: f64, unit: &str| ResultRow {
        scenario: scenario.dir_name().to_string(),
        scenario_version: version,
        implementation: implementation.clone(),
        commit: stamp.commit.clone(),
        toolchain: stamp.toolchain.clone(),
        host: stamp.host.clone(),
        axis: Axis::Conformance,
        metric: metric.to_string(),
        value: Some(value),
        unit: unit.to_string(),
        device_id: stamp.device_id,
        submitter_id: stamp.submitter_id,
    };
    let rows = [
        row("settled_clean", if clean { 1.0 } else { 0.0 }, "bool"),
        row("sent", sent, "msgs"),
        row("delivered", delivered, "msgs"),
    ];
    write_rows(&stamp.host, scenario.dir_name(), &impl_slug, &rows);
}

enum Verdict {
    Ran {
        sent: f64,
        delivered: f64,
        timeouts: f64,
        responder_delivered: f64,
        clean: bool,
    },
    Skipped(String),
    Failed(String),
}

fn run_loopback(matchup: &Matchup, duration_ms: u64) -> Verdict {
    let Topology::Loopback { host } = matchup.topology;
    let manifest = matchup.scenario.manifest();
    if !manifest.exists() {
        return Verdict::Failed(format!("no manifest at {}", manifest.display()));
    }
    let bus = Bus::allocate();
    let host_dir = std::env::temp_dir().join(format!("matchup-host-{}", bus.port));
    std::fs::create_dir_all(&host_dir).expect("host config dir");

    let host_command = match host.host_command(&bus, &host_dir) {
        Ok(command) => command,
        Err(Unavailable(reason)) => {
            return Verdict::Skipped(format!("host {}: {reason}", host.label()))
        }
    };
    let responder_command =
        match matchup
            .receiver
            .client_command(matchup.scenario, Role::Receiver, duration_ms, &bus)
        {
            Ok(command) => command,
            Err(Unavailable(reason)) => {
                return Verdict::Skipped(format!("receiver {}: {reason}", matchup.receiver.label()))
            }
        };
    let initiator_command =
        match matchup
            .sender
            .client_command(matchup.scenario, Role::Sender, duration_ms, &bus)
        {
            Ok(command) => command,
            Err(Unavailable(reason)) => {
                return Verdict::Skipped(format!("sender {}: {reason}", matchup.sender.label()))
            }
        };

    let _host = spawn(host_command, "host");
    if !wait_for_bus(&bus, Duration::from_secs(20)) {
        return Verdict::Failed("host never bound the shared-instance bus".into());
    }

    let responder = spawn(responder_command, "responder");
    if await_line(&responder, "READY", Duration::from_secs(15)).is_none() {
        return Verdict::Failed("responder never became READY".into());
    }

    let initiator = spawn(initiator_command, "initiator");
    let initiator_window = Duration::from_millis(duration_ms) + Duration::from_secs(20);
    let Some(initiator_result) = await_line(&initiator, "RESULT", initiator_window) else {
        return Verdict::Failed("initiator produced no RESULT".into());
    };
    let Some(responder_result) = await_line(&responder, "RESULT", Duration::from_secs(15)) else {
        return Verdict::Failed("responder produced no RESULT".into());
    };

    let _ = std::fs::remove_dir_all(&host_dir);

    let sent = field(&initiator_result, "sent")
        .or_else(|| field(&initiator_result, "cycles"))
        .unwrap_or(0.0);
    let delivered = field(&initiator_result, "delivered")
        .or_else(|| field(&initiator_result, "settled"))
        .or_else(|| field(&initiator_result, "cycles"))
        .unwrap_or(0.0);
    let timeouts = field(&initiator_result, "timeouts")
        .or_else(|| field(&initiator_result, "failures"))
        .unwrap_or(f64::NAN);
    let raced = field(&initiator_result, "raced").unwrap_or(0.0);
    let responder_delivered = field(&responder_result, "delivered")
        .or_else(|| field(&responder_result, "received"))
        .or_else(|| field(&responder_result, "served"))
        .unwrap_or(0.0);
    Verdict::Ran {
        sent,
        delivered,
        timeouts,
        responder_delivered,
        clean: sent == delivered + timeouts + raced && delivered > 0.0,
    }
}

const HOSTS: &[RnsImplementation] = &[RnsImplementation::Reference, RnsImplementation::Prns];
const CLIENTS: &[RnsImplementation] = &[RnsImplementation::Reference, RnsImplementation::Prns];
const SCENARIOS: &[Scenario] = &[
    Scenario::SingleFirehose,
    Scenario::LinkFirehoseSmallPayload,
    Scenario::ResourceSmall,
    Scenario::ResourceTransfer,
    Scenario::RequestResponse,
    Scenario::EstablishmentChurn,
    Scenario::ConcurrentLinks,
    Scenario::ResourceFanout,
];

struct GridFilter {
    scenario: Option<Scenario>,
    duration_ms: u64,
    write: bool,
}

fn parse_filter() -> GridFilter {
    let mut filter = GridFilter {
        scenario: None,
        duration_ms: 3000,
        write: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--write" => filter.write = true,
            "--scenario" => {
                let value = args
                    .next()
                    .unwrap_or_else(|| panic!("--scenario needs a value"));
                filter.scenario = Some(
                    Scenario::parse(&value).unwrap_or_else(|| panic!("unknown scenario {value}")),
                );
            }
            "--duration-ms" => {
                let value = args
                    .next()
                    .unwrap_or_else(|| panic!("--duration-ms needs a value"));
                filter.duration_ms = value.parse().expect("duration-ms is a number");
            }
            other => panic!("unknown flag {other}"),
        }
    }
    filter
}

fn summarize(verdict: &Verdict) -> (&'static str, String) {
    match verdict {
        Verdict::Ran {
            sent,
            delivered,
            timeouts,
            responder_delivered,
            clean,
        } => (
            if *clean { "PASS" } else { "FAIL" },
            format!("sent={sent} delivered={delivered} timeouts={timeouts} (receiver saw {responder_delivered})"),
        ),
        Verdict::Skipped(reason) => ("SKIP", reason.clone()),
        Verdict::Failed(reason) => ("FAIL", reason.clone()),
    }
}

fn main() {
    let filter = parse_filter();
    let scenarios: Vec<Scenario> = match filter.scenario {
        Some(scenario) => vec![scenario],
        None => SCENARIOS.to_vec(),
    };

    eprintln!(
        "Matchup grid (loopback): {} scenario(s) x {} host(s) x {} client(s)",
        scenarios.len(),
        HOSTS.len(),
        CLIENTS.len()
    );

    let stamp = filter.write.then(run_stamp);
    let mut rows = Vec::new();
    let (mut pass, mut skip, mut fail) = (0u32, 0u32, 0u32);
    for scenario in &scenarios {
        for host in HOSTS {
            for client in CLIENTS {
                let matchup = Matchup {
                    sender: *client,
                    receiver: *client,
                    scenario: *scenario,
                    topology: Topology::Loopback { host: *host },
                };
                eprintln!(
                    "--- {} | host={} client={} ---",
                    scenario.dir_name(),
                    host.label(),
                    client.label()
                );
                let verdict = run_loopback(&matchup, filter.duration_ms);
                match &verdict {
                    Verdict::Ran { clean: true, .. } => pass += 1,
                    Verdict::Skipped(_) => skip += 1,
                    _ => fail += 1,
                }
                if let (
                    Some(stamp),
                    Verdict::Ran {
                        sent,
                        delivered,
                        clean,
                        ..
                    },
                ) = (&stamp, &verdict)
                {
                    emit_rows(stamp, *scenario, *host, *client, *sent, *delivered, *clean);
                }
                let (status, detail) = summarize(&verdict);
                rows.push((
                    scenario.dir_name(),
                    host.label(),
                    client.label(),
                    status,
                    detail,
                ));
            }
        }
    }

    println!();
    println!(
        "{:<34} {:<11} {:<11} {:<6} detail",
        "scenario", "host", "client", ""
    );
    for (scenario, host, client, status, detail) in &rows {
        println!("{scenario:<34} {host:<11} {client:<11} {status:<6} {detail}");
    }
    println!();
    println!(
        "{} cells: {pass} pass, {skip} skip, {fail} fail",
        rows.len()
    );
    if let Some(stamp) = &stamp {
        println!(
            "wrote conformance rows under results/{}/<scenario>/",
            stamp.host
        );
    }
    std::process::exit(if fail > 0 { 1 } else { 0 });
}
