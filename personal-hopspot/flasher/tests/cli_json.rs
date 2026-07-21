use std::error::Error;
use std::io;
use std::process::{Command, Output};

use serde_json::Value;

fn run(arguments: &[&str]) -> io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_hopspot-flash"))
        .args(arguments)
        .output()
}

fn single_json_line(output: &Output) -> Result<Value, Box<dyn Error>> {
    assert_eq!(output.status.code(), Some(2));
    assert!(
        output.stderr.is_empty(),
        "JSON parse failures must not write human diagnostics: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = std::str::from_utf8(&output.stdout)?;
    assert!(
        !stdout.contains('\u{1b}'),
        "JSON output must not contain ANSI"
    );
    let mut lines = stdout.lines();
    let line = lines
        .next()
        .ok_or_else(|| io::Error::other("missing terminal NDJSON event"))?;
    assert!(lines.next().is_none(), "one terminal event: {stdout:?}");
    Ok(serde_json::from_str(line)?)
}

#[test]
fn json_monitor_conflict_is_one_schema_one_usage_event() -> Result<(), Box<dyn Error>> {
    let output = run(&["flash", "heltec-v4", "--yes", "--json", "--monitor"])?;
    let event = single_json_line(&output)?;

    assert_eq!(event["schema"], 1);
    assert_eq!(event["event"], "error");
    assert_eq!(event["phase"], "failed");
    assert_eq!(event["error_code"], "usage");
    Ok(())
}

#[test]
fn json_parse_failure_does_not_echo_unknown_credential_arguments() -> Result<(), Box<dyn Error>> {
    const SECRET: &str = "do-not-echo-this-password";
    let output = run(&[
        "flash",
        "heltec-v4",
        "--yes",
        "--json",
        "--wifi-password",
        SECRET,
    ])?;
    let event = single_json_line(&output)?;

    assert_eq!(event["error_code"], "usage");
    assert!(!String::from_utf8_lossy(&output.stdout).contains(SECRET));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(SECRET));
    Ok(())
}
