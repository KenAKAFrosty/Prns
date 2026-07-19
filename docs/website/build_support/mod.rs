mod board_images;
mod flash_manifest;
mod source_archive;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) use board_images::generate as generate_board_images;
pub(crate) use flash_manifest::generate as generate_flash_manifest;
pub(crate) use source_archive::generate as generate_source_archive;

fn sha256_file(path: &Path) -> String {
    if let Some(hash) = sha256_with("shasum", path) {
        return hash;
    }
    if let Some(hash) = sha256_with("sha256sum", path) {
        return hash;
    }
    panic!(
        "failed to compute sha256 for {}; install shasum or sha256sum",
        path.display()
    );
}

fn sha256_with(program: &str, path: &Path) -> Option<String> {
    let mut command = Command::new(program);
    if program == "shasum" {
        command.arg("-a").arg("256");
    }
    let output = command.arg(path).output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .and_then(|stdout| stdout.split_whitespace().next().map(str::to_string))
}

fn write_if_changed(path: &PathBuf, contents: &str) {
    if fs::read_to_string(path).ok().as_deref() == Some(contents) {
        return;
    }
    fs::write(path, contents).unwrap_or_else(|err| {
        panic!("failed to write {}: {err}", path.display());
    });
}

fn replace_if_changed(path: &Path, temp: &Path) {
    let same = fs::read(path)
        .ok()
        .zip(fs::read(temp).ok())
        .is_some_and(|(current, next)| current == next);
    if same {
        let _ = fs::remove_file(temp);
        return;
    }
    fs::rename(temp, path).unwrap_or_else(|err| {
        panic!(
            "failed to replace {} with {}: {err}",
            path.display(),
            temp.display()
        );
    });
}

pub(crate) fn git_output(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    Some(value.trim().to_owned())
}
