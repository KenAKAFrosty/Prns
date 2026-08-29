#![forbid(unsafe_code)]

mod build_identity;
mod cli;
mod command_context;
mod daemon;
mod i2p;
mod interface_discovery;
mod interfaces;
mod managed_service;
mod nnpages;
mod observability;
mod persistence;
mod services;
mod shutdown;
mod splash;
mod terminal;
#[cfg(test)]
mod test_support;
#[cfg(all(
    feature = "tray",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
mod tray;
mod utilities;

use std::process::ExitCode;

use prnsd_control::{ForegroundSpec, LogLane, ManagedProcess, ServicePaths};

#[cfg(not(all(feature = "tray", any(target_os = "macos", target_os = "windows"))))]
#[tokio::main(flavor = "multi_thread")]
async fn main() -> ExitCode {
    let command = match command_from_environment() {
        Ok(Some(command)) => command,
        Ok(None) => return ExitCode::SUCCESS,
        Err(exit_code) => return exit_code,
    };
    run_command(command).await
}

#[cfg(all(feature = "tray", any(target_os = "macos", target_os = "windows")))]
fn main() -> ExitCode {
    let command = match command_from_environment() {
        Ok(Some(command)) => command,
        Ok(None) => return ExitCode::SUCCESS,
        Err(exit_code) => return exit_code,
    };
    let command = match command {
        cli::Command::Run(args) if !args.service => {
            let managed = match run_process_context(&args) {
                Ok(managed) => managed,
                Err(error) => {
                    eprintln!("prnsd: {error}");
                    return ExitCode::FAILURE;
                }
            };
            tray::run(args.daemon, managed);
        }
        command => command,
    };

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("prnsd: async runtime initialization failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(run_command(command))
}

fn command_from_environment() -> Result<Option<cli::Command>, ExitCode> {
    #[cfg(windows)]
    prns_ffi::console::enable_ansi_sequences();
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() == 2 && args.get(1).is_some_and(|arg| arg == "--print-banner") {
        splash::print_daemon();
        return Ok(None);
    }
    match cli::parse_from(args) {
        Ok(command) => Ok(Some(command)),
        Err(error) => {
            let exit_code = error.exit_code();
            let _ = error.print();
            Err(ExitCode::from(exit_code.clamp(0, 255) as u8))
        }
    }
}

async fn run_command(command: cli::Command) -> ExitCode {
    match command {
        cli::Command::Run(args) => {
            let managed = match run_process_context(&args) {
                Ok(managed) => managed,
                Err(error) => {
                    eprintln!("prnsd: {error}");
                    return ExitCode::FAILURE;
                }
            };
            match daemon::run(args.daemon, managed, None, None).await {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("prnsd: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        cli::Command::I2p(args) => i2p::run(args).await,
        cli::Command::Interfaces(args) => interfaces::run(*args),
        cli::Command::NnPages(args) => match nnpages::run_cli(args).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("prnsd nnpages: {error}");
                ExitCode::FAILURE
            }
        },
        cli::Command::Status(args) => match utilities::rnstatus::run(args).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("prnsd status: {error}");
                ExitCode::FAILURE
            }
        },
        cli::Command::Path(args) => match utilities::rnpath::run(args).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                let exit_code = error.exit_code();
                eprintln!("prnsd path: {error}");
                ExitCode::from(exit_code)
            }
        },
        cli::Command::Probe(args) => match utilities::rnprobe::run(args).await {
            Ok(outcome) => ExitCode::from(outcome.exit_code()),
            Err(error) => {
                let exit_code = error.exit_code();
                eprintln!("prnsd probe: {error}");
                ExitCode::from(exit_code)
            }
        },
        cli::Command::Id(args) => match utilities::rnid::run(*args).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                let exit_code = error.exit_code();
                eprintln!("prnsd id: {error}");
                ExitCode::from(exit_code)
            }
        },
        cli::Command::Cp(args) => match utilities::rncp::run(*args).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("prnsd cp: {error}");
                ExitCode::FAILURE
            }
        },
        cli::Command::X(args) => match utilities::rnx::run(*args).await {
            Ok(outcome) => ExitCode::from(outcome.exit_code()),
            Err(error) => {
                let exit_code = error.exit_code();
                eprintln!("prnsd x: {error}");
                ExitCode::from(exit_code)
            }
        },
        cli::Command::Start(args) => managed_service::run(managed_service::Command::Start(args)),
        cli::Command::Restart(args) => {
            managed_service::run(managed_service::Command::Restart(args))
        }
        cli::Command::Stop => managed_service::run(managed_service::Command::Stop),
        cli::Command::Logs => managed_service::run(managed_service::Command::Logs),
    }
}

fn run_process_context(args: &cli::RunArgs) -> Result<Option<ManagedProcess>, RunContextError> {
    if let Some(managed) = ManagedProcess::from_environment().map_err(RunContextError::Service)? {
        return Ok(Some(managed));
    }
    if !args.service && args.daemon.config.is_some() {
        return Ok(None);
    }
    let paths = ServicePaths::discover().map_err(RunContextError::StateDirectory)?;
    if args.service {
        let binary = std::env::current_exe().map_err(RunContextError::CurrentExecutable)?;
        let log_lane = match args.daemon.log_format {
            cli::LogFormat::Human => LogLane::Human,
            cli::LogFormat::Json => LogLane::Json,
        };
        let managed = ManagedProcess::adopt_foreground(
            paths,
            ForegroundSpec {
                binary: &binary,
                log_lane,
                signature: prnsd_control::launch_signature(
                    args.daemon.command_line(),
                    std::env::vars_os(),
                ),
                version: env!("CARGO_PKG_VERSION"),
            },
        )
        .map_err(RunContextError::Service)?;
        return Ok(Some(managed));
    }
    if let Some(record) = prnsd_control::running(&paths).map_err(RunContextError::Service)? {
        return Err(RunContextError::ImplicitDuplicate { pid: record.pid });
    }
    Ok(None)
}

#[derive(Debug)]
enum RunContextError {
    StateDirectory(prnsd_control::StateDirectoryError),
    CurrentExecutable(std::io::Error),
    Service(prnsd_control::ServiceError),
    ImplicitDuplicate { pid: u32 },
}

impl std::fmt::Display for RunContextError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StateDirectory(error) => error.fmt(formatter),
            Self::CurrentExecutable(error) => {
                write!(formatter, "could not locate the prnsd executable: {error}")
            }
            Self::Service(error) => error.fmt(formatter),
            Self::ImplicitDuplicate { pid } => write!(
                formatter,
                "prnsd is already running (pid {pid}); use the normal operator commands or pass --config DIR for an explicitly isolated instance"
            ),
        }
    }
}
