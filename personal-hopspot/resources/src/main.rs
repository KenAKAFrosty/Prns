mod analysis;
mod contracts;
mod matrix;
mod report;

use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};
use personal_hopspot_builder::{
    default_artifact_root, BuildContext, BuildError, BuildIntent, BuildVersion, LtoMode,
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
    Compare(CompareArguments),
    Contracts(ContractsArguments),
    RefreshBaseline,
    Report(ReportArguments),
}

#[derive(Args)]
struct CompareArguments {
    before: PathBuf,
    after: PathBuf,
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
    #[arg(long)]
    allow_overflow: bool,
}

enum OverflowPolicy {
    Reject,
    Report,
}

impl ReportArguments {
    const fn overflow_policy(&self) -> OverflowPolicy {
        if self.allow_overflow {
            OverflowPolicy::Report
        } else {
            OverflowPolicy::Reject
        }
    }
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
    #[error(transparent)]
    Report(#[from] report::ReportError),
    #[error(transparent)]
    Baseline(#[from] report::BaselineError),
    #[error(transparent)]
    Comparison(#[from] report::ComparisonError),
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
        ResourceCommand::Compare(arguments) => {
            print!(
                "{}",
                report::compare_files(&arguments.before, &arguments.after)?
            );
        }
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
        ResourceCommand::RefreshBaseline => {
            refresh_baseline(&root)?;
        }
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
    let intent = BuildIntent::ResourceReport { lto };
    let context =
        BuildContext::new(root, &output_root, BuildVersion::Repository)?.with_intent(intent);
    if arguments.all {
        for target in matrix.iter() {
            build_report(target, &context, arguments.overflow_policy())?;
        }
    } else if let Some(target) = &arguments.target {
        build_report(
            matrix.target(target)?,
            &context,
            arguments.overflow_policy(),
        )?;
    }
    Ok(())
}

fn refresh_baseline(root: &Path) -> Result<(), ResourceError> {
    let catalog = prns_flash_manifest::board_catalog()?;
    let matrix = Matrix::from_catalog(&catalog)?;
    let output_root = default_artifact_root(root).join("resources");
    let context = BuildContext::new(root, &output_root, BuildVersion::Repository)?.with_intent(
        BuildIntent::ResourceReport {
            lto: LtoMode::Configured,
        },
    );
    let reports = matrix
        .iter()
        .map(|target| build_report(target, &context, OverflowPolicy::Reject))
        .collect::<Result<Vec<_>, _>>()?;
    let outcome = report::refresh_baseline(root, &matrix, &context, &reports)?;
    println!(
        "EMBEDDED_RESOURCE_BASELINE: targets={} path={}",
        outcome.targets(),
        outcome.path().display()
    );
    Ok(())
}

fn build_report(
    target: &Target<'_>,
    context: &BuildContext<'_>,
    overflow_policy: OverflowPolicy,
) -> Result<PathBuf, ResourceError> {
    match (overflow_policy, target.build(context)) {
        (_, Ok(evidence)) => write_success_report(target, context, &evidence),
        (
            OverflowPolicy::Report,
            Err(MatrixError::Build {
                source: BuildError::LinkOverflow(evidence),
                ..
            }),
        ) => {
            println!(
                "EMBEDDED_RESOURCE_OVERFLOW: target={} lto={} linker-map={}",
                target.id(),
                context.intent().lto().as_str(),
                evidence.linker_map().display()
            );
            for overflow in evidence.overflows().iter() {
                println!(
                    "overflow region={:?} bytes={}",
                    overflow.region(),
                    overflow.bytes()
                );
            }
            let report = report::write_overflow(target, context, &evidence)?;
            println!("report {}", report.display());
            Ok(report)
        }
        (_, Err(error)) => Err(error.into()),
    }
}

fn write_success_report(
    target: &Target<'_>,
    context: &BuildContext<'_>,
    evidence: &matrix::BuildEvidence,
) -> Result<PathBuf, ResourceError> {
    let adapter = target.adapter();
    println!(
        "EMBEDDED_RESOURCE_BUILD: target={} name={:?} profile={} architecture={} adapter={} linker={} lto={} artifacts={} package_bytes={} elf={}",
        target.id(),
        target.display_name(),
        target.profile().id.0,
        adapter.rust_target(),
        adapter.id().as_str(),
        adapter.linker_flavor().as_str(),
        context.intent().lto().as_str(),
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
    let report = report::write(target, context, evidence)?;
    println!("report {}", report.display());
    Ok(report)
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
        assert!(!arguments.allow_overflow);
        Ok(())
    }

    #[test]
    fn report_accepts_explicit_overflow_evidence_policy() -> Result<(), Box<dyn std::error::Error>>
    {
        let cli = Cli::try_parse_from([
            "resources",
            "report",
            "--target",
            "t-echo-s140-v6",
            "--allow-overflow",
        ])?;
        let ResourceCommand::Report(arguments) = cli.command else {
            return Err("report command was not parsed".into());
        };
        assert!(matches!(
            arguments.overflow_policy(),
            OverflowPolicy::Report
        ));
        Ok(())
    }

    #[test]
    fn compare_requires_exactly_two_report_paths() -> Result<(), Box<dyn std::error::Error>> {
        assert!(Cli::try_parse_from(["resources", "compare", "before.json"]).is_err());
        let cli = Cli::try_parse_from(["resources", "compare", "before.json", "after.json"])?;
        let ResourceCommand::Compare(arguments) = cli.command else {
            return Err("compare command was not parsed".into());
        };
        assert_eq!(arguments.before, Path::new("before.json"));
        assert_eq!(arguments.after, Path::new("after.json"));
        Ok(())
    }

    #[test]
    fn refresh_baseline_accepts_no_selection_or_codegen_override(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let cli = Cli::try_parse_from(["resources", "refresh-baseline"])?;
        assert!(matches!(cli.command, ResourceCommand::RefreshBaseline));
        assert!(Cli::try_parse_from(["resources", "refresh-baseline", "--lto", "thin"]).is_err());
        Ok(())
    }
}
