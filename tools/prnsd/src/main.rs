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

const TOOL_MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");

#[derive(Debug)]
enum CommandError {
    Arguments(ArgumentError),
    CargoSpawn(std::io::Error),
    CargoFailed(Option<i32>),
    DaemonExited(Option<i32>),
    BinaryMissing(PathBuf),
    Service(prnsd_control::ServiceError),
    StateDirectory(prnsd_control::StateDirectoryError),
    NotRunning,
}

impl fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Arguments(error) => error.fmt(formatter),
            Self::CargoSpawn(error) => write!(formatter, "failed to run cargo: {error}"),
            Self::CargoFailed(Some(code)) => write!(formatter, "cargo exited with status {code}"),
            Self::CargoFailed(None) => formatter.write_str("cargo exited unsuccessfully"),
            Self::DaemonExited(Some(code)) => write!(formatter, "prnsd exited with status {code}"),
            Self::DaemonExited(None) => formatter.write_str("prnsd exited unsuccessfully"),
            Self::BinaryMissing(path) => write!(
                formatter,
                "cargo completed without producing the expected daemon at {}",
                path.display()
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
    let invocation = parse_invocation(args)?;
    let root = repo_root();
    let manifest = root.join("prnsd/Cargo.toml");
    if invocation.action == Action::OneShot {
        return run_daemon_through_cargo(cargo_run_arguments(&invocation, &manifest)?, &root);
    }

    let paths = ServicePaths::discover()?;
    let signature = launch_signature(&invocation, env::vars_os());
    match invocation.action {
        Action::Start => start_or_attach(&invocation, &root, &manifest, &paths, signature),
        Action::Restart => {
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

fn repo_root() -> PathBuf {
    PathBuf::from(TOOL_MANIFEST_DIR)
        .parent()
        .and_then(Path::parent)
        .expect("tools/prnsd lives under tools/")
        .to_path_buf()
}
