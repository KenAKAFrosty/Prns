use super::*;
use artifact::Artifact;
use std::path::{Path, PathBuf};
use std::process::Command;

const CASE_INPUT: &str = "PRNS_REMOTE_CONTROL_SIM_CASE";
const OUTPUT: &str = "PRNS_REMOTE_CONTROL_SIM_OUTPUT";
const WORKER: &str = "remote_control::campaign::subprocess::isolated_case";

#[test]
#[ignore = "spawned once per case with explicit process-owned inputs"]
fn isolated_case() {
    let input = std::env::var_os(CASE_INPUT).expect("explicit case input path");
    let output = std::env::var_os(OUTPUT).expect("explicit artifact output path");
    let case: Case = serde_json::from_slice(&std::fs::read(input).expect("read case"))
        .expect("parse versioned case");
    let runs = [execute::run(&case), execute::run(&case)];
    Artifact {
        version: case::VERSION,
        case,
        runs,
    }
    .write(Path::new(&output));
}
fn spawn(executable: &Path, case: &Case, input: &Path, output: &Path) -> Artifact {
    std::fs::write(input, serde_json::to_vec_pretty(case).expect("case schema"))
        .expect("write isolated input");
    let result = Command::new(executable)
        .args(["--ignored", "--exact", WORKER, "--nocapture"])
        .env(CASE_INPUT, input)
        .env(OUTPUT, output)
        .env("RUST_BACKTRACE", "1")
        .output()
        .expect("spawn isolated simulator");
    std::fs::write(output.with_extension("stderr.log"), &result.stderr)
        .expect("worker diagnostics");
    assert!(
        result.status.success(),
        "worker exited before recording its result: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    serde_json::from_slice(&std::fs::read(output).expect("worker artifact"))
        .expect("artifact schema")
}
fn artifact_directory(name: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../validation-artifacts/results/remote-control-simulation")
        .join(name);
    std::fs::create_dir_all(&root).expect("qualification artifact directory");
    for invocation in 0..10_000 {
        let directory = root.join(format!("run-{invocation:04}"));
        match std::fs::create_dir(&directory) {
            Ok(()) => return directory,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("artifact directory: {error}"),
        }
    }
    panic!("artifact invocation budget");
}
fn pin_simulator(directory: &Path) -> PathBuf {
    let executable = directory.join(format!("simulator-worker{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(
        std::env::current_exe().expect("simulator binary"),
        &executable,
    )
    .expect("pin campaign simulator");
    executable
}
fn reduce(executable: &Path, original: &Artifact, directory: &Path) -> Artifact {
    reduction::reduce(original, |case| {
        spawn(
            executable,
            case,
            &directory.join("candidate.json"),
            &directory.join("candidate-result.json"),
        )
    })
}
pub fn campaign(cases: Vec<Case>, name: &str) {
    let root = artifact_directory(name);
    let executable = pin_simulator(&root);
    const ISOLATED_WORKERS: usize = 4;
    for (batch, cases) in cases.chunks(ISOLATED_WORKERS).enumerate() {
        let results = std::thread::scope(|scope| {
            let workers: Vec<_> = cases
                .iter()
                .enumerate()
                .map(|(offset, case)| {
                    let directory =
                        root.join(format!("case-{:04}", batch * ISOLATED_WORKERS + offset));
                    let executable = &executable;
                    scope.spawn(move || {
                        std::fs::create_dir(&directory).expect("case directory");
                        let artifact = spawn(
                            executable,
                            case,
                            &directory.join("case.json"),
                            &directory.join("original.json"),
                        );
                        (directory, artifact)
                    })
                })
                .collect();
            workers
                .into_iter()
                .map(|worker| worker.join().expect("isolated worker"))
                .collect::<Vec<_>>()
        });
        for (directory, artifact) in results {
            if !artifact.passed() {
                let reduced = reduce(&executable, &artifact, &directory);
                reduced.write(&directory.join("reduced.json"));
                panic!("qualification failed: {:?}; original {}, reduced {}; replay with ./tools/prns repo.simulation.remote-control.replay --case {}", artifact.failure().expect("failed qualification"), directory.join("original.json").display(), directory.join("reduced.json").display(), directory.join("case.json").display());
            }
        }
        if (batch + 1) % 8 == 0 {
            eprintln!("{name}: {} cases qualified", (batch + 1) * ISOLATED_WORKERS);
        }
    }
    eprintln!(
        "qualified {} cases, two fresh fixtures each; artifacts {}",
        cases.len(),
        root.display()
    );
}

#[test]
#[ignore = "explicit single-case replay via repository task"]
fn replay_case() {
    let path = std::env::var_os(CASE_INPUT).expect("explicit replay case");
    let case: Case =
        serde_json::from_slice(&std::fs::read(&path).expect("replay input")).expect("case schema");
    let root = artifact_directory("replay");
    let executable = pin_simulator(&root);
    let artifact = spawn(
        &executable,
        &case,
        &root.join("case.json"),
        &root.join("original.json"),
    );
    assert!(
        artifact.passed(),
        "single-case replay failed; see {}",
        root.join("original.json").display()
    );
}

#[test]
#[ignore = "explicit deterministic reduction via repository task"]
fn reduce_case() {
    let path = std::env::var_os(CASE_INPUT).expect("explicit original artifact");
    let original: Artifact = serde_json::from_slice(&std::fs::read(&path).expect("original input"))
        .expect("artifact schema");
    assert_eq!(original.version, case::VERSION);
    let root = artifact_directory("reduction");
    let executable = pin_simulator(&root);
    original.write(&root.join("original.json"));
    let baseline = spawn(
        &executable,
        &original.case,
        &root.join("case.json"),
        &root.join("baseline.json"),
    );
    assert!(
        reduction::same_failure(&original.failure().expect("failed original"), &baseline),
        "original failure is no longer reproduced; retained original and fresh baseline at {}",
        root.display()
    );
    reduce(&executable, &original, &root).write(&root.join("reduced.json"));
}
