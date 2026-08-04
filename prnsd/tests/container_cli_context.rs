#![cfg(unix)]

use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use prnsd_control::{running, ServiceKind, ServicePaths};

static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let sequence = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "prnsd-container-context-{label}-{}-{nanos}-{sequence}",
            std::process::id(),
        ));
        fs::create_dir_all(&path).unwrap_or_else(|error| panic!("{error}"));
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct RunningService {
    child: Child,
    lines: Receiver<String>,
    reader: Option<JoinHandle<()>>,
    captured: Vec<String>,
}

impl RunningService {
    fn start(config: &TestDirectory, state: &TestDirectory, home: &TestDirectory) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_prnsd"))
            .args(["run", "--service", "--config"])
            .arg(config.path())
            .args(["--log-format", "json", "--persistence-policy", "required"])
            .env("PRNSD_STATE_DIR", state.path())
            .env("HOME", home.path())
            .env_remove("RUST_LOG")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|error| panic!("{error}"));
        let stderr = child
            .stderr
            .take()
            .unwrap_or_else(|| panic!("stderr is piped"));
        let (sender, lines) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in std::io::BufReader::new(stderr)
                .lines()
                .map_while(Result::ok)
            {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            lines,
            reader: Some(reader),
            captured: Vec::new(),
        }
    }

    fn wait_until_ready(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(remaining) {
                Ok(line) => {
                    let ready = line.contains("\"event\":\"daemon_ready\"");
                    self.captured.push(line);
                    if ready {
                        return;
                    }
                }
                Err(error) => panic!(
                    "service did not become ready ({error:?}):\n{}",
                    self.captured.join("\n")
                ),
            }
        }
        panic!("service readiness timed out:\n{}", self.captured.join("\n"));
    }

    fn wait_for_exit(mut self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let status = self
                .child
                .try_wait()
                .unwrap_or_else(|error| panic!("{error}"));
            if let Some(status) = status {
                self.reader
                    .take()
                    .unwrap_or_else(|| panic!("log reader is present"))
                    .join()
                    .unwrap_or_else(|_| panic!("log reader panicked"));
                self.captured.extend(self.lines.try_iter());
                assert!(status.success(), "{}", self.captured.join("\n"));
                return;
            }
            assert!(
                Instant::now() < deadline,
                "service did not stop:\n{}",
                self.captured.join("\n")
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for RunningService {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_tcp_port() -> u16 {
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .unwrap_or_else(|error| panic!("{error}"));
    listener
        .local_addr()
        .unwrap_or_else(|error| panic!("{error}"))
        .port()
}

fn distinct_ports(count: usize) -> Vec<u16> {
    let mut ports = Vec::new();
    while ports.len() < count {
        let port = free_tcp_port();
        if !ports.contains(&port) {
            ports.push(port);
        }
    }
    ports
}

fn command(state: &TestDirectory, home: &TestDirectory) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_prnsd"));
    command
        .env("PRNSD_STATE_DIR", state.path())
        .env("HOME", home.path())
        .env_remove("RUST_LOG");
    command
}

fn output(command: &mut Command) -> Output {
    command.output().unwrap_or_else(|error| panic!("{error}"))
}

fn bounded_output(command: &mut Command) -> Output {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("{error}"));
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if child
            .try_wait()
            .unwrap_or_else(|error| panic!("{error}"))
            .is_some()
        {
            return child
                .wait_with_output()
                .unwrap_or_else(|error| panic!("{error}"));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let captured = child
                .wait_with_output()
                .unwrap_or_else(|error| panic!("{error}"));
            panic!(
                "command did not fail closed:\n{}{}",
                String::from_utf8_lossy(&captured.stdout),
                String::from_utf8_lossy(&captured.stderr),
            );
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn service_context_is_the_default_for_every_operator_command() {
    let config = TestDirectory::new("config");
    let state = TestDirectory::new("state");
    let home = TestDirectory::new("home");
    let ports = distinct_ports(3);
    fs::write(
        config.path().join("config"),
        format!(
            "[reticulum]\n\
             enable_transport = Yes\n\
             share_instance = Yes\n\
             shared_instance_port = {}\n\
             instance_control_port = {}\n\
             rpc_key = 000102030405060708090a0b0c0d0e0f\n\
             [interfaces]\n\
             [[Cloud Backbone]]\n\
             type = BackboneInterface\n\
             interface_enabled = Yes\n\
             listen_ip = 127.0.0.1\n\
             listen_port = {}\n\
             discoverable = No\n",
            ports[0], ports[1], ports[2],
        ),
    )
    .unwrap_or_else(|error| panic!("{error}"));

    let mut service = RunningService::start(&config, &state, &home);
    service.wait_until_ready();
    let paths = ServicePaths::in_dir(state.path());
    let service_record = running(&paths)
        .unwrap_or_else(|error| panic!("{error}"))
        .unwrap_or_else(|| panic!("service session is published"));
    assert_eq!(service_record.kind, ServiceKind::Foreground);
    let service_pid = service_record.pid;
    assert_eq!(service_pid, service.child.id());

    let status = output(command(&state, &home).args(["status", "--json"]));
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status: serde_json::Value =
        serde_json::from_slice(&status.stdout).unwrap_or_else(|error| panic!("{error}"));
    assert!(status["transport_id"].as_str().is_some());

    let interfaces = output(command(&state, &home).args(["interfaces", "list"]));
    assert!(
        interfaces.status.success(),
        "{}",
        String::from_utf8_lossy(&interfaces.stderr)
    );
    let interfaces = String::from_utf8_lossy(&interfaces.stdout);
    assert!(interfaces.contains("Cloud Backbone"), "{interfaces}");
    assert!(!interfaces.contains("Default Interface"), "{interfaces}");

    let refresh = output(command(&state, &home).args(["nnpages", "refresh"]));
    assert!(
        refresh.status.success(),
        "{}",
        String::from_utf8_lossy(&refresh.stderr)
    );

    let mut bare_command = command(&state, &home);
    let bare = output(&mut bare_command);
    assert!(
        bare.status.success(),
        "{}",
        String::from_utf8_lossy(&bare.stderr)
    );
    assert!(
        String::from_utf8_lossy(&bare.stderr).contains("already running"),
        "{}",
        String::from_utf8_lossy(&bare.stderr)
    );
    assert_eq!(
        running(&paths)
            .unwrap_or_else(|error| panic!("{error}"))
            .map(|record| record.pid),
        Some(service_pid)
    );

    let repeated_start = output(command(&state, &home).args(["start", "--detach"]));
    assert!(
        repeated_start.status.success(),
        "{}",
        String::from_utf8_lossy(&repeated_start.stderr)
    );
    assert_eq!(
        running(&paths)
            .unwrap_or_else(|error| panic!("{error}"))
            .map(|record| record.pid),
        Some(service_pid)
    );

    let duplicate = bounded_output(command(&state, &home).arg("run"));
    assert!(!duplicate.status.success());
    assert!(
        String::from_utf8_lossy(&duplicate.stderr).contains("already running"),
        "{}",
        String::from_utf8_lossy(&duplicate.stderr)
    );
    assert!(!home.path().join(".reticulum").exists());

    let stop = output(command(&state, &home).arg("stop"));
    assert!(
        stop.status.success(),
        "{}",
        String::from_utf8_lossy(&stop.stderr)
    );
    service.wait_for_exit();
}
