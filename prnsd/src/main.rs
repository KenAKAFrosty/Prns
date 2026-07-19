//! The Personal Reticulum daemon: a configurable shared-instance node built on [`personal_rns::runtime::PrnsNode`].
#![forbid(unsafe_code)]

mod blackhole_exchange;
mod cli;
mod construct;
mod daemon;
mod i2p;
mod identity;
mod interface_discovery;
mod managed_service;
mod management_announces;
#[cfg(feature = "otlp")]
mod metrics;
mod observability;
mod persist;
mod probe_responder;
mod remote_management;
mod request_services;
mod splash;
mod startup_progress;

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
        cli::Command::Start(args) => managed_service::run(managed_service::Command::Start(args)),
        cli::Command::Restart(args) => {
            managed_service::run(managed_service::Command::Restart(args))
        }
        cli::Command::Stop => managed_service::run(managed_service::Command::Stop),
        cli::Command::Status => managed_service::run(managed_service::Command::Status),
        cli::Command::Logs => managed_service::run(managed_service::Command::Logs),
    }
}
