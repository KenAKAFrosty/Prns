use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use super::{git_output, replace_if_changed, sha256_file, write_if_changed};

pub(crate) fn generate(build_version: &str, build_commit: &str) {
    println!("cargo:rerun-if-env-changed=PRNS_SOURCE_ARCHIVE_REF");

    let archive_ref = env::var("PRNS_SOURCE_ARCHIVE_REF")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            if build_commit == "unknown" {
                "HEAD".to_string()
            } else {
                build_commit.to_string()
            }
        });
    let output = PathBuf::from("public").join("source.zip");
    let checksum = PathBuf::from("public").join("source.zip.sha256");
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).expect("failed to create public source archive directory");
    }

    let temp = output.with_extension("zip.tmp");
    let temp_for_git = env::current_dir()
        .unwrap_or_else(|err| panic!("failed to read current directory: {err}"))
        .join(&temp);
    let _ = fs::remove_file(&temp);
    let repo_root =
        git_output(&["rev-parse", "--show-toplevel"]).unwrap_or_else(|| ".".to_string());
    let prefix = format!("Prns-{}/", archive_version(build_version));
    let status = Command::new("git")
        .arg("-C")
        .arg(&repo_root)
        .arg("archive")
        .arg("--format=zip")
        .arg(format!("--prefix={prefix}"))
        .arg("-o")
        .arg(&temp_for_git)
        .arg(&archive_ref)
        .status()
        .unwrap_or_else(|err| {
            panic!("failed to run git archive for source ZIP from {archive_ref}: {err}")
        });
    if !status.success() {
        panic!("git archive failed for source ZIP from {archive_ref} with status {status}");
    }

    replace_if_changed(&output, &temp);
    let hash = sha256_file(&output);
    write_if_changed(&checksum, &format!("{hash}  source.zip\n"));
}

fn archive_version(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
        .collect();
    if sanitized.is_empty() {
        "source".to_string()
    } else {
        sanitized
    }
}
