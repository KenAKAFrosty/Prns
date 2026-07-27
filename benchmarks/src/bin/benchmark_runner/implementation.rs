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
        let mut command = match venv_base_interpreter(Path::new(program)) {
            Some(base) => {
                let mut command = Command::new(base);
                command.env("__PYVENV_LAUNCHER__", program);
                command
            }
            None => Command::new(program),
        };
        command.args(args);
        Some(command)
    }
}

fn venv_base_interpreter(program: &Path) -> Option<PathBuf> {
    if !program
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("python.exe"))
    {
        return None;
    }
    let venv = program.parent()?.parent()?;
    let config = std::fs::read_to_string(venv.join("pyvenv.cfg")).ok()?;
    let home = config.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        (key.trim() == "home").then(|| value.trim().to_string())
    })?;
    let base = Path::new(&home).join("python.exe");
    base.exists().then_some(base)
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
    fn venv_trampolines_resolve_to_the_base_interpreter() {
        let root = std::env::temp_dir().join(format!("venv-redirect-{}", std::process::id()));
        let scripts = root.join("venv/Scripts");
        let home = root.join("base");
        std::fs::create_dir_all(&scripts).expect("create venv scripts dir");
        std::fs::create_dir_all(&home).expect("create base interpreter dir");
        std::fs::write(home.join("python.exe"), b"").expect("create base python");
        std::fs::write(
            root.join("venv/pyvenv.cfg"),
            format!("home = {}\nversion_info = 3.13\n", home.display()),
        )
        .expect("write pyvenv.cfg");
        assert_eq!(
            venv_base_interpreter(&scripts.join("python.exe")),
            Some(home.join("python.exe"))
        );
        assert_eq!(venv_base_interpreter(&scripts.join("rnsd.exe")), None);
        assert_eq!(venv_base_interpreter(Path::new("python.exe")), None);
        let _ = std::fs::remove_dir_all(&root);
    }

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
