#![cfg(all(target_os = "linux", feature = "wifi-halow"))]

use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
};

#[tokio::test]
async fn a_missing_radio_keeps_wired_boot_available_past_the_old_startup_deadline() {
    let state = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_personal-hopspot-headless"))
        .arg("--state-dir")
        .arg(state.path())
        .args([
            "--listen",
            "127.0.0.1:0",
            "--halow-device",
            "prns-absent",
            "--halow-scope",
            "boot-test",
            "--run-for",
            "8",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut ready = String::new();
    tokio::time::timeout(Duration::from_secs(3), output.read_line(&mut ready))
        .await
        .unwrap()
        .unwrap();
    let address = ready
        .strip_prefix("hopspot_ready listen=")
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap();
    let connection = tokio::net::TcpStream::connect(address).await.unwrap();
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert_eq!(child.try_wait().unwrap(), None);
    drop(connection);
    assert!(tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
}
