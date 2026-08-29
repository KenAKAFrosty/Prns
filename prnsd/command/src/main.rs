mod arguments;
mod build;
mod lifecycle;

use std::env;
use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use arguments::{help_requested, parse_invocation, print_help, Action, ArgumentError};
use build::{build_daemon, cargo_run_arguments, run_daemon_through_cargo};
use lifecycle::{attach, launch_signature, print_banner, start_built, start_or_attach};
use prnsd_control::ServicePaths;

const COMMAND_MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");

#[derive(Debug)]
enum CommandError {
    Arguments(ArgumentError),
    CargoSpawn(std::io::Error),
    CargoWait(std::io::Error),
    CargoMessage(serde_json::Error),
    CargoStdoutUnavailable,
    CargoFailed(Option<i32>),
    SourceSnapshotSpawn(std::io::Error),
    SourceSnapshotFailed(Option<i32>),
    DaemonExited(Option<i32>),
    DaemonArtifactMissing,
    DaemonArtifactConflict { first: PathBuf, second: PathBuf },
    Service(prnsd_control::ServiceError),
    StateDirectory(prnsd_control::StateDirectoryError),
    NotRunning,
}

impl fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Arguments(error) => error.fmt(formatter),
            Self::CargoSpawn(error) => write!(formatter, "failed to run cargo: {error}"),
            Self::CargoWait(error) => write!(formatter, "failed to wait for cargo: {error}"),
            Self::CargoMessage(error) => write!(formatter, "invalid cargo build message: {error}"),
            Self::CargoStdoutUnavailable => {
                formatter.write_str("cargo build output was unavailable")
            }
            Self::CargoFailed(Some(code)) => write!(formatter, "cargo exited with status {code}"),
            Self::CargoFailed(None) => formatter.write_str("cargo exited unsuccessfully"),
            Self::SourceSnapshotSpawn(error) => {
                write!(
                    formatter,
                    "failed to package the local source snapshot: {error}"
                )
            }
            Self::SourceSnapshotFailed(Some(code)) => {
                write!(
                    formatter,
                    "source snapshot packaging exited with status {code}"
                )
            }
            Self::SourceSnapshotFailed(None) => {
                formatter.write_str("source snapshot packaging exited unsuccessfully")
            }
            Self::DaemonExited(Some(code)) => write!(formatter, "prnsd exited with status {code}"),
            Self::DaemonExited(None) => formatter.write_str("prnsd exited unsuccessfully"),
            Self::DaemonArtifactMissing => {
                formatter.write_str("cargo completed without reporting the prnsd executable")
            }
            Self::DaemonArtifactConflict { first, second } => write!(
                formatter,
                "cargo reported conflicting prnsd executables at {} and {}",
                first.display(),
                second.display()
            ),
            Self::Service(error) => error.fmt(formatter),
            Self::StateDirectory(error) => error.fmt(formatter),
            Self::NotRunning => formatter.write_str("prnsd is not running"),
        }
    }
}

impl From<ArgumentError> for CommandError {
    fn from(error: ArgumentError) -> Self {
        Self::Arguments(error)
    }
}

impl From<prnsd_control::ServiceError> for CommandError {
    fn from(error: prnsd_control::ServiceError) -> Self {
        Self::Service(error)
    }
}

impl From<prnsd_control::StateDirectoryError> for CommandError {
    fn from(error: prnsd_control::StateDirectoryError) -> Self {
        Self::StateDirectory(error)
    }
}

fn main() -> ExitCode {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    if help_requested(&args) {
        print_help();
        return ExitCode::SUCCESS;
    }
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(CommandError::DaemonExited(code)) => {
            ExitCode::from(code.unwrap_or(1).clamp(1, 255) as u8)
        }
        Err(error) => {
            eprintln!("prnsd: {error}");
            match error {
                CommandError::Arguments(_) => ExitCode::from(2),
                CommandError::NotRunning => ExitCode::from(3),
                CommandError::CargoFailed(Some(code)) => ExitCode::from(code.clamp(1, 255) as u8),
                _ => ExitCode::FAILURE,
            }
        }
    }
}

fn run(args: &[OsString]) -> Result<(), CommandError> {
    let mut invocation = parse_invocation(args)?;
    let root = repo_root();
    let manifest = root.join("prnsd/Cargo.toml");
    if invocation.action == Action::OneShot {
        provision_local_source_snapshot(&mut invocation, &root)?;
        return run_daemon_through_cargo(cargo_run_arguments(&invocation, &manifest)?, &root);
    }

    let paths = ServicePaths::discover()?;
    let signature = launch_signature(&invocation, env::vars_os());
    match invocation.action {
        Action::Start => start_or_attach(&invocation, &root, &manifest, &paths, signature),
        Action::Restart => {
            configure_local_source_environment(&root)?;
            let binary = build_daemon(&invocation, &root, &manifest, false)?;
            if prnsd_control::stop(&paths)? {
                eprintln!("Stopped prnsd");
            }
            start_built(&invocation, &root, &paths, signature, binary)
        }
        Action::Build => {
            let binary = build_daemon(&invocation, &root, &manifest, true)?;
            println!("{}", binary.display());
            Ok(())
        }
        Action::Stop => match prnsd_control::running(&paths)? {
            Some(record) => {
                print_banner(&record.binary);
                eprintln!(
                    "Stopping prnsd (pid {}); showing recent and shutdown logs\n",
                    record.pid
                );
                prnsd_control::stop_and_follow(&paths, &record)?;
                eprintln!("\nStopped prnsd");
                Ok(())
            }
            None => {
                eprintln!("prnsd is already stopped");
                Ok(())
            }
        },
        Action::Logs => match prnsd_control::running(&paths)? {
            Some(record) => attach(&paths, &record),
            None => Err(CommandError::NotRunning),
        },
        Action::OneShot => {
            run_daemon_through_cargo(cargo_run_arguments(&invocation, &manifest)?, &root)
        }
    }
}

fn provision_local_source_snapshot(
    invocation: &mut arguments::Invocation,
    root: &Path,
) -> Result<(), CommandError> {
    if !requests_implicit_local_source(invocation) || env::var_os("PRNSD_SOURCE_ARCHIVE").is_some()
    {
        return Ok(());
    }
    let archive = package_local_source_snapshot(root)?;
    invocation
        .daemon_args
        .push(OsString::from("--source-archive"));
    invocation.daemon_args.push(archive.into_os_string());
    Ok(())
}

fn configure_local_source_environment(root: &Path) -> Result<(), CommandError> {
    if env::var_os("PRNSD_SOURCE_ARCHIVE").is_some() {
        return Ok(());
    }
    let archive = root.join("target/nnpages-source/source.zip");
    if !archive.is_file() {
        return Ok(());
    }
    let archive = package_local_source_snapshot(root)?;
    env::set_var("PRNSD_SOURCE_ARCHIVE", archive);
    Ok(())
}

fn package_local_source_snapshot(root: &Path) -> Result<PathBuf, CommandError> {
    let archive = root.join("target/nnpages-source/source.zip");
    let status = std::process::Command::new("python3")
        .arg(root.join("tools/release/package-source-snapshot.py"))
        .arg("--output")
        .arg(&archive)
        .current_dir(root)
        .status()
        .map_err(CommandError::SourceSnapshotSpawn)?;
    if !status.success() {
        return Err(CommandError::SourceSnapshotFailed(status.code()));
    }
    Ok(archive)
}

fn requests_implicit_local_source(invocation: &arguments::Invocation) -> bool {
    let args = &invocation.daemon_args;
    args.first().is_some_and(|arg| arg == "nnpages")
        && args.get(1).is_some_and(|arg| arg == "seed")
        && args.iter().any(|arg| arg == "--source")
        && !args.iter().any(|arg| {
            arg == "--source-archive"
                || arg.to_str().is_some_and(|arg| {
                    arg.strip_prefix("--source-archive")
                        .is_some_and(|rest| rest.starts_with('='))
                })
        })
}

fn repo_root() -> PathBuf {
    PathBuf::from(COMMAND_MANIFEST_DIR)
        .parent()
        .and_then(Path::parent)
        .expect("prnsd command lives under prnsd/")
        .to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invocation(args: &[&str]) -> arguments::Invocation {
        arguments::parse_invocation(&args.iter().copied().map(OsString::from).collect::<Vec<_>>())
            .expect("valid invocation")
    }

    #[test]
    fn cargo_source_seed_requests_a_local_snapshot_only_when_implicit() {
        assert!(requests_implicit_local_source(&invocation(&[
            "nnpages", "seed", "--source"
        ])));
        assert!(!requests_implicit_local_source(&invocation(&[
            "nnpages", "seed"
        ])));
        assert!(!requests_implicit_local_source(&invocation(&[
            "nnpages",
            "seed",
            "--source",
            "--source-archive",
            "/release/source.zip"
        ])));
        assert!(!requests_implicit_local_source(&invocation(&[
            "nnpages",
            "seed",
            "--source",
            "--source-archive=/release/source.zip"
        ])));
    }
}
