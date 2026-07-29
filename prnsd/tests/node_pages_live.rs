#![cfg(unix)]

use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path =
            std::env::temp_dir().join(format!("prnsd-pages-live-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path).unwrap_or_else(|error| panic!("{error}"));
        fs::write(
            path.join("config"),
            "[reticulum]\nenable_transport = Yes\nshare_instance = No\nannounce_node_page = No\n[logging]\nloglevel = 7\nlogtimestamps = No\n[interfaces]\n",
        )
        .unwrap_or_else(|error| panic!("{error}"));
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

struct RunningDaemon {
    child: Child,
    lines: Receiver<String>,
    reader: Option<JoinHandle<()>>,
    captured: Vec<String>,
}

impl RunningDaemon {
    fn start(directory: &TestDirectory) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_prnsd"))
            .args(["run", "--config"])
            .arg(directory.path())
            .args(["--log-format", "json"])
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

    fn terminate(mut self) {
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
        assert!(status.success(), "{}", self.captured.join("\n"));
    }
}

impl Drop for RunningDaemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn refresh(directory: &TestDirectory) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_prnsd"))
        .args(["pages", "refresh", "--config"])
        .arg(directory.path())
        .output()
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap_or_else(|error| panic!("{error}"))
}

#[test]
fn foreground_daemon_reconciles_page_paths_on_operator_request() {
    let directory = TestDirectory::new();
    let mut daemon = RunningDaemon::start(&directory);
    daemon.wait_until_ready();

    let pages = directory.path().join("pages");
    fs::create_dir(&pages).unwrap_or_else(|error| panic!("{error}"));
    fs::write(pages.join("index.mu"), b"index").unwrap_or_else(|error| panic!("{error}"));
    fs::write(pages.join("about.mu"), b"about").unwrap_or_else(|error| panic!("{error}"));
    let added = refresh(&directory);
    assert!(added.contains("2 page route(s): 2 added, 0 removed, 0 unchanged"));

    fs::remove_file(pages.join("about.mu")).unwrap_or_else(|error| panic!("{error}"));
    let removed = refresh(&directory);
    assert!(removed.contains("1 page route(s): 0 added, 1 removed, 1 unchanged"));

    daemon.terminate();
}
