mod build_support;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use build_support::{generate_board_catalog, generate_board_images};
use prns_flash_manifest::{
    board_catalog, minisign_public_key_id, ManifestTargetSetPolicy, Sha256Digest,
};

const REPO_VERSION_PATH: &str = "../../VERSION";
const SOURCE_ARCHIVE_ENV: &str = "PRNS_SOURCE_ARCHIVE";
const API_DOCS_ENV: &str = "PRNS_API_DOCS_STAGED";
const LOCAL_DEV_PUBLIC_KEY_ENV: &str = "PRNS_LOCAL_DEV_PUBLIC_KEY";
const LOCAL_DEV_BOARDS_ENV: &str = "PRNS_LOCAL_DEV_BOARDS";
const LOCAL_DEV_SOURCE_DIGEST_ENV: &str = "PRNS_LOCAL_DEV_SOURCE_DIGEST";
const LOCAL_DEV_SOURCE_STATE_ENV: &str = "PRNS_LOCAL_DEV_SOURCE_STATE";
const LOCAL_DEV_PUBLIC_KEY_PATH_ENV: &str = "PRNS_LOCAL_DEV_PUBLIC_KEY_PATH";

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
    generate_board_images();
    generate_board_catalog();
    configure_local_development(&version, &commit);

    println!("cargo:rustc-env=PRNS_BUILD_VERSION={version}");
    println!("cargo:rustc-env=PRNS_GIT_COMMIT={commit}");
    println!("cargo:rustc-env=PRNS_GIT_COMMIT_SHORT={short}");
    println!("cargo:rustc-env=PRNS_BUILD_CHANNEL={channel}");
    println!(
        "cargo:rustc-env=PRNS_SOURCE_ARCHIVE_AVAILABLE={}",
        staged_source_available()
    );
    println!(
        "cargo:rustc-env=PRNS_API_DOCS_AVAILABLE={}",
        api_docs_staged()
    );
    println!("cargo:rerun-if-env-changed=PRNS_BUILD_VERSION");
    println!("cargo:rerun-if-env-changed=PRNS_BUILD_COMMIT");
    println!("cargo:rerun-if-env-changed=PRNS_BUILD_COMMIT_SHORT");
    println!("cargo:rerun-if-env-changed=PRNS_BUILD_CHANNEL");
    println!("cargo:rerun-if-env-changed={SOURCE_ARCHIVE_ENV}");
    println!("cargo:rerun-if-env-changed={API_DOCS_ENV}");
    println!("cargo:rerun-if-env-changed={LOCAL_DEV_PUBLIC_KEY_ENV}");
    println!("cargo:rerun-if-env-changed={LOCAL_DEV_BOARDS_ENV}");
    println!("cargo:rerun-if-env-changed={LOCAL_DEV_SOURCE_DIGEST_ENV}");
    println!("cargo:rerun-if-env-changed={LOCAL_DEV_SOURCE_STATE_ENV}");

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

fn configure_local_development(version: &str, commit: &str) {
    let enabled = env::var_os("CARGO_FEATURE_LOCAL_DEV_FLASHER").is_some();
    let inputs = [
        LOCAL_DEV_PUBLIC_KEY_ENV,
        LOCAL_DEV_BOARDS_ENV,
        LOCAL_DEV_SOURCE_DIGEST_ENV,
        LOCAL_DEV_SOURCE_STATE_ENV,
    ];
    if !enabled {
        if let Some(name) = inputs.iter().find(|name| env::var_os(name).is_some()) {
            panic!("{name} is forbidden without the local-dev-flasher feature");
        }
        return;
    }
    if env::var_os("CARGO_FEATURE_BROWSER_TEST_FIXTURE").is_some() {
        panic!("local-dev-flasher is mutually exclusive with every other website profile");
    }
    let key_path = required_environment_path(LOCAL_DEV_PUBLIC_KEY_ENV);
    let public_key = fs::read_to_string(&key_path)
        .unwrap_or_else(|error| panic!("could not read {}: {error}", key_path.display()));
    if minisign_public_key_id(&public_key).is_none() {
        panic!("{LOCAL_DEV_PUBLIC_KEY_ENV} is not a canonical Minisign public key");
    }
    let boards = required_environment(LOCAL_DEV_BOARDS_ENV);
    let board_slugs = boards.split(',').collect::<Vec<_>>();
    let catalog =
        board_catalog().unwrap_or_else(|error| panic!("shared board catalog is invalid: {error}"));
    let policy = ManifestTargetSetPolicy::local_development(&catalog, &board_slugs)
        .unwrap_or_else(|error| panic!("{LOCAL_DEV_BOARDS_ENV} is invalid: {error}"));
    let selected = catalog
        .boards
        .iter()
        .filter(|board| {
            policy
                .expected_board_slugs()
                .any(|slug| slug == board.slug.as_str())
        })
        .map(|board| board.slug.as_str())
        .collect::<Vec<_>>()
        .join(",");
    if boards != selected {
        panic!("{LOCAL_DEV_BOARDS_ENV} must use canonical catalog order");
    }
    let digest = required_environment(LOCAL_DEV_SOURCE_DIGEST_ENV);
    Sha256Digest::parse(digest.clone())
        .unwrap_or_else(|error| panic!("{LOCAL_DEV_SOURCE_DIGEST_ENV} is invalid: {error}"));
    let state = required_environment(LOCAL_DEV_SOURCE_STATE_ENV);
    if !matches!(state.as_str(), "clean" | "dirty") {
        panic!("{LOCAL_DEV_SOURCE_STATE_ENV} must be clean or dirty");
    }
    if commit.len() != 40
        || !commit
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        panic!("local-dev-flasher requires a lowercase full HEAD commit");
    }
    let base = read_repo_version().expect("local-dev-flasher requires repository VERSION");
    let expected_version = format!("{base}-dev.{state}.{digest}");
    if version != expected_version {
        panic!("PRNS_BUILD_VERSION must equal {expected_version}");
    }
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    let embedded_key = out_dir.join("local-dev-minisign.pub");
    fs::write(&embedded_key, public_key)
        .unwrap_or_else(|error| panic!("could not stage ephemeral public key: {error}"));
    println!(
        "cargo:rustc-env={LOCAL_DEV_PUBLIC_KEY_PATH_ENV}={}",
        embedded_key.display()
    );
    println!("cargo:rustc-env={LOCAL_DEV_BOARDS_ENV}={boards}");
    println!("cargo:rustc-env={LOCAL_DEV_SOURCE_DIGEST_ENV}={digest}");
    println!("cargo:rustc-env={LOCAL_DEV_SOURCE_STATE_ENV}={state}");
    println!("cargo:rerun-if-changed={}", key_path.display());
}

fn required_environment(name: &str) -> String {
    env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| panic!("{name} is required for local-dev-flasher"))
}

fn required_environment_path(name: &str) -> PathBuf {
    PathBuf::from(required_environment(name))
}

fn api_docs_staged() -> bool {
    env::var(API_DOCS_ENV).is_ok_and(|value| value == "1")
}

fn staged_source_available() -> bool {
    let Some(archive) = env::var_os(SOURCE_ARCHIVE_ENV) else {
        return false;
    };
    let archive = PathBuf::from(archive);
    let mut checksum = archive.as_os_str().to_os_string();
    checksum.push(".sha256");
    archive.is_file() && Path::new(&checksum).is_file()
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

fn git_output(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    Some(value.trim().to_owned())
}
