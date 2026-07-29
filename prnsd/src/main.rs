#![forbid(unsafe_code)]

mod cli;
mod daemon;
mod i2p;
mod interface_discovery;
mod interfaces;
mod managed_service;
mod node_pages;
mod observability;
mod persistence;
mod services;
mod shutdown;
mod splash;
mod terminal;
#[cfg(all(
    feature = "tray",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
mod tray;
mod utilities;

use std::process::ExitCode;

use prnsd_control::ManagedProcess;

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
        cli::Command::Run(args) => {
            let managed = match tray::managed_process() {
                Ok(managed) => managed,
                Err(exit_code) => return exit_code,
            };
            tray::run(args, managed);
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
            let managed = match ManagedProcess::from_environment() {
                Ok(managed) => managed,
                Err(error) => {
                    eprintln!("prnsd: {error}");
                    return ExitCode::FAILURE;
                }
            };
            match daemon::run(args, managed, None, None).await {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("prnsd: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        cli::Command::I2p(args) => i2p::run(args).await,
        cli::Command::Interfaces(args) => interfaces::run(*args),
        cli::Command::Pages(args) => match node_pages::run_cli(args).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("prnsd pages: {error}");
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
