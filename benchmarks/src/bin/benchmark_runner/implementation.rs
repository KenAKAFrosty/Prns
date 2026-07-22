use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use benchmarks::{load_implementations, ImplementationDescriptor};

#[derive(Clone)]
pub(super) struct Implementation {
    descriptor: ImplementationDescriptor,
}

impl Implementation {
    pub(super) fn name(&self) -> &str {
        &self.descriptor.slug
    }

    pub(super) fn slug(&self) -> &str {
        &self.descriptor.slug
    }

    pub(super) fn label(&self) -> &str {
        &self.descriptor.implementation
    }

    pub(super) fn interop_command(&self) -> Option<Command> {
        let participant = self.descriptor.participant.as_ref()?;
        let expanded: Vec<OsString> = participant
            .command
            .iter()
            .map(|component| OsString::from(expand(component)))
            .collect();
        let (program, args) = expanded.split_first()?;
        let mut command = Command::new(program);
        command.args(args);
        Some(command)
    }
}

fn benchmark_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn bin_dir() -> PathBuf {
    std::env::current_exe()
        .expect("current benchmark executable")
        .parent()
        .expect("benchmark binary directory")
        .to_path_buf()
}

fn reference_python() -> PathBuf {
    std::env::var_os("RNS_REFERENCE_PYTHON")
        .filter(|path| Path::new(path).exists())
        .map(PathBuf::from)
        .or_else(|| {
            let reference = benchmark_dir().join("reference");
            [
                reference.join(".venv/bin/python"),
                reference.join(".venv/Scripts/python.exe"),
            ]
            .into_iter()
            .find(|path| path.exists())
        })
        .unwrap_or_else(|| PathBuf::from("python3"))
}

fn expand(component: &str) -> String {
    component
        .replace("{benchmark_dir}", &benchmark_dir().to_string_lossy())
        .replace("{bin_dir}", &bin_dir().to_string_lossy())
        .replace("{reference_python}", &reference_python().to_string_lossy())
}

pub(super) fn implementation(name: &str) -> Implementation {
    let descriptors = load_implementations();
    let known = descriptors
        .iter()
        .map(|descriptor| descriptor.slug.as_str())
        .collect::<Vec<_>>()
        .join("|");
    let descriptor = descriptors
        .into_iter()
        .find(|descriptor| descriptor.slug == name)
        .unwrap_or_else(|| panic!("unknown implementation {name:?} ({known})"));
    Implementation { descriptor }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptors_drive_exact_public_commands() {
        let ours = implementation("personal-rns");
        assert_eq!(ours.slug(), "personal-rns");
        assert!(ours.interop_command().is_some());
        let reference = implementation("rns-1.4.0-compiled");
        assert_eq!(reference.slug(), "rns-1.4.0-compiled");
        assert!(reference.interop_command().is_some());
    }
}
