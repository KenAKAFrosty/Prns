use std::collections::HashSet;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::AppError;

struct EspToolchainEnv {
    path: OsString,
    libclang_path: Option<OsString>,
}

pub(crate) fn configure_esp_toolchain(command: &mut Command) -> Result<PathBuf, AppError> {
    let env = esp_toolchain_env()?;
    let linker = find_on_path("xtensa-esp32s3-elf-gcc", &env.path).ok_or_else(|| {
        AppError::developer_toolchain("xtensa-esp32s3-elf-gcc was not found; install the ESP Rust toolchain or update export-esp.sh")
    })?;

    command.env("PATH", &env.path);
    if let Some(libclang_path) = env.libclang_path {
        command.env("LIBCLANG_PATH", libclang_path);
    }
    Ok(linker)
}

fn esp_toolchain_env() -> Result<EspToolchainEnv, AppError> {
    let mut path_entries = Vec::new();
    let mut libclang_path = env::var_os("LIBCLANG_PATH");

    if let Some(home) = home_dir() {
        let export_path = home.join("export-esp.sh");
        if let Ok(contents) = fs::read_to_string(&export_path) {
            for line in contents.lines() {
                if let Some(value) = parse_export_value(line, "PATH") {
                    for part in value.split(':') {
                        if part == "$PATH" || part == "${PATH}" || part.is_empty() {
                            continue;
                        }
                        path_entries.push(expand_export_path(part, &home));
                    }
                } else if let Some(value) = parse_export_value(line, "LIBCLANG_PATH") {
                    libclang_path = Some(expand_export_path(&value, &home).into_os_string());
                }
            }
        }

        collect_xtensa_toolchain_bins(
            &home
                .join(".rustup")
                .join("toolchains")
                .join("esp")
                .join("xtensa-esp-elf"),
            &mut path_entries,
        );
    }

    if let Some(current_path) = env::var_os("PATH") {
        path_entries.extend(env::split_paths(&current_path));
    }

    let mut seen = HashSet::new();
    path_entries.retain(|path| seen.insert(path.to_string_lossy().into_owned()));
    let path = env::join_paths(path_entries).map_err(|err| {
        AppError::developer_toolchain(format!("failed to build ESP toolchain PATH: {err}"))
    })?;

    Ok(EspToolchainEnv {
        path,
        libclang_path,
    })
}

fn parse_export_value(line: &str, key: &str) -> Option<String> {
    parse_assignment_value(line, key)
}

pub(crate) fn parse_assignment_value(line: &str, key: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
    let (name, value) = line.split_once('=')?;
    if name.trim() != key {
        return None;
    }
    Some(unquote_assignment_value(value.trim()))
}

fn unquote_assignment_value(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        if (bytes[0] == b'"' && bytes[value.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[value.len() - 1] == b'\'')
        {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

fn expand_export_path(value: &str, home: &Path) -> PathBuf {
    let home_string = home.to_string_lossy();
    let expanded = value
        .replace("${HOME}", &home_string)
        .replace("$HOME", &home_string);
    if let Some(rest) = expanded.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(expanded)
    }
}

fn collect_xtensa_toolchain_bins(root: &Path, path_entries: &mut Vec<PathBuf>) {
    let flat_bin = root.join("bin");
    if flat_bin.is_dir() {
        path_entries.push(flat_bin);
    }
    let Ok(releases) = fs::read_dir(root) else {
        return;
    };

    for release in releases.flatten() {
        let bin = release.path().join("xtensa-esp-elf").join("bin");
        if bin.is_dir() {
            path_entries.push(bin);
        }
    }
}

fn find_on_path(binary: &str, path: &OsString) -> Option<PathBuf> {
    env::split_paths(path).find_map(|dir| {
        let candidate = dir.join(binary);
        if candidate.is_file() {
            return Some(candidate);
        }
        if !env::consts::EXE_SUFFIX.is_empty() {
            let candidate = dir.join(format!("{binary}{}", env::consts::EXE_SUFFIX));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        None
    })
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

pub(crate) fn rust_host_triple() -> Result<String, AppError> {
    let version = capture_stdout(Command::new("rustc").arg("-vV"), "rustc -vV")?;
    version
        .lines()
        .find_map(|line| line.strip_prefix("host: ").map(str::to_string))
        .ok_or_else(|| AppError::developer_toolchain("rustc -vV did not report a host triple"))
}

pub(crate) fn run_status(command: &mut Command, label: &str) -> Result<(), AppError> {
    command.stdin(Stdio::inherit());
    command.stdout(Stdio::inherit());
    command.stderr(Stdio::inherit());
    let status = command
        .status()
        .map_err(|err| AppError::developer_toolchain(format!("failed to run {label}: {err}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(AppError::developer_toolchain(format!(
            "{label} exited with {status}"
        )))
    }
}

pub(crate) fn capture_stdout(command: &mut Command, label: &str) -> Result<String, AppError> {
    let output = command
        .output()
        .map_err(|err| AppError::developer_toolchain(format!("failed to run {label}: {err}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::developer_toolchain(format!(
            "{label} exited with {}: {stderr}",
            output.status
        )));
    }
    String::from_utf8(output.stdout).map_err(|err| {
        AppError::developer_toolchain(format!("{label} produced invalid UTF-8 output: {err}"))
    })
}
