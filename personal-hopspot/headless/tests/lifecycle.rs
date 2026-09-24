use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

const HOST: &str = env!("CARGO_BIN_EXE_personal-hopspot-headless");
const TEST_TIMEOUT: Duration = Duration::from_secs(15);

fn command(state: &Path) -> Command {
    let mut command = Command::new(HOST);
    command
        .arg("--state-dir")
        .arg(state)
        .args(["--listen", "127.0.0.1:0", "--run-for", "2"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

async fn start(state: &Path) -> (Child, String) {
    let mut child = command(state).spawn().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut ready = String::new();
    tokio::time::timeout(TEST_TIMEOUT, output.read_line(&mut ready))
        .await
        .unwrap()
        .unwrap();
    child.stdout = Some(output.into_inner());
    if !ready.starts_with("hopspot_ready ") {
        let output = tokio::time::timeout(TEST_TIMEOUT, child.wait_with_output())
            .await
            .unwrap()
            .unwrap();
        panic!(
            "host never became ready: {ready:?}; {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let destination = ready.split("node_page=").nth(1).unwrap().trim().to_owned();
    (child, destination)
}

#[tokio::test]
async fn restart_preserves_identity_and_concurrent_writers_are_refused() {
    let directory = tempfile::tempdir().unwrap();
    let (first, first_destination) = start(directory.path()).await;
    let refused = tokio::time::timeout(TEST_TIMEOUT, command(directory.path()).output())
        .await
        .unwrap()
        .unwrap();
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("already in use"));
    let first = tokio::time::timeout(TEST_TIMEOUT, first.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(first.status.success(), "{:?}", first.stderr);
    assert!(String::from_utf8_lossy(&first.stderr).contains("PersistenceFlushed"));
    let identity = std::fs::read(directory.path().join("transport_identity")).unwrap();
    let (second, second_destination) = start(directory.path()).await;
    let second = tokio::time::timeout(TEST_TIMEOUT, second.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(second.status.success(), "{:?}", second.stderr);
    assert_eq!(first_destination, second_destination);
    assert_eq!(
        identity,
        std::fs::read(directory.path().join("transport_identity")).unwrap()
    );
}

#[tokio::test]
async fn malformed_identity_is_preserved_and_startup_fails() {
    let directory = tempfile::tempdir().unwrap();
    let identity = directory.path().join("transport_identity");
    std::fs::write(&identity, b"damaged").unwrap();
    let output = tokio::time::timeout(TEST_TIMEOUT, command(directory.path()).output())
        .await
        .unwrap()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("identity was refused"));
    assert_eq!(std::fs::read(identity).unwrap(), b"damaged");
}

#[tokio::test]
async fn remote_control_identity_survives_restart_and_defaults_to_no_controllers() {
    let directory = tempfile::tempdir().unwrap();
    let mut previous = None;
    for _ in 0..2 {
        let output = tokio::time::timeout(TEST_TIMEOUT, command(directory.path()).output())
            .await
            .unwrap()
            .unwrap();
        assert!(output.status.success(), "{:?}", output.stderr);
        let stdout = String::from_utf8(output.stdout).unwrap();
        let control = stdout
            .lines()
            .find(|line| line.starts_with("hopspot_remote_control_ready "))
            .expect("public control bootstrap data")
            .to_owned();
        assert!(control.ends_with("controllers=0"));
        if let Some(previous) = previous {
            assert_eq!(control, previous);
        }
        previous = Some(control);
    }
}
