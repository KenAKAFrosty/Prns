use std::path::{Path, PathBuf};
use std::process::Command;

mod support;

fn oracle_script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("python/prns_section_skip_oracle.py")
}

#[test]
fn stock_rns_1_4_2_ignores_and_preserves_the_prns_section() {
    let python = support::required_python("SMOKE_PYTHON");
    let output = Command::new(python)
        .arg(oracle_script())
        .output()
        .expect("spawn RNS 1.4.2 Prns-section oracle");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let logs = format!("{stdout}\n{stderr}");

    assert!(output.status.success(), "stock RNS oracle failed:\n{logs}");
    assert!(
        logs.contains("System interfaces are ready"),
        "stock RNS did not finish startup:\n{logs}"
    );
    let result = logs
        .lines()
        .find_map(|line| line.strip_prefix("PRNS_STOCK_SECTION_RESULT="))
        .expect("oracle emits its result marker");
    let result: serde_json::Value = serde_json::from_str(result).expect("oracle emits JSON");

    assert_eq!(result["version"], "1.4.2");
    assert_eq!(result["config_unchanged"], true);
    assert_eq!(result["registered"], serde_json::json!([]));
    assert_eq!(result["loaded_prns"]["resource_mem_in"], "64 MiB");
    assert_eq!(result["loaded_prns"]["resource_mem_out"], "0");
}
