use std::fs;
use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Command, Stdio};

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "prnsd-config-diagnostics-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn invalid_config_exits_before_startup_and_renders_every_actionable_error() {
    let directory = TestDirectory::new();
    let path = directory.0.join("config");
    fs::write(
        &path,
        "[reticulum]\ndiscover_interfaces = perhaps\n[interfaces]\n[[Hub]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_port = many\noutgoing = sideways\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_prnsd"))
        .args([
            "run",
            "--log-format",
            "json",
            "--config",
            directory.0.to_str().unwrap(),
        ])
        .env_remove("RUST_LOG")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(rendered.matches(": error[").count(), 3);
    assert!(rendered.contains(&path.display().to_string()));
    assert!(rendered.contains("[reticulum] > discover_interfaces"));
    assert!(rendered.contains("[interfaces] > [[Hub]] > target_port"));
    assert!(rendered.contains("[interfaces] > [[Hub]] > outgoing"));
    assert!(rendered.contains("accepted:"));
    assert!(rendered.contains("fix:"));
    assert!(!rendered.contains("\"event\":\"network_identity_failed\""));
    assert!(!rendered.contains("\"event\":\"config_invalid\""));
}

#[test]
fn panic_on_interface_error_stops_before_readiness_after_an_initial_bind_failure() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let directory = TestDirectory::new();
    fs::write(
        directory.0.join("config"),
        format!(
            "[reticulum]\nshare_instance = No\npanic_on_interface_error = Yes\n[logging]\nloglevel = 7\nlogtimestamps = No\n[interfaces]\n[[Occupied]]\ntype = TCPServerInterface\nenabled = Yes\nlisten_ip = 127.0.0.1\nlisten_port = {port}\n"
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_prnsd"))
        .args([
            "run",
            "--log-format",
            "json",
            "--config",
            directory.0.to_str().unwrap(),
        ])
        .env_remove("RUST_LOG")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(rendered.contains("\"event\":\"interface_start_failed\""));
    assert!(rendered.contains("\"event\":\"interface_failure_shutdown\""));
    assert!(!rendered.contains("\"event\":\"daemon_ready\""));
    assert!(!rendered.contains("\"event\":\"daemon_ready_degraded\""));
    assert!(!rendered.contains("\"timestamp\":"));

    let overridden = Command::new(env!("CARGO_BIN_EXE_prnsd"))
        .args([
            "run",
            "--log-format",
            "json",
            "--config",
            directory.0.to_str().unwrap(),
        ])
        .env("RUST_LOG", "error")
        .output()
        .unwrap();
    let overridden = format!(
        "{}{}",
        String::from_utf8_lossy(&overridden.stdout),
        String::from_utf8_lossy(&overridden.stderr)
    );
    assert!(overridden.contains("\"event\":\"interface_failure_shutdown\""));
    assert!(!overridden.contains("\"event\":\"interface_start_failed\""));
}

#[test]
fn a_retrying_interface_reports_degraded_readiness_without_panicking_by_default() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    let directory = TestDirectory::new();
    fs::write(
        directory.0.join("config"),
        format!(
            "[reticulum]\nshare_instance = No\n[logging]\nloglevel = 7\n[interfaces]\n[[Retrying]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_host = 127.0.0.1\ntarget_port = {port}\n"
        ),
    )
    .unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_prnsd"))
        .args([
            "run",
            "--log-format",
            "json",
            "--config",
            directory.0.to_str().unwrap(),
        ])
        .env_remove("RUST_LOG")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = child.stderr.take().unwrap();
    let (line_tx, line_rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in std::io::BufReader::new(stderr)
            .lines()
            .map_while(Result::ok)
        {
            let _ = line_tx.send(line);
        }
    });

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut ready = None;
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let Ok(line) = line_rx.recv_timeout(remaining) else {
            break;
        };
        if line.contains("\"event\":\"daemon_ready_degraded\"") {
            ready = Some(line);
            break;
        }
    }
    let _ = child.kill();
    child.wait().unwrap();
    reader.join().unwrap();

    let ready = ready.expect("the daemon reports degraded readiness");
    assert!(ready.contains("\"online\":0"));
    assert!(ready.contains("\"listening\":0"));
    assert!(ready.contains("\"retrying\":1"));
    assert!(ready.contains("\"failed\":0"));
}
