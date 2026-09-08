use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::BuildError;

pub(crate) use evidence::capture_toolchain_evidence;

pub fn embedded_cargo_command() -> Command {
    let mut command = Command::new("cargo");
    command
        .env_remove("RUSTUP_TOOLCHAIN")
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_TARGET_DIR");
    command
}

pub fn rust_host_triple() -> Result<String, BuildError> {
    rust_host_triple_for(None)
}

fn rust_host_triple_for(directory: Option<&Path>) -> Result<String, BuildError> {
    let version = capture_stdout(configured_rustc(directory).arg("-vV"), "rustc -vV")?;
    version
        .lines()
        .find_map(|line| line.strip_prefix("host: ").map(str::to_string))
        .ok_or_else(|| BuildError::Toolchain("rustc -vV did not report a host triple".to_string()))
}

pub fn llvm_objcopy() -> Result<PathBuf, BuildError> {
    rust_tool("llvm-objcopy")
}

pub(crate) fn rust_tool(name: &str) -> Result<PathBuf, BuildError> {
    rust_tool_for(None, name)
}

pub(crate) fn rust_tool_for_cargo(cargo: &Command, name: &str) -> Result<PathBuf, BuildError> {
    rust_tool_for(cargo.get_current_dir(), name)
}

fn rust_tool_for(directory: Option<&Path>, name: &str) -> Result<PathBuf, BuildError> {
    let host_triple = rust_host_triple_for(directory)?;
    let sysroot = capture_stdout(
        configured_rustc(directory).arg("--print").arg("sysroot"),
        "rustc",
    )?;
    let path = Path::new(sysroot.trim())
        .join("lib")
        .join("rustlib")
        .join(host_triple.trim())
        .join("bin")
        .join(name);
    if path.is_file() {
        Ok(path)
    } else {
        Err(BuildError::Toolchain(format!(
            "Rust tool {name:?} was not found at {}",
            path.display()
        )))
    }
}

fn configured_rustc(directory: Option<&Path>) -> Command {
    let mut command = Command::new("rustc");
    command.env_remove("RUSTUP_TOOLCHAIN");
    if let Some(directory) = directory {
        command.current_dir(directory);
    }
    command
}

pub fn run_status(command: &mut Command, label: &str) -> Result<(), BuildError> {
    command.stdin(Stdio::inherit());
    command.stdout(Stdio::inherit());
    command.stderr(Stdio::inherit());
    let status = command
        .status()
        .map_err(|error| BuildError::Toolchain(format!("failed to run {label}: {error}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(BuildError::Toolchain(format!(
            "{label} exited with {status}"
        )))
    }
}

pub fn capture_stdout(command: &mut Command, label: &str) -> Result<String, BuildError> {
    let output = command
        .output()
        .map_err(|error| BuildError::Toolchain(format!("failed to run {label}: {error}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(BuildError::Toolchain(format!(
            "{label} exited with {}: {stderr}",
            output.status
        )));
    }
    String::from_utf8(output.stdout).map_err(|error| {
        BuildError::Toolchain(format!("{label} produced invalid UTF-8 output: {error}"))
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::ffi::OsStr;

    use super::*;

    #[test]
    fn embedded_cargo_ignores_inherited_host_configuration() {
        let command = embedded_cargo_command();
        let environments = command.get_envs().collect::<BTreeMap<_, _>>();
        assert_eq!(
            environments,
            BTreeMap::from([
                (OsStr::new("CARGO_TARGET_DIR"), None),
                (OsStr::new("RUSTFLAGS"), None),
                (OsStr::new("RUSTUP_TOOLCHAIN"), None),
            ])
        );
    }
}
mod evidence;
