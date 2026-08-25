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
        "[reticulum]\ndiscover_interfaces = perhaps\n[interfaces]\n[[Hub]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_host = 127.0.0.1\ntarget_port = many\noutgoing = sideways\n",
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
    assert!(rendered.contains(&format!(
        "prnsd interfaces repair --config {}",
        directory.0.display()
    )));
    assert!(!rendered.contains("\"event\":\"network_identity_failed\""));
    assert!(!rendered.contains("\"event\":\"config_invalid\""));
}

#[test]
fn prns_resource_memory_limits_are_applied_before_daemon_readiness() {
    let directory = TestDirectory::new();
    fs::write(
        directory.0.join("config"),
        "[reticulum]\nshare_instance = No\n[prns]\nresource_mem_in = 2 KiB\nresource_mem_out = 0\n[logging]\nloglevel = 7\n",
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
    let (line_sender, line_receiver) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in std::io::BufReader::new(stderr)
            .lines()
            .map_while(Result::ok)
        {
            if line_sender.send(line).is_err() {
                break;
            }
        }
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut events = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        match line_receiver.recv_timeout(remaining) {
            Ok(line) => {
                let ready = line.contains("\"event\":\"daemon_ready\"")
                    || line.contains("\"event\":\"daemon_ready_degraded\"");
                events.push(line);
                if ready {
                    break;
                }
            }
            Err(
                std::sync::mpsc::RecvTimeoutError::Timeout
                | std::sync::mpsc::RecvTimeoutError::Disconnected,
            ) => break,
        }
    }
    let _ = child.kill();
    child.wait().unwrap();
    reader.join().unwrap();

    let configured = events
        .iter()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|event| event["event"] == "resource_memory_limits_configured")
        .unwrap_or_else(|| panic!("missing Resource limit event:\n{}", events.join("\n")));
    assert_eq!(configured["incoming_bytes"], 2 * 1024);
    assert_eq!(configured["outgoing_bytes"], 0);
    assert_eq!(configured["changes_require_restart"], true);
    assert!(events.iter().any(|line| {
        line.contains("\"event\":\"daemon_ready\"")
            || line.contains("\"event\":\"daemon_ready_degraded\"")
    }));
    assert!(!events
        .iter()
        .any(|line| { line.contains("\"event\":\"config_warning\"") && line.contains("[prns]") }));
}

#[test]
fn remaining_follow_ons_warn_while_blackhole_exchange_does_not() {
    let directory = TestDirectory::new();
    let path = directory.0.join("config");
    fs::write(
        &path,
        "[reticulum]\nshare_instance = No\nenable_remote_management = Yes\nremote_management_allowed = 00112233445566778899aabbccddeeff\nrespond_to_probes = No\npublish_blackhole = No\n[logging]\nloglevel = 7\n[interfaces]\n[[LAN]]\ntype = AutoInterface\nenabled = Yes\nignore_config_warnings = Yes\n",
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
    let (line_sender, line_receiver) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in std::io::BufReader::new(stderr)
            .lines()
            .map_while(Result::ok)
        {
            if line_sender.send(line).is_err() {
                break;
            }
        }
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut lines = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        match line_receiver.recv_timeout(remaining) {
            Ok(line) => {
                let ready = line.contains("\"event\":\"daemon_ready\"")
                    || line.contains("\"event\":\"daemon_ready_degraded\"");
                lines.push(line);
                if ready {
                    while let Ok(line) =
                        line_receiver.recv_timeout(std::time::Duration::from_millis(200))
                    {
                        lines.push(line);
                    }
                    break;
                }
            }
            Err(
                std::sync::mpsc::RecvTimeoutError::Timeout
                | std::sync::mpsc::RecvTimeoutError::Disconnected,
            ) => break,
        }
    }
    let _ = child.kill();
    child.wait().unwrap();
    reader.join().unwrap();
    lines.extend(line_receiver.try_iter());
    let rendered = lines.join("\n");
    assert!(
        rendered.contains("\"event\":\"config_warning\""),
        "missing config warning in daemon output:\n{rendered}"
    );
    assert!(
        rendered.contains("\"code\":\"unsupported_setting\""),
        "missing warning code in daemon output:\n{rendered}"
    );
    assert!(
        rendered.contains(&path.display().to_string().replace('\\', "\\\\")),
        "missing config path in daemon output:\n{rendered}"
    );
    assert!(
        rendered.contains("ignore_config_warnings"),
        "missing setting name in daemon output:\n{rendered}"
    );
    assert!(
        rendered.contains("\"event\":\"remote_management_enabled\""),
        "remote management was not activated before readiness:\n{rendered}"
    );
    assert!(
        !rendered.lines().any(|line| {
            line.contains("\"event\":\"config_warning\"")
                && (line.contains("enable_remote_management")
                    || line.contains("remote_management_allowed")
                    || line.contains("respond_to_probes")
                    || line.contains("publish_blackhole"))
        }),
        "applied daemon settings still warned as unsupported:\n{rendered}"
    );
    assert!(
        rendered.contains("\"event\":\"daemon_ready\"")
            || rendered.contains("\"event\":\"daemon_ready_degraded\""),
        "missing readiness in daemon output:\n{rendered}"
    );
}

#[test]
fn probe_responder_activates_before_readiness_without_a_follow_on_warning() {
    let directory = TestDirectory::new();
    fs::write(
        directory.0.join("config"),
        "[reticulum]\nshare_instance = No\nrespond_to_probes = Yes\n[logging]\nloglevel = 7\n",
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
    let (line_sender, line_receiver) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in std::io::BufReader::new(stderr)
            .lines()
            .map_while(Result::ok)
        {
            if line_sender.send(line).is_err() {
                break;
            }
        }
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut lines = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        match line_receiver.recv_timeout(remaining) {
            Ok(line) => {
                let ready = line.contains("\"event\":\"daemon_ready\"");
                lines.push(line);
                if ready {
                    break;
                }
            }
            Err(
                std::sync::mpsc::RecvTimeoutError::Timeout
                | std::sync::mpsc::RecvTimeoutError::Disconnected,
            ) => break,
        }
    }
    let _ = child.kill();
    child.wait().unwrap();
    reader.join().unwrap();
    let rendered = lines.join("\n");
    let activated = rendered
        .find("\"event\":\"probe_responder_enabled\"")
        .expect("probe responder activates");
    let ready = rendered
        .find("\"event\":\"daemon_ready\"")
        .expect("daemon becomes ready");
    assert!(activated < ready);
    assert!(!rendered.lines().any(|line| {
        line.contains("\"event\":\"config_warning\"") && line.contains("respond_to_probes")
    }));
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
fn occupied_shared_instance_control_port_fails_before_readiness() {
    let bus_probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let bus_port = bus_probe.local_addr().unwrap().port();
    drop(bus_probe);
    let occupied_control = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let control_port = occupied_control.local_addr().unwrap().port();
    let directory = TestDirectory::new();
    fs::write(
        directory.0.join("config"),
        format!(
            "[reticulum]\nshare_instance = Yes\nshared_instance_type = TCP\nshared_instance_port = {bus_port}\ninstance_control_port = {control_port}\n[logging]\nloglevel = 7\nlogtimestamps = No\n"
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
    assert!(rendered.contains("\"event\":\"shared_instance_endpoint_unavailable\""));
    assert!(rendered.contains("\"endpoint\":\"tcp_control\""));
    assert!(rendered.contains("\"error_kind\":\"AddrInUse\""));
    assert!(!rendered.contains("\"event\":\"shared_instance_started\""));
    assert!(!rendered.contains("\"event\":\"daemon_ready\""));
    assert!(!rendered.contains("\"event\":\"daemon_ready_degraded\""));
    assert!(
        std::net::TcpListener::bind(("127.0.0.1", bus_port)).is_ok(),
        "the failed daemon cannot leave its data bus reserved"
    );
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

#[test]
fn an_idle_i2p_interface_constructs_before_ready_without_a_sam_router() {
    let directory = TestDirectory::new();
    fs::write(
        directory.0.join("config"),
        "[reticulum]\nshare_instance = No\n[logging]\nloglevel = 7\n[interfaces]\n[[Private I2P]]\ntype = I2PInterface\nenabled = Yes\n",
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
    let mut lines = Vec::new();
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let Ok(line) = line_rx.recv_timeout(remaining) else {
            break;
        };
        let ready = line.contains("\"event\":\"daemon_ready\"");
        lines.push(line);
        if ready {
            while let Ok(line) = line_rx.recv_timeout(std::time::Duration::from_millis(200)) {
                lines.push(line);
            }
            break;
        }
    }
    let _ = child.kill();
    child.wait().unwrap();
    reader.join().unwrap();
    lines.extend(line_rx.try_iter());
    let rendered = lines.join("\n");

    assert!(
        rendered.contains("\"event\":\"interface_started\""),
        "missing interface-start event in daemon output:\n{rendered}"
    );
    assert!(rendered.contains("\"medium\":\"i2p\""));
    assert!(rendered.contains("\"event\":\"daemon_ready\""));
    assert!(rendered.contains("\"online\":1"));
    assert!(!rendered.contains("\"event\":\"daemon_ready_degraded\""));
}
