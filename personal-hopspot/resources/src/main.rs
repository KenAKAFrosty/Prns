mod contracts;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use contracts::{ContractOutcome, ContractsMode};

#[derive(Parser)]
#[command(name = "personal-hopspot-resources")]
struct Cli {
    #[command(subcommand)]
    command: ResourceCommand,
}

#[derive(Subcommand)]
enum ResourceCommand {
    Contracts(ContractsArguments),
}

#[derive(Args)]
#[group(required = true, multiple = false)]
struct ContractsArguments {
    #[arg(long)]
    check: bool,
    #[arg(long)]
    write: bool,
}

impl ContractsArguments {
    fn mode(&self) -> ContractsMode {
        if self.check {
            ContractsMode::Check
        } else {
            ContractsMode::Write
        }
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("RESOURCE_ERROR: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), contracts::ContractError> {
    let root_hint = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let root = std::fs::canonicalize(&root_hint).map_err(|source| {
        contracts::ContractError::ResolveRepositoryRoot {
            path: root_hint,
            source,
        }
    })?;
    let ResourceCommand::Contracts(arguments) = cli.command;
    match contracts::run(&root, arguments.mode())? {
        ContractOutcome::Verified { artifact_count } => {
            println!("EMBEDDED_CONTRACTS_OK: {artifact_count} artifacts match canonical profiles");
        }
        ContractOutcome::Written { updated, unchanged } => {
            for path in &updated {
                println!("updated {}", path.display());
            }
            println!(
                "EMBEDDED_CONTRACTS_WRITTEN: {} updated, {unchanged} unchanged",
                updated.len()
            );
        }
    }
    Ok(())
}
