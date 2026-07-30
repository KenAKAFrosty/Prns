#![cfg(unix)]

use std::ffi::OsString;
use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use prnsd_control::{
    active_config_dir, start, stop, LaunchSpec, LogLane, ServicePaths, StartOutcome,
};

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let directory = Self::empty("config");
        fs::write(
            directory.path().join("config"),
            "[reticulum]\nenable_transport = Yes\nshare_instance = No\n[logging]\nloglevel = 7\nlogtimestamps = No\n[interfaces]\n",
        )
        .unwrap_or_else(|error| panic!("{error}"));
        directory
    }

    fn empty(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "prnsd-nnpages-live-{label}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap_or_else(|error| panic!("{error}"));
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

struct ManagedDaemon {
    paths: ServicePaths,
}

impl ManagedDaemon {
    fn start(config: &TestDirectory, state: &TestDirectory) -> Self {
        let paths = ServicePaths::in_dir(state.path());
        let args = vec![
            OsString::from("run"),
            OsString::from("--log-format"),
            OsString::from("json"),
            OsString::from("--config"),
            config.path().as_os_str().to_owned(),
        ];
        let working_dir = std::env::current_dir().unwrap_or_else(|error| panic!("{error}"));
        let outcome = start(
            &paths,
            LaunchSpec {
                binary: Path::new(env!("CARGO_BIN_EXE_prnsd")),
                managed_binary: None,
                args: &args,
                working_dir: &working_dir,
                log_lane: LogLane::Json,
                signature: 47,
                version: env!("CARGO_PKG_VERSION"),
            },
        )
        .unwrap_or_else(|error| panic!("{error}"));
        assert!(matches!(
            outcome,
            StartOutcome::Started(_) | StartOutcome::AlreadyRunning(_)
        ));
        Self { paths }
    }
}

impl Drop for ManagedDaemon {
    fn drop(&mut self) {
        let _ = stop(&self.paths);
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
        .args(["nnpages", "refresh", "--config"])
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

    let nnpages = directory.path().join("nnpages");
    let pages = nnpages.join("pages");
    fs::create_dir_all(&pages).unwrap_or_else(|error| panic!("{error}"));
    fs::write(pages.join("index.mu"), b"index").unwrap_or_else(|error| panic!("{error}"));
    fs::write(pages.join("about.mu"), b"about").unwrap_or_else(|error| panic!("{error}"));
    let files = nnpages.join("files");
    fs::create_dir(&files).unwrap_or_else(|error| panic!("{error}"));
    fs::write(files.join("demo.txt"), b"demo").unwrap_or_else(|error| panic!("{error}"));
    fs::write(
        nnpages.join("settings.toml"),
        "announce = false\nannounce_interval_minutes = 360\n",
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let added = refresh(&directory);
    assert!(added.contains("3 hosted route(s): 3 added, 0 removed, 0 unchanged"));

    fs::remove_file(pages.join("about.mu")).unwrap_or_else(|error| panic!("{error}"));
    let removed = refresh(&directory);
    assert!(removed.contains("2 hosted route(s): 0 added, 1 removed, 2 unchanged"));

    daemon.terminate();
}

#[test]
fn omitted_config_targets_the_active_managed_daemon_for_rename() {
    let config = TestDirectory::new();
    let state = TestDirectory::empty("state");
    let pages = config.path().join("nnpages/pages");
    fs::create_dir_all(&pages).unwrap_or_else(|error| panic!("{error}"));
    fs::write(pages.join("index.mu"), b"index").unwrap_or_else(|error| panic!("{error}"));
    let daemon = ManagedDaemon::start(&config, &state);

    assert_eq!(
        active_config_dir(&daemon.paths).unwrap_or_else(|error| panic!("{error}")),
        Some(config.path().to_path_buf())
    );
    let rename = Command::new(env!("CARGO_BIN_EXE_prnsd"))
        .args(["nnpages", "rename", "Managed Node"])
        .env("PRNSD_STATE_DIR", state.path())
        .output()
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        rename.status.success(),
        "{}",
        String::from_utf8_lossy(&rename.stderr)
    );
    assert!(
        String::from_utf8_lossy(&rename.stdout)
            .contains("Announced the new name on all interfaces."),
        "{}",
        String::from_utf8_lossy(&rename.stdout)
    );
    assert_eq!(
        fs::read_to_string(config.path().join("nnpages/name"))
            .unwrap_or_else(|error| panic!("{error}")),
        "Managed Node"
    );

    let explicit = TestDirectory::empty("explicit");
    let override_rename = Command::new(env!("CARGO_BIN_EXE_prnsd"))
        .args(["nnpages", "rename", "Explicit Node", "--config"])
        .arg(explicit.path())
        .env("PRNSD_STATE_DIR", state.path())
        .output()
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        override_rename.status.success(),
        "{}",
        String::from_utf8_lossy(&override_rename.stderr)
    );
    assert_eq!(
        fs::read_to_string(explicit.path().join("nnpages/name"))
            .unwrap_or_else(|error| panic!("{error}")),
        "Explicit Node"
    );
    assert_eq!(
        fs::read_to_string(config.path().join("nnpages/name"))
            .unwrap_or_else(|error| panic!("{error}")),
        "Managed Node"
    );
}

#[test]
fn seed_establishes_settings_and_refreshes_when_settings_are_the_only_change() {
    let config = TestDirectory::new();
    let state = TestDirectory::empty("seed-state");
    let _daemon = ManagedDaemon::start(&config, &state);
    let run_seed = || {
        Command::new(env!("CARGO_BIN_EXE_prnsd"))
            .args(["nnpages", "seed"])
            .env("PRNSD_STATE_DIR", state.path())
            .output()
            .unwrap_or_else(|error| panic!("{error}"))
    };

    let initial = run_seed();
    assert!(
        initial.status.success(),
        "{}",
        String::from_utf8_lossy(&initial.stderr)
    );
    let root = config.path().join("nnpages");
    let settings = root.join("settings.toml");
    assert!(root.join("pages").is_dir());
    assert!(root.join("files").is_dir());
    assert_eq!(
        fs::read_to_string(&settings).unwrap_or_else(|error| panic!("{error}")),
        "announce = true\nannounce_interval_minutes = 360\n"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        assert_eq!(
            fs::metadata(&settings)
                .unwrap_or_else(|error| panic!("{error}"))
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    fs::remove_file(&settings).unwrap_or_else(|error| panic!("{error}"));
    let settings_only = run_seed();
    assert!(
        settings_only.status.success(),
        "{}",
        String::from_utf8_lossy(&settings_only.stderr)
    );
    let settings_only_output = String::from_utf8_lossy(&settings_only.stdout);
    assert!(settings_only_output.contains("Seeded "));
    assert!(
        settings_only_output.contains("Refreshed "),
        "{settings_only_output}"
    );

    let operator_bytes = b"this is not = valid = TOML\n";
    fs::write(&settings, operator_bytes).unwrap_or_else(|error| panic!("{error}"));
    let repeated = run_seed();
    assert!(
        repeated.status.success(),
        "{}",
        String::from_utf8_lossy(&repeated.stderr)
    );
    assert_eq!(
        fs::read(&settings).unwrap_or_else(|error| panic!("{error}")),
        operator_bytes
    );
}
