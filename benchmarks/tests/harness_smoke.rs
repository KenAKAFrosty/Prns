use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

#[test]
fn a_short_firehose_run_settles_clean_end_to_end() {
    let manifest = benchmarks::scenario_dir("single-firehose").join("manifest.json");
    let manifest = manifest.to_str().expect("utf8 path");

    let mut responder = Command::new(env!("CARGO_BIN_EXE_scenario_node"))
        .args([manifest, "responder", "127.0.0.1:0", "500"])
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn responder");
    let mut responder_lines = BufReader::new(responder.stdout.take().expect("piped")).lines();
    let ready = responder_lines
        .by_ref()
        .map_while(Result::ok)
        .find(|line| line.starts_with("READY"))
        .expect("responder reports READY");
    let addr = ready
        .split_whitespace()
        .find_map(|kv| kv.strip_prefix("addr="))
        .expect("READY carries the bound addr")
        .to_string();

    let initiator = Command::new(env!("CARGO_BIN_EXE_scenario_node"))
        .args([manifest, "initiator", &addr, "500"])
        .output()
        .expect("run initiator");
    let stdout = String::from_utf8_lossy(&initiator.stdout);
    let result = stdout
        .lines()
        .find(|line| line.starts_with("RESULT"))
        .expect("initiator reports RESULT");

    let field = |key: &str| -> u64 {
        result
            .split_whitespace()
            .find_map(|kv| kv.strip_prefix(&format!("{key}=")))
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| panic!("RESULT carries {key}: {result}"))
    };
    assert!(field("delivered") > 0, "the firehose delivers: {result}");
    assert_eq!(
        field("timeouts"),
        0,
        "a healthy pair settles clean: {result}"
    );
    assert_eq!(
        field("sent"),
        field("delivered"),
        "every send settles delivered: {result}",
    );

    let responder_result = responder_lines
        .map_while(Result::ok)
        .find(|line| line.starts_with("RESULT"))
        .expect("responder reports RESULT");
    let _ = responder.wait();
    assert!(
        responder_result.contains(&format!("delivered={}", field("delivered"))),
        "both ends agree: {responder_result} vs {result}",
    );
}
