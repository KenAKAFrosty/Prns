use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};
use personal_hopspot_builder::{capture_source_custody, SourceCaptureError};
use thiserror::Error;

use crate::baseline::{self, BaselineError};
use crate::contract::{
    ArchitectureId, ComponentId, IdentifierError, MatrixStatus, MiriCoverage, RunnerId, ScenarioId,
};
use crate::evidence::{
    record_miri, record_target_isa, MiriRecordRequest, RecordError, TargetIsaRecordRequest,
};
use crate::report::{self, ComparisonError, SummaryError};

#[derive(Parser)]
#[command(name = "personal-hopspot-assurance")]
struct Cli {
    #[command(subcommand)]
    command: AssuranceCommand,
}

#[derive(Subcommand)]
enum AssuranceCommand {
    Compare(CompareArguments),
    Record(Box<RecordArguments>),
    RefreshBaseline(RefreshBaselineArguments),
    Summarize(SummarizeArguments),
}

#[derive(Args)]
struct RecordArguments {
    #[command(subcommand)]
    command: RecordCommand,
}

#[derive(Subcommand)]
enum RecordCommand {
    Isa(TargetIsaRecordArguments),
    Miri(MiriRecordArguments),
}

#[derive(Args)]
struct TargetIsaRecordArguments {
    #[arg(long)]
    architecture: String,
    #[arg(long)]
    scenario: String,
    #[arg(long)]
    runner: String,
    #[arg(long)]
    completed_scenarios: u32,
    #[arg(long)]
    cargo_version: String,
    #[arg(long)]
    rustc_version: String,
    #[arg(long)]
    qemu_version: String,
    #[arg(long)]
    qemu_executable: PathBuf,
    #[arg(long)]
    source: Vec<PathBuf>,
    #[arg(long)]
    transcript: PathBuf,
    #[arg(long)]
    executable: PathBuf,
    #[arg(long)]
    log: Vec<PathBuf>,
    #[arg(long)]
    output: PathBuf,
}

#[derive(Args)]
struct MiriRecordArguments {
    #[arg(long)]
    component: String,
    #[arg(long)]
    scenario: String,
    #[arg(long)]
    runner: String,
    #[arg(long)]
    coverage: MiriCoverageArgument,
    #[arg(long)]
    completed_tests: u32,
    #[arg(long)]
    rustc_version: String,
    #[arg(long)]
    miri_version: String,
    #[arg(long)]
    source: Vec<PathBuf>,
    #[arg(long)]
    log: Vec<PathBuf>,
    #[arg(long)]
    output: PathBuf,
}

#[derive(Clone, Copy, ValueEnum)]
enum MiriCoverageArgument {
    Stacked,
    StackedAndTree,
}

impl From<MiriCoverageArgument> for MiriCoverage {
    fn from(value: MiriCoverageArgument) -> Self {
        match value {
            MiriCoverageArgument::Stacked => Self::Stacked,
            MiriCoverageArgument::StackedAndTree => Self::StackedAndTree,
        }
    }
}

#[derive(Args)]
struct CompareArguments {
    before: PathBuf,
    after: PathBuf,
}

#[derive(Args)]
struct RefreshBaselineArguments {
    #[arg(long)]
    matrix: PathBuf,
}

#[derive(Args)]
struct SummarizeArguments {
    #[arg(long)]
    resources: PathBuf,
    #[arg(long)]
    proofs: PathBuf,
    #[arg(long)]
    output: PathBuf,
}

#[derive(Debug, Error)]
enum AssuranceError {
    #[error("failed to resolve repository root {path}: {source}")]
    ResolveRepositoryRoot {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Baseline(#[from] BaselineError),
    #[error(transparent)]
    Comparison(#[from] ComparisonError),
    #[error(transparent)]
    Identifier(#[from] IdentifierError),
    #[error(transparent)]
    Record(#[from] RecordError),
    #[error(transparent)]
    Summary(#[from] SummaryError),
    #[error(transparent)]
    Source(#[from] SourceCaptureError),
    #[error("source changed while composing embedded assurance evidence")]
    SourceChanged,
    #[error("embedded assurance matrix contains {required_failures} required failures")]
    RequiredEvidence { required_failures: usize },
}

pub fn entrypoint() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ASSURANCE_ERROR: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), AssuranceError> {
    let root_hint = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let root = std::fs::canonicalize(&root_hint).map_err(|source| {
        AssuranceError::ResolveRepositoryRoot {
            path: root_hint,
            source,
        }
    })?;
    match cli.command {
        AssuranceCommand::Compare(arguments) => {
            print!("{}", report::compare(&arguments.before, &arguments.after)?);
        }
        AssuranceCommand::Record(arguments) => match arguments.command {
            RecordCommand::Isa(arguments) => {
                let output = arguments.output.clone();
                record_target_isa(
                    &root,
                    TargetIsaRecordRequest {
                        architecture: ArchitectureId::parse(arguments.architecture)?,
                        scenario: ScenarioId::parse(arguments.scenario)?,
                        runner: RunnerId::parse(arguments.runner)?,
                        completed_scenarios: arguments.completed_scenarios,
                        cargo_version: arguments.cargo_version,
                        rustc_version: arguments.rustc_version,
                        qemu_version: arguments.qemu_version,
                        qemu_executable: arguments.qemu_executable,
                        sources: arguments.source,
                        transcript: arguments.transcript,
                        executable: arguments.executable,
                        logs: arguments.log,
                        output: arguments.output,
                    },
                )?;
                println!("EMBEDDED_ISA_PROOF: {}", output.display());
            }
            RecordCommand::Miri(arguments) => {
                let output = arguments.output.clone();
                record_miri(
                    &root,
                    MiriRecordRequest {
                        component: ComponentId::parse(arguments.component)?,
                        scenario: ScenarioId::parse(arguments.scenario)?,
                        runner: RunnerId::parse(arguments.runner)?,
                        coverage: arguments.coverage.into(),
                        completed_tests: arguments.completed_tests,
                        rustc_version: arguments.rustc_version,
                        miri_version: arguments.miri_version,
                        sources: arguments.source,
                        logs: arguments.log,
                        output: arguments.output,
                    },
                )?;
                println!("EMBEDDED_MIRI_PROOF: {}", output.display());
            }
        },
        AssuranceCommand::RefreshBaseline(arguments) => {
            let source = capture_source_custody(&root)?;
            let outcome = baseline::refresh(&arguments.matrix, &baseline::path(&root), &source)?;
            println!(
                "EMBEDDED_ASSURANCE_BASELINE: targets={} capabilities={} path={}",
                outcome.targets(),
                outcome.capabilities(),
                outcome.path().display(),
            );
        }
        AssuranceCommand::Summarize(arguments) => {
            let source = capture_source_custody(&root)?;
            let outcome = report::summarize(
                &arguments.resources,
                &arguments.proofs,
                &arguments.output,
                &source,
            )?;
            if capture_source_custody(&root)? != source {
                return Err(AssuranceError::SourceChanged);
            }
            println!(
                "EMBEDDED_ASSURANCE_MATRIX: targets={} capabilities={} json={} markdown={}",
                outcome.targets(),
                outcome.capabilities(),
                outcome.json().display(),
                outcome.markdown().display(),
            );
            if let MatrixStatus::Failed { required_failures } = outcome.status() {
                return Err(AssuranceError::RequiredEvidence {
                    required_failures: *required_failures,
                });
            }
        }
    }
    Ok(())
}
