//! Separate executables, real TCP, independent identities and databases.
#![cfg(feature = "headless")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use prns_groups::fixture;
use prns_groups_service::Timestamp;
use prns_host::IdentityHash;
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const BINARY: &str = env!("CARGO_BIN_EXE_dev-group-peer");
struct Peer {
    child: Child,
    input: ChildStdin,
    replies: mpsc::Receiver<Value>,
    reader: Option<thread::JoinHandle<()>>,
    ready: Value,
}
impl Peer {
    fn open(root: &Path, connection: &str, address: &str) -> Self {
        let mut child = Command::new(BINARY)
            .args([
                "run",
                root.to_str().unwrap(),
                connection,
                address,
                "--poll-ms",
                "100",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, replies) = mpsc::channel();
        let reader = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let line = line.unwrap();
                let value: Value = serde_json::from_str(&line)
                    .unwrap_or_else(|error| panic!("non-JSON stdout: {line:?}: {error}"));
                if sender.send(value).is_err() {
                    break;
                }
            }
        });
        let mut peer = Self {
            child,
            input,
            replies,
            reader: Some(reader),
            ready: Value::Null,
        };
        peer.ready = peer
            .replies
            .recv_timeout(Duration::from_secs(15))
            .expect("peer ready");
        assert_eq!(peer.ready["event"], "ready", "{}", peer.ready);
        peer
    }
    fn command(&mut self, command: Value) -> Value {
        writeln!(self.input, "{command}").unwrap();
        self.input.flush().unwrap();
        self.replies
            .recv_timeout(Duration::from_secs(15))
            .expect("peer command result")
    }
    fn history(&mut self) -> Value {
        self.command(json!({"op":"history", "before":null, "limit":50}))
    }
    fn wait_record(&mut self, id: &Value, receipts: Option<usize>) -> Value {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let history = self.history();
            if let Some(message) = history["messages"]
                .as_array()
                .and_then(|messages| messages.iter().find(|message| &message["record_id"] == id))
            {
                if receipts.is_none_or(|expected| message["received_count"] == expected) {
                    return message.clone();
                }
            }
            assert!(
                Instant::now() < deadline,
                "record {id}, receipts {receipts:?}: {history}"
            );
            thread::sleep(Duration::from_millis(100));
        }
    }
    fn stop(&mut self) {
        assert_eq!(self.command(json!({"op":"stop"}))["event"], "stopped");
        assert!(self.child.wait().unwrap().success());
    }
    fn crash(&mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        if self.child.try_wait().unwrap().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            reader.join().unwrap();
        }
    }
}
fn init(root: &Path) -> Value {
    let result = Command::new(BINARY)
        .args(["init", root.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    serde_json::from_slice(&result.stdout).unwrap()
}
fn enrollment(root: &Path) -> Vec<PathBuf> {
    let directories: Vec<_> = (0..3)
        .map(|index| root.join(format!("member-{index}")))
        .collect();
    let identities: Vec<_> = directories
        .iter()
        .map(|directory| {
            let identity = init(directory);
            assert_eq!(identity, init(directory), "init must preserve identity");
            let text = identity["identity"].as_str().unwrap();
            IdentityHash::new(std::array::from_fn(|index| {
                u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).unwrap()
            }))
        })
        .collect();
    let fixtures = fixture::generate(
        "Headless TCP process test",
        &identities,
        Timestamp(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        ),
    )
    .unwrap();
    for (directory, fixture) in directories.iter().zip(fixtures) {
        let path = directory.join("group-fixture.json");
        std::fs::write(&path, fixture).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
    directories
}

#[test]
fn tcp_processes_route_catch_up_reconcile_receipts_and_reopen_exact_ids() {
    let directory = tempfile::tempdir().unwrap();
    let roots = enrollment(directory.path());
    let mut relay = Peer::open(&roots[1], "--listen", "127.0.0.1:0");
    let address = relay.ready["listen"].as_str().unwrap().to_owned();
    let mut alice = Peer::open(&roots[0], "--connect", &address);
    let mut carol = Peer::open(&roots[2], "--connect", &address);
    let token = alice.command(json!({"op":"request_id"}));
    assert_eq!(token["event"], "request_id");
    assert!(token["request_id"]
        .as_str()
        .unwrap()
        .starts_with("50475231"));
    let request_id = token["request_id"].clone();
    let request = json!({"op":"send", "request_id":request_id, "text":"TCP group text"});
    let accepted = alice.command(request.clone());
    assert_eq!(accepted["event"], "accepted", "{accepted}");
    relay.wait_record(&accepted["record_id"], None);
    carol.wait_record(&accepted["record_id"], None);
    alice.wait_record(&accepted["record_id"], Some(2));
    carol.stop();
    let delayed_id = alice.command(json!({"op":"request_id"}))["request_id"].clone();
    assert_ne!(delayed_id, request_id);
    let delayed =
        alice.command(json!({"op":"send", "request_id":delayed_id, "text":"Carol was offline"}));
    assert_eq!(delayed["event"], "accepted", "{delayed}");
    relay.wait_record(&delayed["record_id"], None);
    alice.crash();
    let mut carol = Peer::open(&roots[2], "--connect", &address);
    carol.wait_record(&delayed["record_id"], None);
    let mut alice = Peer::open(&roots[0], "--connect", &address);
    assert_eq!(
        alice.command(request),
        accepted,
        "durable retry must recover exact ID"
    );
    assert_eq!(
        alice.command(json!({"op":"send", "request_id":request_id, "text":"changed"}))["code"],
        "idempotency_conflict"
    );
    alice.wait_record(&delayed["record_id"], Some(2));
    assert_eq!(alice.history()["messages"].as_array().unwrap().len(), 2);
    assert_eq!(carol.history()["messages"].as_array().unwrap().len(), 2);
    let first = alice.command(json!({"op":"history", "before":null, "limit":1}));
    let cursor = first["next_before"].as_str().unwrap();
    assert_eq!(cursor.len(), 2 * prns_groups::RECORD_CURSOR_BYTES);
    let second = alice.command(json!({"op":"history", "before":cursor, "limit":1}));
    assert_eq!(second["event"], "history");
    assert!(second["next_before"].is_null());
    assert_ne!(
        first["messages"][0]["record_id"],
        second["messages"][0]["record_id"]
    );
    assert_eq!(first["current_incarnation"], second["current_incarnation"]);
    assert_eq!(
        carol.command(json!({"op":"history", "before":cursor, "limit":1}))["code"],
        "invalid_input",
        "a cursor cannot select another peer's independent store"
    );
    alice.stop();
    carol.stop();
    relay.stop();
}

#[test]
fn process_rejects_bad_input_and_fixture_identity_without_replacing_state() {
    let directory = tempfile::tempdir().unwrap();
    let roots = enrollment(directory.path());
    let identity = init(&roots[0]);
    std::fs::copy(
        roots[1].join("group-fixture.json"),
        roots[0].join("group-fixture.json"),
    )
    .unwrap();
    let result = Command::new(BINARY)
        .args(["run", roots[0].to_str().unwrap(), "--listen", "127.0.0.1:0"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert_eq!(init(&roots[0]), identity);
    let mut peer = Peer::open(&roots[1], "--listen", "127.0.0.1:0");
    assert_eq!(
        peer.command(json!({"op":"send", "request_id":"short", "text":"text"}))["code"],
        "invalid_input"
    );
    assert_eq!(
        peer.command(
            json!({"op":"send", "request_id":"01".repeat(32), "text":"old opaque request"})
        )["code"],
        "retry_expired"
    );
    assert_eq!(
        peer.command(json!({"op":"history", "limit":0}))["code"],
        "invalid_input"
    );
    writeln!(peer.input, "{}", "x".repeat(16385)).unwrap();
    peer.input.flush().unwrap();
    assert_eq!(
        peer.replies.recv_timeout(Duration::from_secs(5)).unwrap()["code"],
        "invalid_input"
    );
    assert_eq!(peer.command(json!({"op":"status"}))["event"], "status");
    assert!(peer.history()["messages"].as_array().unwrap().is_empty());
    peer.stop();
}

#[cfg(unix)]
#[test]
fn refuses_public_or_symlinked_private_paths() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let directory = tempfile::tempdir().unwrap();
    let public_root = directory.path().join("public");
    std::fs::create_dir(&public_root).unwrap();
    std::fs::set_permissions(&public_root, std::fs::Permissions::from_mode(0o755)).unwrap();
    let rejected = Command::new(BINARY)
        .args(["init", public_root.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(!public_root.join("identity").exists());
    let roots = enrollment(directory.path());
    let alias = directory.path().join("alias");
    symlink(&roots[0], &alias).unwrap();
    assert!(!Command::new(BINARY)
        .args(["init", alias.to_str().unwrap()])
        .output()
        .unwrap()
        .status
        .success());
    let fixture_path = roots[0].join("group-fixture.json");
    let moved_fixture = roots[0].join("saved-fixture.json");
    std::fs::rename(&fixture_path, &moved_fixture).unwrap();
    symlink(&moved_fixture, &fixture_path).unwrap();
    let rejected = Command::new(BINARY)
        .args(["run", roots[0].to_str().unwrap(), "--listen", "127.0.0.1:0"])
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(!roots[0].join("application.redb").exists());
    std::fs::remove_file(&fixture_path).unwrap();
    std::fs::rename(moved_fixture, &fixture_path).unwrap();
    std::fs::set_permissions(&fixture_path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(!Command::new(BINARY)
        .args(["run", roots[0].to_str().unwrap(), "--listen", "127.0.0.1:0"])
        .output()
        .unwrap()
        .status
        .success());
    std::fs::set_permissions(
        roots[1].join("identity"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(!Command::new(BINARY)
        .args(["init", roots[1].to_str().unwrap()])
        .output()
        .unwrap()
        .status
        .success());
}
