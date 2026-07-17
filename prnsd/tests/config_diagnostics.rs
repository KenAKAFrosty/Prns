use std::fs;
use std::path::PathBuf;
use std::process::Command;

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
        .output()
        .unwrap();

    assert!(!output.status.success());
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(rendered.matches("\"event\":\"config_invalid\"").count(), 3);
    assert!(rendered.contains(&path.display().to_string()));
    assert!(rendered.contains("[reticulum] > discover_interfaces"));
    assert!(rendered.contains("[interfaces] > [[Hub]] > target_port"));
    assert!(rendered.contains("[interfaces] > [[Hub]] > outgoing"));
    assert!(!rendered.contains("\"event\":\"network_identity_failed\""));
}
