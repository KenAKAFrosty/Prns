mod build_support;

use std::env;
use std::fs;
use std::path::PathBuf;

use build_support::{
    generate_board_catalog, generate_board_images, generate_source_archive, git_output,
};

const REPO_VERSION_PATH: &str = "../../VERSION";
const WRITE_PUBLIC_ASSETS_ENV: &str = "PRNS_WRITE_PUBLIC_ASSETS";
const EMBEDDED_SITE_ENV: &str = "PRNS_EMBEDDED_SITE";

fn main() {
    let version = build_version();
    let commit = env::var("PRNS_BUILD_COMMIT")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| git_output(&["rev-parse", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_string());
    let short = env::var("PRNS_BUILD_COMMIT_SHORT")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| short_commit(&commit));
    let channel = env::var("PRNS_BUILD_CHANNEL").unwrap_or_else(|_| "stable".to_string());
    assert!(
        matches!(channel.as_str(), "stable" | "preview"),
        "PRNS_BUILD_CHANNEL must be stable or preview"
    );
    let write_public_assets = should_write_public_assets();

    generate_board_images();
    generate_board_catalog();
    if write_public_assets {
        generate_source_archive(&version, &commit);
    }

    println!("cargo:rustc-env=PRNS_BUILD_VERSION={version}");
    println!("cargo:rustc-env=PRNS_GIT_COMMIT={commit}");
    println!("cargo:rustc-env=PRNS_GIT_COMMIT_SHORT={short}");
    println!("cargo:rustc-env=PRNS_BUILD_CHANNEL={channel}");
    println!("cargo:rerun-if-env-changed=PRNS_BUILD_VERSION");
    println!("cargo:rerun-if-env-changed=PRNS_BUILD_COMMIT");
    println!("cargo:rerun-if-env-changed=PRNS_BUILD_COMMIT_SHORT");
    println!("cargo:rerun-if-env-changed=PRNS_BUILD_CHANNEL");
    println!("cargo:rerun-if-env-changed={EMBEDDED_SITE_ENV}");
    println!("cargo:rerun-if-env-changed={WRITE_PUBLIC_ASSETS_ENV}");

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

fn should_write_public_assets() -> bool {
    env_flag(WRITE_PUBLIC_ASSETS_ENV)
}

fn env_flag(name: &str) -> bool {
    env::var_os(name).is_some_and(|value| !value.is_empty() && value != "0")
}

fn build_version() -> String {
    env::var("PRNS_BUILD_VERSION")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(read_repo_version)
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string())
}

fn read_repo_version() -> Option<String> {
    let path = PathBuf::from(REPO_VERSION_PATH);
    println!("cargo:rerun-if-changed={}", path.display());
    fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn short_commit(commit: &str) -> String {
    commit.chars().take(12).collect()
}
