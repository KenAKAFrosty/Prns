use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

const BUILD_COMMIT_ENV: &str = "PRNS_BUILD_COMMIT";

fn main() {
    let manifest = env::var_os("CARGO_MANIFEST_DIR").expect("Cargo sets CARGO_MANIFEST_DIR");
    let repository = Path::new(&manifest)
        .parent()
        .expect("prnsd lives directly beneath the repository root");
    let commit = env::var(BUILD_COMMIT_ENV)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| git_output(repository, &["rev-parse", "HEAD"]).unwrap_or_default());
    let commit = if commit.is_empty() {
        String::from("unknown")
    } else {
        assert!(
            is_full_commit(&commit),
            "{BUILD_COMMIT_ENV} must be a lowercase full Git commit"
        );
        commit
    };
    let short = if is_full_commit(&commit) {
        &commit[..12]
    } else {
        "development"
    };

    println!("cargo:rustc-env=PRNS_GIT_COMMIT={commit}");
    println!("cargo:rustc-env=PRNS_GIT_COMMIT_SHORT={short}");
    println!("cargo:rerun-if-env-changed={BUILD_COMMIT_ENV}");
    println!("cargo:rerun-if-changed=../VERSION");

    if let Some(head) = git_output(repository, &["rev-parse", "--git-path", "HEAD"]) {
        println!("cargo:rerun-if-changed={head}");
        if let Ok(contents) = fs::read_to_string(&head) {
            if let Some(reference) = contents.trim().strip_prefix("ref: ") {
                if let Some(path) = git_output(repository, &["rev-parse", "--git-path", reference])
                {
                    println!("cargo:rerun-if-changed={path}");
                }
            }
        }
    }
}

fn git_output(repository: &Path, arguments: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(repository)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn is_full_commit(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
