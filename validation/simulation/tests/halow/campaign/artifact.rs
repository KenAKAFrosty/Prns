use super::*;
use std::{
    fs,
    path::{Path, PathBuf},
};
pub fn directory(family: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../validation-artifacts/results/halow-simulation")
        .join(family);
    fs::create_dir_all(&root).expect("artifact family");
    for index in 0..10_000 {
        let candidate = root.join(format!("run-{index:04}"));
        match fs::create_dir(&candidate) {
            Ok(()) => return candidate,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => panic!("artifact directory: {error}"),
        }
    }
    panic!("artifact run directory budget exhausted");
}
pub fn qualify(directory: &Path, index: usize, case: &Case) {
    let first = execute::run(case);
    let second = execute::run(case);
    let status = if first != second {
        Qualification::ReplayMismatch
    } else if first.outcome != execute::Outcome::Passed {
        Qualification::Failed
    } else {
        Qualification::Passed
    };
    let artifact =
        serde_json::json!({"version":1, "case":case, "status":status, "runs":[first,second]});
    fs::write(
        directory.join(format!("case-{index:04}.json")),
        serde_json::to_vec(&artifact).expect("structured artifact"),
    )
    .expect("retain passing and failing traces");
    assert_eq!(
        status,
        Qualification::Passed,
        "case {case:?}; complete evidence in {}",
        directory.display()
    );
}
#[derive(Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum Qualification {
    Passed,
    Failed,
    ReplayMismatch,
}
