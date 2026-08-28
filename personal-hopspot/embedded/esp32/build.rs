use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../../VERSION");
    println!("cargo:rerun-if-env-changed=PRNS_BUILD_VERSION");
    println!("cargo:rerun-if-env-changed=PRNS_BUILD_SOURCE_DIGEST");
    println!("cargo:rerun-if-env-changed=PRNS_SOURCE_SHA256");
    println!("cargo:rerun-if-env-changed=PRNS_BLE_DISCOVERY_GROUP");
    track_git_head();
    let build_commit_short = git_commit_short();
    let build_version = env::var("PRNS_BUILD_VERSION")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            fs::read_to_string("../../../VERSION")
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| env::var("CARGO_PKG_VERSION").unwrap());
    let build_source_digest = env::var("PRNS_BUILD_SOURCE_DIGEST")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            env::var("PRNS_SOURCE_SHA256")
                .ok()
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| build_commit_short.clone());
    println!("cargo:rustc-env=HOPSPOT_BUILD_COMMIT_SHORT={build_commit_short}");
    println!("cargo:rustc-env=HOPSPOT_BUILD_VERSION={build_version}");
    println!("cargo:rustc-env=HOPSPOT_BUILD_SOURCE_DIGEST={build_source_digest}");
    println!(
        "cargo:rustc-env=HOPSPOT_BUILD_IDENTITY=version={build_version} source={build_source_digest}"
    );
}

fn git_commit_short() -> String {
    env::var("PRNS_BUILD_COMMIT_SHORT")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            env::var("PRNS_BUILD_COMMIT")
                .ok()
                .filter(|value| !value.is_empty())
                .map(|value| value.chars().take(12).collect())
        })
        .or_else(|| git_output(&["rev-parse", "--short=12", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_string())
}

fn track_git_head() {
    let mut paths = Vec::new();
    if let Some(path) = git_path("HEAD") {
        paths.push(path);
    }
    if let Some(reference) = git_output(&["symbolic-ref", "-q", "HEAD"]) {
        if let Some(path) = git_path(&reference) {
            paths.push(path);
        }
        if let Some(path) = git_path("packed-refs") {
            paths.push(path);
        }
    }
    for path in paths {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

fn git_path(name: &str) -> Option<PathBuf> {
    let path = PathBuf::from(git_output(&["rev-parse", "--git-path", name])?);
    if path.is_absolute() {
        return Some(path);
    }
    git_output(&["rev-parse", "--show-toplevel"])
        .map(PathBuf::from)
        .map(|root| root.join(path))
}

fn git_output(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    Some(value.trim().to_owned())
}
