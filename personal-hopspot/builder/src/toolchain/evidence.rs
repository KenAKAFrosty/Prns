use std::path::Path;
use std::process::Command;

use crate::architecture::Adapter;
use crate::{capture_stdout, BuildError, ToolchainEvidence};

pub(crate) fn capture_toolchain_evidence(
    cargo: &Command,
    adapter: &Adapter,
    linker: &Path,
) -> Result<ToolchainEvidence, BuildError> {
    let rustc_version = capture_stdout(configured_command(cargo, "rustc").arg("-vV"), "rustc -vV")?;
    let cargo_version = capture_stdout(configured_command(cargo, "cargo").arg("-V"), "cargo -V")?;
    let linker_version = capture_stdout(
        configured_command(cargo, linker).args(adapter.linker_version_arguments()),
        "linker --version",
    )?;
    Ok(ToolchainEvidence::new(
        rustc_version.trim().to_string(),
        cargo_version.trim().to_string(),
        linker_version.trim().to_string(),
    ))
}

fn configured_command(cargo: &Command, program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    if let Some(directory) = cargo.get_current_dir() {
        command.current_dir(directory);
    }
    for (key, value) in cargo.get_envs() {
        if let Some(value) = value {
            command.env(key, value);
        } else {
            command.env_remove(key);
        }
    }
    command
}
