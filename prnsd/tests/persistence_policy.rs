#![cfg(unix)]

use std::fs;
use std::io::BufRead;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(name: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "prnsd-persistence-{name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap_or_else(|error| panic!("{error}"));
        fs::write(
            path.join("config"),
            "[reticulum]\nenable_transport = Yes\nshare_instance = No\n[logging]\nloglevel = 7\nlogtimestamps = No\n[interfaces]\n",
        )
        .unwrap_or_else(|error| panic!("{error}"));
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn run_to_completion(&self, policy: &str) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_prnsd"))
            .args([
                "run",
                "--config",
                self.path()
                    .to_str()
                    .unwrap_or_else(|| panic!("temporary path is UTF-8")),
                "--log-format",
                "json",
                "--persistence-policy",
                policy,
            ])
            .env_remove("RUST_LOG")
            .output()
            .unwrap_or_else(|error| panic!("{error}"))
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let persistence = self.0.join("storage/prns");
        if persistence.exists() {
            let _ = fs::set_permissions(&persistence, fs::Permissions::from_mode(0o700));
        }
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct RunningDaemon {
    child: Child,
    lines: Receiver<String>,
    reader: Option<JoinHandle<()>>,
    captured: Vec<String>,
}

impl RunningDaemon {
    fn start(directory: &TestDirectory, policy: &str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_prnsd"))
            .args([
                "run",
                "--config",
                directory
                    .path()
                    .to_str()
                    .unwrap_or_else(|| panic!("temporary path is UTF-8")),
                "--log-format",
                "json",
                "--persistence-policy",
                policy,
            ])
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
                    "daemon did not become ready ({error:?}):\n{}",
                    self.captured.join("\n")
                ),
            }
        }
        panic!("daemon readiness timed out:\n{}", self.captured.join("\n"));
    }

    fn terminate(mut self) -> (ExitStatus, String) {
        let signal = Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status()
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(signal.success());
        let status = self.child.wait().unwrap_or_else(|error| panic!("{error}"));
        self.reader
            .take()
            .unwrap_or_else(|| panic!("log reader is present"))
            .join()
            .unwrap_or_else(|_| panic!("log reader panicked"));
        self.captured.extend(self.lines.try_iter());
        (status, self.captured.join("\n"))
    }
}

impl Drop for RunningDaemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn required_persistence_refuses_an_unavailable_transport_identity() {
    let directory = TestDirectory::new("required-startup");
    let storage = directory.path().join("storage");
    fs::create_dir(&storage).unwrap_or_else(|error| panic!("{error}"));
    fs::create_dir(storage.join("transport_identity")).unwrap_or_else(|error| panic!("{error}"));

    let output = directory.run_to_completion("required");

    assert!(!output.status.success());
    let rendered = String::from_utf8_lossy(&output.stderr);
    assert!(rendered.contains("\"event\":\"transport_identity_unavailable\""));
    assert!(rendered.contains("required transport identity is unavailable"));
    assert!(!rendered.contains("\"event\":\"daemon_ready\""));
}

#[test]
fn best_effort_retains_transport_identity_and_node_persistence_fallbacks() {
    let directory = TestDirectory::new("best-effort");
    let storage = directory.path().join("storage");
    fs::create_dir(&storage).unwrap_or_else(|error| panic!("{error}"));
    fs::create_dir(storage.join("transport_identity")).unwrap_or_else(|error| panic!("{error}"));
    fs::write(storage.join("prns"), b"not a directory").unwrap_or_else(|error| panic!("{error}"));
    let mut daemon = RunningDaemon::start(&directory, "best-effort");
    daemon.wait_until_ready();

    let (status, rendered) = daemon.terminate();
    assert!(status.success(), "{rendered}");
    assert!(rendered.contains("\"event\":\"identity_ephemeral\""));
    assert!(rendered.contains("\"event\":\"persistence_unavailable\""));
}

#[test]
fn best_effort_still_requires_remote_control_identity_custody() {
    let directory = TestDirectory::new("remote-control-required");
    let storage = directory.path().join("storage");
    fs::create_dir(&storage).unwrap_or_else(|error| panic!("{error}"));
    fs::write(storage.join("remote_control"), b"not a directory")
        .unwrap_or_else(|error| panic!("{error}"));

    let output = directory.run_to_completion("best-effort");

    assert!(!output.status.success());
    let rendered = String::from_utf8_lossy(&output.stderr);
    assert!(rendered.contains("\"event\":\"remote_control_identity_unavailable\""));
    assert!(rendered.contains("required RemoteControl identities are unavailable"));
    assert!(!rendered.contains("\"event\":\"daemon_ready\""));
}

#[test]
fn required_sigterm_waits_for_both_final_flushes() {
    let directory = TestDirectory::new("graceful");
    let mut daemon = RunningDaemon::start(&directory, "required");
    daemon.wait_until_ready();

    let (status, rendered) = daemon.terminate();
    assert!(status.success(), "{rendered}");
    let state = rendered
        .find("\"event\":\"state_persisted\"")
        .unwrap_or_else(|| panic!("state flush missing:\n{rendered}"));
    let ratchets = rendered
        .find("\"event\":\"ratchets_persisted\"")
        .unwrap_or_else(|| panic!("ratchet flush missing:\n{rendered}"));
    let shutdown = rendered
        .find("\"event\":\"daemon_shutdown\"")
        .unwrap_or_else(|| panic!("shutdown acknowledgement missing:\n{rendered}"));
    assert!(state < shutdown);
    assert!(ratchets < shutdown);
}

#[test]
fn required_final_write_failure_returns_a_nonzero_process_status() {
    let directory = TestDirectory::new("sabotaged");
    let mut daemon = RunningDaemon::start(&directory, "required");
    daemon.wait_until_ready();
    let persistence = directory.path().join("storage/prns");
    let displaced = directory.path().join("storage/prns-displaced");
    fs::rename(&persistence, &displaced).unwrap_or_else(|error| panic!("{error}"));
    fs::write(&persistence, b"not a directory").unwrap_or_else(|error| panic!("{error}"));

    let (status, rendered) = daemon.terminate();
    fs::remove_file(&persistence).unwrap_or_else(|error| panic!("{error}"));
    fs::rename(&displaced, &persistence).unwrap_or_else(|error| panic!("{error}"));
    assert!(!status.success(), "{rendered}");
    assert!(rendered.contains("\"event\":\"persistence_failed\""));
    assert!(rendered.contains("required persistence failed to flush"));
}
