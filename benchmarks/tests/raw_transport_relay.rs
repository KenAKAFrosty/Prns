use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

fn lines(stdout: ChildStdout) -> Receiver<String> {
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = send.send(line);
        }
    });
    receive
}

fn await_line(receive: &Receiver<String>, prefix: &str, timeout: Duration) -> String {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let line = receive
            .recv_timeout(remaining)
            .unwrap_or_else(|_| panic!("no {prefix} line within {timeout:?}"));
        if line.starts_with(prefix) {
            return line;
        }
    }
}

fn metric(result: &str, key: &str) -> u64 {
    result
        .split_whitespace()
        .find_map(|field| field.strip_prefix(&format!("{key}=")))
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| panic!("line lacks numeric {key}: {result}"))
}

struct TemporaryManifest(PathBuf);

impl Drop for TemporaryManifest {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn integration_manifest(scenario: &str) -> TemporaryManifest {
    let source = benchmarks::scenario_dir(scenario).join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(source).expect("read scenario manifest"))
            .expect("parse scenario manifest");
    // Release smoke owns each manifest's performance window (256 tiny routed
    // frames or 16 transported resource parts per direction). This debug
    // process test isolates setup, bidirectional forwarding, drain, and STOP.
    manifest["profile"]["window"] = 1.into();
    manifest["profile"]["duration_ms"] = 50.into();
    manifest["profile"]["drain_timeout_ms"] = 5_000.into();
    let path = std::env::temp_dir().join(format!(
        "prns-raw-transport-integration-{}.json",
        uuid::Uuid::new_v4()
    ));
    std::fs::write(
        &path,
        serde_json::to_vec(&manifest).expect("serialize integration manifest"),
    )
    .expect("write integration manifest");
    TemporaryManifest(path)
}

#[test]
fn rust_relay_profiles_forward_both_ways_drain_and_stop_cleanly() {
    for (scenario, expected_bitrate, expected_mtu) in [
        ("raw-transport-throughput", 500_000_000, 131_072),
        ("transport-resource-throughput", 500_000_000, 131_072),
        (
            "transport-resource-throughput-unleashed",
            1_000_000_000,
            524_288,
        ),
    ] {
        run_profile(scenario, expected_bitrate, expected_mtu);
    }
}

fn run_profile(scenario: &str, expected_bitrate: u64, expected_mtu: u64) {
    let manifest_file = integration_manifest(scenario);
    let manifest = manifest_file.0.to_str().expect("UTF-8 manifest path");
    let mut relay = Command::new(env!("CARGO_BIN_EXE_participant_node"))
        .args([manifest, "relay", "127.0.0.1:0", "50"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn Rust relay");
    let relay_lines = lines(relay.stdout.take().expect("relay stdout"));
    let ready = await_line(&relay_lines, "READY", Duration::from_secs(10));
    assert_eq!(metric(&ready, "bitrate_bps"), expected_bitrate);
    assert_eq!(metric(&ready, "mtu_bytes"), expected_mtu);
    let addresses = ready
        .split_whitespace()
        .find_map(|field| field.strip_prefix("addr="))
        .expect("relay READY has two addresses");

    let mut driver = Command::new(env!("CARGO_BIN_EXE_raw_transport_driver"))
        .args([manifest, "wire-driver", addresses, "50"])
        .env("BENCHMARK_SMOKE", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn raw transport driver");
    let driver_lines = lines(driver.stdout.take().expect("driver stdout"));
    await_line(&driver_lines, "READY", Duration::from_secs(10));
    await_line(&driver_lines, "HARNESS", Duration::from_secs(10));
    await_line(&driver_lines, "MEASURE_READY", Duration::from_secs(30));
    driver
        .stdin
        .as_mut()
        .expect("driver stdin")
        .write_all(b"START\n")
        .expect("start measurement");
    await_line(&driver_lines, "MEASURE_DONE", Duration::from_secs(45));
    let result = await_line(&driver_lines, "RESULT", Duration::from_secs(10));

    assert!(metric(&result, "sent_a_to_b") > 0, "{result}");
    assert!(metric(&result, "sent_b_to_a") > 0, "{result}");
    assert_eq!(
        metric(&result, "sent"),
        metric(&result, "carried"),
        "{result}"
    );
    assert_eq!(
        metric(&result, "sent_payload_bytes"),
        metric(&result, "carried_payload_bytes")
    );
    for key in [
        "missing",
        "duplicates",
        "corrupt",
        "reordered",
        "unexpected",
        "timed_out_frames",
        "drain_timeouts",
        "outstanding",
    ] {
        assert_eq!(metric(&result, key), 0, "{key}: {result}");
    }
    assert!(driver.wait().expect("wait for driver").success());

    relay
        .stdin
        .as_mut()
        .expect("relay stdin")
        .write_all(b"STOP\n")
        .expect("stop relay");
    assert!(relay.wait().expect("wait for relay").success());
}
