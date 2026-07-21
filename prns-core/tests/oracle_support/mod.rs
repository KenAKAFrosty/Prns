use std::ffi::OsString;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

const REQUIRED_ENVIRONMENT: &str = "PRNS_ORACLE_REQUIRED";

pub fn reference_python(environment: &str, fallback: &str) -> Option<OsString> {
    if let Some(interpreter) = std::env::var_os(environment) {
        assert!(
            !interpreter.is_empty(),
            "{environment} must name a Python interpreter"
        );
        return Some(interpreter);
    }
    let fallback = Path::new(env!("CARGO_MANIFEST_DIR")).join(fallback);
    if fallback.is_file() {
        return Some(fallback.into_os_string());
    }
    assert!(
        std::env::var_os(REQUIRED_ENVIRONMENT).is_none(),
        "{environment} is required for this oracle lane and the developer fallback is missing at {}",
        fallback.display()
    );
    None
}

pub fn run_json_oracle(
    python: &std::ffi::OsStr,
    script: &Path,
    input: &serde_json::Value,
) -> serde_json::Value {
    let mut child = Command::new(python)
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn Python oracle");
    child
        .stdin
        .take()
        .expect("oracle stdin")
        .write_all(
            serde_json::to_vec(input)
                .expect("oracle input serializes")
                .as_slice(),
        )
        .expect("write oracle input");
    let output = child.wait_with_output().expect("Python oracle runs");
    assert!(
        output.status.success(),
        "Python oracle failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("Python oracle emits JSON")
}
