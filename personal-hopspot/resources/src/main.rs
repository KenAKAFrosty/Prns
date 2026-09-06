mod contracts;
mod matrix;

use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};
use personal_hopspot_builder::{
    default_artifact_root, BuildConfiguration, BuildContext, BuildError, BuildVersion, LtoMode,
};
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
#[command(group(
    ArgGroup::new("selection")
        .required(true)
        .multiple(false)
        .args(["all", "target"])
))]
struct ReportArguments {
    #[arg(long)]
    all: bool,
    #[arg(long)]
    target: Option<String>,
    #[arg(long, value_enum, default_value_t)]
    lto: LtoArgument,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
enum LtoArgument {
    #[default]
    Configured,
    Fat,
    Thin,
}

impl From<LtoArgument> for LtoMode {
    fn from(value: LtoArgument) -> Self {
        match value {
            LtoArgument::Configured => Self::Configured,
            LtoArgument::Fat => Self::Fat,
            LtoArgument::Thin => Self::Thin,
        }
    }
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
    let lto = LtoMode::from(arguments.lto);
    let output_root = default_artifact_root(root).join("resources");
    let configuration = BuildConfiguration::new(lto, true);
    let context = BuildContext::new(root, &output_root, BuildVersion::Repository)?
        .with_configuration(configuration);
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
    let adapter = target.adapter();
    println!(
        "EMBEDDED_RESOURCE_BUILD: target={} name={:?} profile={} architecture={} adapter={} linker={} lto={} artifacts={} package_bytes={} elf={}",
        target.id(),
        target.display_name(),
        target.profile().0,
        adapter.rust_target(),
        adapter.id().as_str(),
        adapter.linker_flavor().as_str(),
        context.configuration().lto().as_str(),
        evidence.artifacts().len(),
        evidence.package_bytes(),
        evidence.elf().display()
    );
    for artifact in evidence.artifacts() {
        println!("artifact {} {}", artifact.bytes(), artifact.path());
    }
    if let Some(linker_map) = evidence.linker_map() {
        println!("linker-map {}", linker_map.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_scope_is_required_and_exclusive() {
        assert!(Cli::try_parse_from(["resources", "report"]).is_err());
        assert!(Cli::try_parse_from(["resources", "report", "--all", "--target", "t114"]).is_err());
    }

    #[test]
    fn report_accepts_explicit_lto_modes() -> Result<(), Box<dyn std::error::Error>> {
        let cli =
            Cli::try_parse_from(["resources", "report", "--target", "t114", "--lto", "thin"])?;
        let ResourceCommand::Report(arguments) = cli.command else {
            return Err("report command was not parsed".into());
        };
        assert_eq!(arguments.target.as_deref(), Some("t114"));
        assert_eq!(arguments.lto, LtoArgument::Thin);
        Ok(())
    }
}
