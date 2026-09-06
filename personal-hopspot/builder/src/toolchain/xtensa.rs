use std::collections::HashSet;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::BuildError;

struct XtensaToolchainEnv {
    path: OsString,
    libclang_path: Option<OsString>,
}

pub fn configure_xtensa_toolchain(command: &mut Command) -> Result<PathBuf, BuildError> {
    let env = xtensa_toolchain_env()?;
    let linker = find_on_path("xtensa-esp32s3-elf-gcc", &env.path).ok_or_else(|| {
        BuildError::Toolchain(
            "xtensa-esp32s3-elf-gcc was not found; install the Xtensa Rust toolchain or update export-esp.sh"
                .to_string(),
        )
    })?;

    command.env("PATH", &env.path);
    if let Some(libclang_path) = env.libclang_path {
        command.env("LIBCLANG_PATH", libclang_path);
    }
    Ok(linker)
}

fn xtensa_toolchain_env() -> Result<XtensaToolchainEnv, BuildError> {
    let mut path_entries = Vec::new();
    let mut libclang_path = env::var_os("LIBCLANG_PATH");

    if let Some(home) = home_dir() {
        let export_path = home.join("export-esp.sh");
        if let Ok(contents) = fs::read_to_string(&export_path) {
            for line in contents.lines() {
                if let Some(value) = parse_assignment_value(line, "PATH") {
                    for part in value.split(':') {
                        if part == "$PATH" || part == "${PATH}" || part.is_empty() {
                            continue;
                        }
                        path_entries.push(expand_export_path(part, &home));
                    }
                } else if let Some(value) = parse_assignment_value(line, "LIBCLANG_PATH") {
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
    let path = env::join_paths(path_entries).map_err(|error| {
        BuildError::Toolchain(format!("failed to build Xtensa toolchain PATH: {error}"))
    })?;

    Ok(XtensaToolchainEnv {
        path,
        libclang_path,
    })
}

fn parse_assignment_value(line: &str, key: &str) -> Option<String> {
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
    env::split_paths(path).find_map(|directory| {
        let candidate = directory.join(binary);
        if candidate.is_file() {
            return Some(candidate);
        }
        if !env::consts::EXE_SUFFIX.is_empty() {
            let candidate = directory.join(format!("{binary}{}", env::consts::EXE_SUFFIX));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assignment_values_accept_export_and_quotes() {
        assert_eq!(
            parse_assignment_value("export PATH=\"/tools/bin:$PATH\"", "PATH").as_deref(),
            Some("/tools/bin:$PATH")
        );
        assert_eq!(
            parse_assignment_value("LIBCLANG_PATH='/llvm/lib'", "LIBCLANG_PATH").as_deref(),
            Some("/llvm/lib")
        );
        assert_eq!(parse_assignment_value("OTHER=value", "PATH"), None);
    }
}
