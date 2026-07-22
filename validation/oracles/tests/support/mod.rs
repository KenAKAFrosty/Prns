use std::ffi::OsString;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

pub fn required_python(environment: &str) -> OsString {
    let interpreter = std::env::var_os(environment)
        .unwrap_or_else(|| panic!("{environment} must be set by validation/run.py"));
    assert!(
        !interpreter.is_empty(),
        "{environment} must name a Python interpreter"
    );
    interpreter
}

// Each integration-test target compiles this shared module independently; only
// the wire and identity targets need the JSON subprocess helper.
#[allow(dead_code)]
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
