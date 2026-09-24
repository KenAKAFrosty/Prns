use super::{
    artifact::{Artifact, Retention},
    *,
};
use std::{
    path::{Path, PathBuf},
    process::Command,
};
const CASE_INPUT: &str = "PRNS_CORE_WORK_CASE";
const OUTPUT: &str = "PRNS_CORE_WORK_OUTPUT";
const WORKER: &str = "core_work::campaign::subprocess::isolated_case";
#[test]
#[ignore = "process-isolated case with explicit input and output"]
fn isolated_case() {
    let case: Case = serde_json::from_slice(
        &std::fs::read(std::env::var_os(CASE_INPUT).expect("case path")).expect("case file"),
    )
    .expect("case schema");
    let output = std::env::var_os(OUTPUT).expect("artifact path");
    let retention = if case.seed == 42 {
        Retention::Full
    } else {
        Retention::CompactPassing
    };
    let runs = [execute::run(&case), execute::run(&case)];
    Artifact::from_runs(case, runs, retention).write(Path::new(&output));
}
fn directory(name: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../validation-artifacts/results/core-work-simulation")
        .join(name);
    std::fs::create_dir_all(&root).expect("artifact root");
    for invocation in 0..10_000 {
        let path = root.join(format!("run-{invocation:04}"));
        match std::fs::create_dir(&path) {
            Ok(()) => return path,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => panic!("artifact directory: {error}"),
        }
    }
    panic!("artifact invocation budget");
}
fn pin(directory: &Path) -> PathBuf {
    let path = directory.join(format!("simulator-worker{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(std::env::current_exe().expect("running executable"), &path)
        .expect("pin simulator");
    path
}
fn spawn(binary: &Path, case: &Case, input: &Path, output: &Path) -> Artifact {
    std::fs::write(
        input,
        serde_json::to_vec_pretty(case).expect("case encoding"),
    )
    .expect("case publication");
    let result = Command::new(binary)
        .args(["--ignored", "--exact", WORKER, "--nocapture"])
        .env(CASE_INPUT, input)
        .env(OUTPUT, output)
        .env("RUST_BACKTRACE", "1")
        .output()
        .expect("worker spawn");
    std::fs::write(output.with_extension("stderr.log"), &result.stderr)
        .expect("retained worker stderr");
    assert!(
        result.status.success(),
        "isolated worker failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    serde_json::from_slice(&std::fs::read(output).expect("worker artifact"))
        .expect("artifact schema")
}
fn reduce(binary: &Path, original: &Artifact, directory: &Path) -> Artifact {
    let mut sequence = 0;
    reduction::reduce(original, |case| {
        sequence += 1;
        spawn(
            binary,
            case,
            &directory.join(format!("candidate-{sequence:04}.json")),
            &directory.join(format!("candidate-{sequence:04}-result.json")),
        )
    })
}
pub fn campaign(cases: Vec<Case>, name: &str) {
    const WORKERS: usize = 4;
    let root = directory(name);
    let binary = pin(&root);
    for (batch, cases) in cases.chunks(WORKERS).enumerate() {
        let results = std::thread::scope(|scope| {
            let handles: Vec<_> = cases
                .iter()
                .enumerate()
                .map(|(offset, case)| {
                    let directory = root.join(format!("case-{:04}", batch * WORKERS + offset));
                    let binary = &binary;
                    scope.spawn(move || {
                        std::fs::create_dir(&directory).expect("case directory");
                        let artifact = spawn(
                            binary,
                            case,
                            &directory.join("case.json"),
                            &directory.join("original.json"),
                        );
                        (directory, artifact)
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().expect("worker join"))
                .collect::<Vec<_>>()
        });
        for (directory, artifact) in results {
            if artifact.passed() {
                continue;
            }
            reduce(&binary, &artifact, &directory).write(&directory.join("reduced.json"));
            panic!("core qualification failed: {:?}; artifact {}; replay with ./tools/prns repo.simulation.core-work.replay --case {}", artifact.failure().expect("failed qualification"), directory.join("original.json").display(), directory.join("case.json").display());
        }
        if (batch + 1).is_multiple_of(8) {
            eprintln!("{name}: {} cases qualified", (batch + 1) * WORKERS);
        }
    }
    eprintln!(
        "qualified {} cases twice; artifacts {}",
        cases.len(),
        root.display()
    );
}
#[test]
#[ignore = "explicit repository replay task"]
fn replay_case() {
    let path = std::env::var_os(CASE_INPUT).expect("case path");
    let case: Case =
        serde_json::from_slice(&std::fs::read(path).expect("case read")).expect("case schema");
    let root = directory("replay");
    let binary = pin(&root);
    let artifact = spawn(
        &binary,
        &case,
        &root.join("case.json"),
        &root.join("original.json"),
    );
    assert!(artifact.passed(), "replay failed at {}", root.display());
}
#[test]
#[ignore = "explicit repository reduction task"]
fn reduce_case() {
    let path = std::env::var_os(CASE_INPUT).expect("artifact path");
    let original: Artifact = serde_json::from_slice(&std::fs::read(path).expect("artifact read"))
        .expect("artifact schema");
    assert_eq!(original.version, case::VERSION);
    let root = directory("reduction");
    let binary = pin(&root);
    original.write(&root.join("original.json"));
    let baseline = spawn(
        &binary,
        &original.case,
        &root.join("case.json"),
        &root.join("baseline.json"),
    );
    assert!(
        reduction::same_failure(original.failure().expect("failed artifact"), &baseline),
        "original failure no longer reproduces; evidence {}",
        root.display()
    );
    reduce(&binary, &original, &root).write(&root.join("reduced.json"));
}
