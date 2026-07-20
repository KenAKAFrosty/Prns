use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use prnsd_control::{
    config_digest, request_reload, running, start, stop, LaunchSpec, LogLane, ReloadResult,
    ServicePaths, ServiceRecord, StartOutcome,
};

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(name: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "prnsd-live-apply-{name}-{}-{nanos}",
            std::process::id()
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

struct ManagedDaemon {
    paths: ServicePaths,
}

impl Drop for ManagedDaemon {
    fn drop(&mut self) {
        let _ = stop(&self.paths);
    }
}

fn launch(config: &TestDirectory, state: &TestDirectory) -> (ManagedDaemon, ServiceRecord) {
    let paths = ServicePaths::in_dir(state.path());
    let args = vec![
        OsString::from("run"),
        OsString::from("--log-format"),
        OsString::from("json"),
        OsString::from("--config"),
        config.path().as_os_str().to_owned(),
    ];
    let binary = Path::new(env!("CARGO_BIN_EXE_prnsd"));
    let working_dir = std::env::current_dir().unwrap_or_else(|error| panic!("{error}"));
    let outcome = start(
        &paths,
        LaunchSpec {
            binary,
            managed_binary: None,
            args: &args,
            working_dir: &working_dir,
            log_lane: LogLane::Json,
            signature: 41,
            version: env!("CARGO_PKG_VERSION"),
        },
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let record = match outcome {
        StartOutcome::Started(record) | StartOutcome::AlreadyRunning(record) => record,
    };
    (ManagedDaemon { paths }, record)
}

fn free_tcp_port() -> u16 {
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .unwrap_or_else(|error| panic!("{error}"));
    listener
        .local_addr()
        .unwrap_or_else(|error| panic!("{error}"))
        .port()
}

#[test]
fn managed_interface_changes_apply_without_restarting_the_daemon() {
    let config = TestDirectory::new("config");
    let state = TestDirectory::new("state");
    fs::write(
        config.path().join("config"),
        "[reticulum]\nshare_instance = No\n[interfaces]\n",
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let binary = Path::new(env!("CARGO_BIN_EXE_prnsd"));
    let (daemon, record) = launch(&config, &state);
    let paths = daemon.paths.clone();
    let socket = std::net::UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .unwrap_or_else(|error| panic!("{error}"));
    let port = socket
        .local_addr()
        .unwrap_or_else(|error| panic!("{error}"))
        .port();
    drop(socket);

    let add = Command::new(binary)
        .args([
            "interfaces",
            "add",
            "udp",
            "--name",
            "UDP",
            "--listen-ip",
            "127.0.0.1",
            "--listen-port",
        ])
        .arg(port.to_string())
        .args(["--apply", "--config"])
        .arg(config.path())
        .env("PRNSD_STATE_DIR", state.path())
        .output()
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(add.status.success(), "{}", String::from_utf8_lossy(&add.stderr));
    assert!(String::from_utf8_lossy(&add.stdout).contains("applied without restarting"));
    assert_eq!(
        running(&paths)
            .unwrap_or_else(|error| panic!("{error}"))
            .map(|current| current.pid),
        Some(record.pid)
    );
    let configured = fs::read_to_string(config.path().join("config"))
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(configured.contains("type = UDPInterface"));
    let plan = prns_config::parse_and_plan(&configured)
        .unwrap_or_else(|error| panic!("{error}"))
        .value;
    assert_eq!(plan.interfaces.len(), 1);
    assert_eq!(plan.interfaces[0].name, "UDP");

    let disable = Command::new(binary)
        .args(["interfaces", "disable", "UDP", "--apply", "--config"])
        .arg(config.path())
        .env("PRNSD_STATE_DIR", state.path())
        .output()
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        disable.status.success(),
        "{}",
        String::from_utf8_lossy(&disable.stderr)
    );
    assert_eq!(
        running(&paths)
            .unwrap_or_else(|error| panic!("{error}"))
            .map(|current| current.pid),
        Some(record.pid)
    );

    assert_eq!(
        request_reload(&paths, config_digest(b"stale configuration"))
            .unwrap_or_else(|error| panic!("{error}")),
        Some(ReloadResult::Rejected)
    );

    let changed = fs::read_to_string(config.path().join("config"))
        .unwrap_or_else(|error| panic!("{error}"))
        .replace("share_instance = No", "share_instance = No\nenable_transport = Yes");
    fs::write(config.path().join("config"), changed)
        .unwrap_or_else(|error| panic!("{error}"));
    let restart_required = Command::new(binary)
        .args(["interfaces", "apply", "--config"])
        .arg(config.path())
        .env("PRNSD_STATE_DIR", state.path())
        .output()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(restart_required.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&restart_required.stderr).contains("restart prnsd"));
    assert_eq!(
        running(&paths)
            .unwrap_or_else(|error| panic!("{error}"))
            .map(|current| current.pid),
        Some(record.pid)
    );

    drop(daemon);
    assert!(
        running(&paths)
            .unwrap_or_else(|error| panic!("{error}"))
            .is_none()
    );
}

#[test]
fn managed_shared_client_directs_apply_to_the_routing_owner() {
    let instance_port = free_tcp_port();
    let mut control_port = free_tcp_port();
    while control_port == instance_port {
        control_port = free_tcp_port();
    }
    let source = format!(
        "[reticulum]\nshare_instance = Yes\nshared_instance_port = {instance_port}\ninstance_control_port = {control_port}\nrpc_key = 000102030405060708090a0b0c0d0e0f\n[interfaces]\n"
    );
    let owner_config = TestDirectory::new("owner-config");
    let owner_state = TestDirectory::new("owner-state");
    fs::write(owner_config.path().join("config"), &source)
        .unwrap_or_else(|error| panic!("{error}"));
    let client_config = TestDirectory::new("client-config");
    let client_state = TestDirectory::new("client-state");
    fs::write(client_config.path().join("config"), source)
        .unwrap_or_else(|error| panic!("{error}"));

    let (owner, owner_record) = launch(&owner_config, &owner_state);
    let (client, client_record) = launch(&client_config, &client_state);
    assert_ne!(owner_record.pid, client_record.pid);

    let output = Command::new(env!("CARGO_BIN_EXE_prnsd"))
        .args([
            "interfaces",
            "add",
            "usb-auto",
            "--name",
            "USB",
            "--apply",
            "--config",
        ])
        .arg(client_config.path())
        .env("PRNSD_STATE_DIR", client_state.path())
        .output()
        .unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("routing-table owner"));
    assert_eq!(
        running(&owner.paths)
            .unwrap_or_else(|error| panic!("{error}"))
            .map(|record| record.pid),
        Some(owner_record.pid)
    );
    assert_eq!(
        running(&client.paths)
            .unwrap_or_else(|error| panic!("{error}"))
            .map(|record| record.pid),
        Some(client_record.pid)
    );

    drop(client);
    drop(owner);
}
