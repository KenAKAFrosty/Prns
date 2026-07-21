use std::path::{Path, PathBuf};
use std::process::Command;

mod support;

fn oracle_script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/oracle/prns_interface_skip_oracle.py")
}

#[test]
fn stock_rns_1_4_0_skips_prns_owned_types_and_aliases() {
    let Some(python) =
        support::reference_python("SMOKE_PYTHON", "../benchmarks/reference/.venv/bin/python")
    else {
        return;
    };

    let output = Command::new(python)
        .arg(oracle_script())
        .output()
        .expect("spawn RNS 1.4.0 Prns interface oracle");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let logs = format!("{stdout}\n{stderr}");

    assert!(output.status.success(), "stock RNS oracle failed:\n{logs}");
    assert!(
        logs.contains("Could not locate external interface module"),
        "stock RNS did not report the missing external module:\n{logs}"
    );
    assert!(
        logs.contains("System interfaces are ready"),
        "stock RNS did not finish interface startup:\n{logs}"
    );

    let result = logs
        .lines()
        .find_map(|line| line.strip_prefix("PRNS_STOCK_SKIP_RESULT="))
        .expect("oracle emits its result marker");
    let result: serde_json::Value = serde_json::from_str(result).expect("oracle emits JSON");
    assert_eq!(result["version"], "1.4.0");
    assert_eq!(result["discovery_default_stamp_cost"], 16);
    assert_eq!(result["registered"], serde_json::json!([]));
    assert_eq!(
        result["configured"],
        serde_json::json!([
            "PrnsUsbAuto",
            "PrnsUsbAutoInterface",
            "PrnsBluetoothAuto",
            "PrnsBluetoothAutoInterface",
            "PrnsBleAuto",
            "PrnsBleAutoInterface",
            "PrnsWebSocketClient",
            "PrnsWebSocketClientInterface",
            "PrnsWebSocketServer",
            "PrnsWebSocketServerInterface"
        ])
    );
}
