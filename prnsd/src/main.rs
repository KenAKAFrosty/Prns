#![forbid(unsafe_code)]

mod cli;
mod daemon;
mod i2p;
mod interface_discovery;
mod managed_service;
mod observability;
mod persistence;
mod services;
mod splash;
mod utilities;

use std::process::ExitCode;

use prnsd_control::ManagedProcess;

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() == 2 && args.get(1).is_some_and(|arg| arg == "--print-banner") {
        splash::print_daemon();
        return ExitCode::SUCCESS;
    }
    let command = match cli::parse_from(args) {
        Ok(command) => command,
        Err(error) => {
            let exit_code = error.exit_code();
            let _ = error.print();
            return ExitCode::from(exit_code.clamp(0, 255) as u8);
        }
    };
    match command {
        cli::Command::Run(args) => {
            let managed = match ManagedProcess::from_environment() {
                Ok(managed) => managed,
                Err(error) => {
                    eprintln!("prnsd: {error}");
                    return ExitCode::FAILURE;
                }
            };
            daemon::run(args, managed).await;
            ExitCode::SUCCESS
        }
        cli::Command::I2p(args) => i2p::run(args).await,
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
        cli::Command::Start(args) => managed_service::run(managed_service::Command::Start(args)),
        cli::Command::Restart(args) => {
            managed_service::run(managed_service::Command::Restart(args))
        }
        cli::Command::Stop => managed_service::run(managed_service::Command::Stop),
        cli::Command::Logs => managed_service::run(managed_service::Command::Logs),
    }
}
