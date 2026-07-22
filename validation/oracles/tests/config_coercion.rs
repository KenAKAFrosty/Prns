use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use prns_config::reference;

mod support;

fn oracle_script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("python/coercion_oracle.py")
}

fn run_oracle(python: &std::ffi::OsStr, corpus: &[String]) -> Vec<serde_json::Value> {
    let input = serde_json::to_string(corpus).expect("corpus serializes");
    let mut child = Command::new(python)
        .arg(oracle_script())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn coercion oracle python");
    child
        .stdin
        .take()
        .expect("oracle stdin")
        .write_all(input.as_bytes())
        .expect("write corpus to oracle");
    let output = child.wait_with_output().expect("oracle runs");
    assert!(
        output.status.success(),
        "oracle exited with failure:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("oracle emits json array")
}

fn coerced_bool(scalar: &str) -> Result<bool, ()> {
    let config = format!(
        "[interfaces]\n[[H]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_host = host\ntarget_port = 4242\noutgoing = {scalar}\n"
    );
    match reference::parse(&config) {
        Ok(parsed) => parsed.interfaces.first().and_then(|i| i.outgoing).ok_or(()),
        Err(_) => Err(()),
    }
}

fn coerced_u64(scalar: &str) -> Result<u64, ()> {
    let config = format!(
        "[interfaces]\n[[H]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_host = host\ntarget_port = 4242\nconnect_timeout = {scalar}\n"
    );
    match reference::parse(&config) {
        Ok(parsed) => match &parsed.interfaces.first().ok_or(())?.params {
            reference::ReferenceConfigParams::TcpClient {
                connect_timeout, ..
            } => connect_timeout.ok_or(()),
            _ => Err(()),
        },
        Err(_) => Err(()),
    }
}

fn coerced_f64(scalar: &str) -> Result<f64, ()> {
    let config = format!(
        "[interfaces]\n[[H]]\ntype = AutoInterface\nenabled = Yes\ndiscoverable = Yes\nlatitude = {scalar}\n"
    );
    match reference::parse(&config) {
        Ok(parsed) => parsed
            .interfaces
            .first()
            .and_then(|interface| interface.discovery.latitude)
            .ok_or(()),
        Err(_) => Err(()),
    }
}

fn compare(python: &std::ffi::OsStr, corpus: &[String], seed: Option<u64>) {
    let oracle = run_oracle(python, corpus);
    assert_eq!(oracle.len(), corpus.len(), "one oracle result per scalar");
    for (index, (scalar, verdict)) in corpus.iter().zip(&oracle).enumerate() {
        let provenance = seed.map_or_else(
            || format!("case {index}"),
            |seed| format!("seed {seed:#018x}, case {index}"),
        );
        match (coerced_bool(scalar), &verdict["bool"]) {
            (Ok(ours), serde_json::Value::Bool(python)) => {
                assert_eq!(
                    ours, *python,
                    "bool mismatch at {provenance} for {scalar:?}"
                )
            }
            (Err(()), serde_json::Value::Null) => {}
            (ours, python) => {
                panic!("bool divergence at {provenance} for {scalar:?}: ours={ours:?}, ConfigObj={python}")
            }
        }

        let expected_u64: Result<u64, ()> = match verdict["int"].as_str() {
            Some(decimal) => decimal.parse::<u64>().map_err(|_| ()),
            None => Err(()),
        };
        assert_eq!(
            coerced_u64(scalar),
            expected_u64,
            "u64 mismatch at {provenance} for {scalar:?}: python int = {:?}",
            verdict["int"]
        );

        match (coerced_f64(scalar), verdict["float"].as_str()) {
            (Ok(ours), Some(python_repr)) => {
                let python: f64 = python_repr
                    .parse()
                    .expect("python float repr parses in rust");
                assert!(
                    ours == python || (ours.is_nan() && python.is_nan()),
                    "f64 mismatch at {provenance} for {scalar:?}: ours={ours}, python={python}"
                );
            }
            (Err(()), None) => {}
            (ours, python) => {
                panic!("f64 divergence at {provenance} for {scalar:?}: ours={ours:?}, python={python:?}")
            }
        }
    }
}

const CURATED: &[&str] = &[
    "0",
    "00",
    "007",
    "42",
    "+7",
    "-3",
    "1_000",
    "1_000_000",
    "1__0",
    "_5",
    "5_",
    "1_",
    "999999999999999999999999999999",
    "0x1f",
    "3.5",
    "1 2",
    "",
    "1.5",
    ".5",
    "5.",
    "1e10",
    "1_0.5",
    "1.0e1_0",
    "inf",
    "-inf",
    "nan",
    "Infinity",
    "1.2.3",
    "yes",
    "no",
    "on",
    "off",
    "true",
    "false",
    "YES",
    "Off",
    "True",
    "FALSE",
    "y",
    "n",
    "enabled",
    "disabled",
    "2",
    "-1",
    "18446744073709551615",
    "18446744073709551616",
    "9223372036854775807",
    "9223372036854775808",
    "-9223372036854775808",
    "-9223372036854775809",
    "4.9406564584124654e-324",
    "1.7976931348623157e308",
    "1.7976931348623159e308",
    "-0.0",
    "+0.0",
    " 42 ",
    "\t1\t",
    "1\u{0}2",
    "NaN(payload)",
];

struct Generator {
    state: u64,
}

impl Generator {
    fn new(seed: u64) -> Self {
        Generator { state: seed }
    }

    fn next(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.state >> 33
    }

    fn scalar(&mut self) -> String {
        const POOL: &[u8] = b"0123456789+-._einf";
        let length = 1 + (self.next() % 8) as usize;
        (0..length)
            .map(|_| POOL[(self.next() % POOL.len() as u64) as usize] as char)
            .collect()
    }
}

fn generated_corpus(count: usize, seed: u64) -> Vec<String> {
    let mut generator = Generator::new(seed);
    (0..count)
        .map(|_| generator.scalar())
        .filter(|scalar| !scalar.starts_with(' ') && !scalar.ends_with(' '))
        .collect()
}

#[test]
fn coercions_match_configobj_on_curated_scalars() {
    let python = support::required_python("SMOKE_PYTHON");
    let corpus: Vec<String> = CURATED.iter().map(|scalar| scalar.to_string()).collect();
    compare(&python, &corpus, None);
}

#[test]
fn coercions_match_configobj_on_generated_scalars() {
    let python = support::required_python("SMOKE_PYTHON");
    const SEED: u64 = 0xc0ffee;
    compare(&python, &generated_corpus(400, SEED), Some(SEED));
}
