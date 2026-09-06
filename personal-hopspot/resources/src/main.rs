mod contracts;
mod matrix;

use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use personal_hopspot_builder::{default_artifact_root, BuildContext, BuildError, BuildVersion};
use thiserror::Error;

use contracts::{ContractOutcome, ContractsMode};
use matrix::{Matrix, MatrixError, Target};

#[derive(Parser)]
#[command(name = "personal-hopspot-resources")]
struct Cli {
    #[command(subcommand)]
    command: ResourceCommand,
}

#[derive(Subcommand)]
enum ResourceCommand {
    Contracts(ContractsArguments),
    Report(ReportArguments),
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

#[derive(Args)]
#[group(required = true, multiple = false)]
struct ReportArguments {
    #[arg(long)]
    all: bool,
    #[arg(long)]
    target: Option<String>,
}

#[derive(Debug, Error)]
enum ResourceError {
    #[error("failed to resolve repository root {path}: {source}")]
    ResolveRepositoryRoot {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Catalog(#[from] prns_flash_manifest::CatalogError),
    #[error(transparent)]
    Contracts(#[from] contracts::ContractError),
    #[error(transparent)]
    Matrix(#[from] MatrixError),
    #[error(transparent)]
    Build(#[from] BuildError),
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

fn run(cli: Cli) -> Result<(), ResourceError> {
    let root_hint = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let root = std::fs::canonicalize(&root_hint).map_err(|source| {
        ResourceError::ResolveRepositoryRoot {
            path: root_hint,
            source,
        }
    })?;
    match cli.command {
        ResourceCommand::Contracts(arguments) => match contracts::run(&root, arguments.mode())? {
            ContractOutcome::Verified { artifact_count } => {
                println!(
                    "EMBEDDED_CONTRACTS_OK: {artifact_count} artifacts match canonical profiles"
                );
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
        },
        ResourceCommand::Report(arguments) => {
            build_reports(&root, &arguments)?;
        }
    }
    Ok(())
}

fn build_reports(root: &Path, arguments: &ReportArguments) -> Result<(), ResourceError> {
    let catalog = prns_flash_manifest::board_catalog()?;
    let matrix = Matrix::from_catalog(&catalog)?;
    let output_root = default_artifact_root(root);
    let context = BuildContext::new(root, &output_root, BuildVersion::Repository)?;
    if arguments.all {
        for target in matrix.iter() {
            build_report(target, &context)?;
        }
    } else if let Some(target) = &arguments.target {
        build_report(matrix.target(target)?, &context)?;
    }
    Ok(())
}

fn build_report(target: &Target<'_>, context: &BuildContext<'_>) -> Result<(), ResourceError> {
    let evidence = target.build(context)?;
    println!(
        "EMBEDDED_RESOURCE_BUILD: target={} name={:?} profile={} architecture={} artifacts={} package_bytes={} elf={}",
        target.id(),
        target.display_name(),
        target.profile().0,
        target.architecture().rust_target(),
        evidence.artifacts().len(),
        evidence.package_bytes(),
        evidence.elf().display()
    );
    for artifact in evidence.artifacts() {
        println!("artifact {} {}", artifact.bytes(), artifact.path());
    }
    Ok(())
}
