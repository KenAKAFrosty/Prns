use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(name: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "prnsd-interfaces-{name}-{}-{nanos}",
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

fn run(directory: &TestDirectory, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_prnsd"))
        .arg("interfaces")
        .args(args)
        .arg("--config")
        .arg(directory.path())
        .output()
        .unwrap_or_else(|error| panic!("{error}"))
}

#[test]
fn scripted_add_emits_a_canonical_stanza_and_preserves_the_source() {
    let directory = TestDirectory::new("add");
    let source = "# retained\n[reticulum]\n  share_instance = No\n[interfaces]\n";
    fs::write(directory.path().join("config"), source).unwrap_or_else(|error| panic!("{error}"));

    let output = run(
        &directory,
        &["add", "BLE", "--name", "Nearby", "--network-name", "mesh"],
    );

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let candidate = fs::read_to_string(directory.path().join("config"))
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(candidate.starts_with(source));
    assert!(candidate.contains("type = PrnsBluetoothAuto"));
    assert!(candidate.contains("interface_enabled = Yes"));
    assert_eq!(
        fs::read_to_string(directory.path().join("config.prns-backup"))
            .unwrap_or_else(|error| panic!("{error}")),
        source
    );
}

#[test]
fn dry_runs_redact_secrets_and_leave_the_file_untouched() {
    let directory = TestDirectory::new("redaction");
    let source = "[reticulum]\n  share_instance = No\n[interfaces]\n";
    fs::write(directory.path().join("config"), source).unwrap_or_else(|error| panic!("{error}"));

    let output = run(
        &directory,
        &[
            "add",
            "auto-wifi",
            "--name",
            "WiFi",
            "--pass-phrase",
            "private-value",
            "--dry-run",
        ],
    );
    let rendered = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success());
    assert!(rendered.contains("<redacted>"));
    assert!(!rendered.contains("private-value"));
    assert_eq!(
        fs::read_to_string(directory.path().join("config"))
            .unwrap_or_else(|error| panic!("{error}")),
        source
    );
}

#[test]
fn diagnostics_redact_secrets_unless_explicitly_revealed() {
    let directory = TestDirectory::new("diagnostic-redaction");
    fs::write(
        directory.path().join("config"),
        "[interfaces]\n[[WiFi]]\ntype = AutoInterface\ninterface_enabled = Yes\npass_phrase = first-private\npassphrase = second-private\n",
    )
    .unwrap_or_else(|error| panic!("{error}"));

    let redacted = run(&directory, &["check"]);
    let redacted_error = String::from_utf8_lossy(&redacted.stderr);
    assert_eq!(redacted.status.code(), Some(1));
    assert!(redacted_error.contains("<redacted>"));
    assert!(!redacted_error.contains("first-private"));
    assert!(!redacted_error.contains("second-private"));

    let revealed = run(&directory, &["--show-secrets", "check"]);
    let revealed_error = String::from_utf8_lossy(&revealed.stderr);
    assert_eq!(revealed.status.code(), Some(1));
    assert!(revealed_error.contains("first-private"));
    assert!(revealed_error.contains("second-private"));
}

#[test]
fn typed_options_reject_inapplicable_interface_settings() {
    let directory = TestDirectory::new("inapplicable");
    let output = run(
        &directory,
        &[
            "add",
            "usb-auto",
            "--name",
            "USB",
            "--listen-port",
            "4242",
            "--dry-run",
        ],
    );

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("does not apply"));
    assert!(!directory.path().join("config").exists());
}

#[test]
fn rnode_multi_add_requires_and_emits_typed_radio_members() {
    let directory = TestDirectory::new("rnode-multi");
    let output = run(
        &directory,
        &[
            "add",
            "rnode-multi",
            "--name",
            "Multi",
            "--port",
            "/dev/ttyACM0",
            "--radio",
            "Primary:0:868000000:125000:7:8:5",
            "--dry-run",
        ],
    );
    let rendered = String::from_utf8_lossy(&output.stdout);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(rendered.contains("type = RNodeMultiInterface"));
    assert!(rendered.contains("[[[Primary]]]"));
    assert!(rendered.contains("vport = 0"));
    assert!(!directory.path().join("config").exists());
}

#[test]
fn removal_requires_confirmation_without_a_terminal() {
    let directory = TestDirectory::new("remove");
    let source = "[reticulum]\n  share_instance = No\n[interfaces]\n[[USB]]\ntype = PrnsUsbAuto\ninterface_enabled = Yes\n";
    fs::write(directory.path().join("config"), source).unwrap_or_else(|error| panic!("{error}"));

    let output = run(&directory, &["remove", "USB"]);

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        fs::read_to_string(directory.path().join("config"))
            .unwrap_or_else(|error| panic!("{error}")),
        source
    );
}

#[test]
fn scripted_commands_cover_the_complete_editing_lifecycle() {
    let directory = TestDirectory::new("lifecycle");
    fs::write(
        directory.path().join("config"),
        "[reticulum]\nshare_instance = No\n[interfaces]\n[[WiFi]]\ntype = AutoInterface\ninterface_enabled = Yes\nmode = full\ninterface_mode = full\n",
    )
    .unwrap_or_else(|error| panic!("{error}"));

    let listed = run(&directory, &["list"]);
    assert!(listed.status.success());
    assert!(String::from_utf8_lossy(&listed.stdout).contains("WiFi: AutoInterface (enabled)"));

    let repaired = run(&directory, &["repair", "--safe"]);
    assert!(
        repaired.status.success(),
        "{}",
        String::from_utf8_lossy(&repaired.stderr)
    );
    let after_repair = fs::read_to_string(directory.path().join("config"))
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(after_repair.contains("interface_mode = full"));
    assert!(!after_repair
        .lines()
        .any(|line| line.trim_start().starts_with("mode =")));

    let disabled = run(&directory, &["disable", "WiFi"]);
    assert!(
        disabled.status.success(),
        "{}",
        String::from_utf8_lossy(&disabled.stderr)
    );
    let enabled = run(&directory, &["enable", "WiFi"]);
    assert!(
        enabled.status.success(),
        "{}",
        String::from_utf8_lossy(&enabled.stderr)
    );

    let edited = run(
        &directory,
        &["edit", "WiFi", "--rename", "Mesh", "--group-id", "prns"],
    );
    assert!(
        edited.status.success(),
        "{}",
        String::from_utf8_lossy(&edited.stderr)
    );

    let checked = run(&directory, &["check"]);
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
    let changed = fs::read_to_string(directory.path().join("config"))
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(changed.contains("[[Mesh]]"));
    assert!(changed.contains("group_id = prns"));
    assert!(changed.contains("interface_enabled = Yes"));

    let disabled = run(&directory, &["disable", "Mesh"]);
    assert!(
        disabled.status.success(),
        "{}",
        String::from_utf8_lossy(&disabled.stderr)
    );
    let removed = run(&directory, &["remove", "Mesh", "--yes"]);
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    assert!(!fs::read_to_string(directory.path().join("config"))
        .unwrap_or_else(|error| panic!("{error}"))
        .contains("[[Mesh]]"));
}

#[test]
fn apply_without_a_managed_daemon_uses_exit_status_three() {
    let directory = TestDirectory::new("apply-stopped");
    fs::write(
        directory.path().join("config"),
        "[reticulum]\n  share_instance = No\n[interfaces]\n",
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let state = TestDirectory::new("state");

    let output = Command::new(env!("CARGO_BIN_EXE_prnsd"))
        .args(["interfaces", "apply", "--config"])
        .arg(directory.path())
        .env("PRNSD_STATE_DIR", state.path())
        .output()
        .unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(output.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&output.stderr).contains("no managed daemon is running"));
}
