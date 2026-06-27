use std::env;
use std::fs;
use std::process::Command;

fn main() {
    let commit = env::var("PRNS_BUILD_COMMIT")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| git_output(&["rev-parse", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_string());
    let short = env::var("PRNS_BUILD_COMMIT_SHORT")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| short_commit(&commit));

    println!("cargo:rustc-env=PRNS_GIT_COMMIT={commit}");
    println!("cargo:rustc-env=PRNS_GIT_COMMIT_SHORT={short}");
    println!("cargo:rerun-if-env-changed=PRNS_BUILD_COMMIT");
    println!("cargo:rerun-if-env-changed=PRNS_BUILD_COMMIT_SHORT");

    if let Some(head) = git_output(&["rev-parse", "--git-path", "HEAD"]) {
        println!("cargo:rerun-if-changed={head}");
        if let Ok(head_contents) = fs::read_to_string(&head) {
            if let Some(reference) = head_contents.trim().strip_prefix("ref: ") {
                if let Some(path) = git_output(&["rev-parse", "--git-path", reference]) {
                    println!("cargo:rerun-if-changed={path}");
                }
            }
        }
    }
}

fn git_output(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    Some(value.trim().to_owned())
}

fn short_commit(commit: &str) -> String {
    commit.chars().take(12).collect()
}
