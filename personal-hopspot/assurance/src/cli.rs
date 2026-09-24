use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};
use personal_hopspot_builder::{capture_source_custody, SourceCaptureError};
use thiserror::Error;

use crate::baseline::{self, BaselineError};
use crate::contract::{
    ArchitectureId, ComponentId, FailureKind, IdentifierError, MatrixStatus, MiriCoverage,
    PlatformId, PlatformMilestone, RunnerId, ScenarioId, ToolKind,
};
use crate::evidence::{
    record_failure, record_miri, record_platform_emulation, record_target_isa, FailureCapability,
    FailureRecordRequest, MiriRecordRequest, PlatformRecordRequest, RecordError,
    TargetIsaRecordRequest,
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
    Failure(FailureRecordArguments),
    Isa(TargetIsaRecordArguments),
    Miri(MiriRecordArguments),
    Platform(PlatformRecordArguments),
}

#[derive(Args)]
struct FailureRecordArguments {
    #[command(subcommand)]
    capability: FailureCapabilityArguments,
}

#[derive(Subcommand)]
enum FailureCapabilityArguments {
    Isa(TargetIsaFailureArguments),
    Miri(MiriFailureArguments),
    Platform(PlatformFailureArguments),
}

#[derive(Args)]
struct FailureArguments {
    #[arg(long)]
    scenario: String,
    #[arg(long)]
    runner: String,
    #[arg(long)]
    failure_kind: FailureKindArgument,
    #[arg(long)]
    diagnostic: String,
    #[arg(long)]
    source: Vec<PathBuf>,
    #[arg(long)]
    log: Vec<PathBuf>,
    #[arg(long)]
    output: PathBuf,
}

#[derive(Args)]
struct MiriFailureArguments {
    #[command(flatten)]
    failure: FailureArguments,
    #[arg(long)]
    component: String,
    #[arg(long)]
    rustc_version: String,
    #[arg(long)]
    miri_version: String,
}

#[derive(Args)]
struct TargetIsaFailureArguments {
    #[command(flatten)]
    failure: FailureArguments,
    #[arg(long)]
    architecture: String,
    #[arg(long)]
    cargo_version: String,
    #[arg(long)]
    rustc_version: String,
    #[arg(long)]
    qemu_version: String,
    #[arg(long)]
    qemu_executable: PathBuf,
}

#[derive(Args)]
struct PlatformFailureArguments {
    #[command(flatten)]
    failure: FailureArguments,
    #[arg(long)]
    platform: String,
    #[arg(long)]
    cargo_version: String,
    #[arg(long)]
    rustc_version: String,
    #[arg(long)]
    linker_version: String,
    #[arg(long)]
    emulator: PlatformEmulatorArgument,
    #[arg(long)]
    emulator_version: String,
    #[arg(long)]
    emulator_executable: PathBuf,
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

#[derive(Args)]
struct PlatformRecordArguments {
    #[arg(long)]
    platform: String,
    #[arg(long)]
    scenario: String,
    #[arg(long)]
    runner: String,
    #[arg(long)]
    milestone: PlatformMilestoneArgument,
    #[arg(long)]
    cargo_version: String,
    #[arg(long)]
    rustc_version: String,
    #[arg(long)]
    linker_version: String,
    #[arg(long)]
    emulator: PlatformEmulatorArgument,
    #[arg(long)]
    emulator_version: String,
    #[arg(long)]
    emulator_executable: PathBuf,
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

#[derive(Clone, Copy, ValueEnum)]
enum PlatformMilestoneArgument {
    ApplicationEntry,
    RuntimeInitialized,
}

impl From<PlatformMilestoneArgument> for PlatformMilestone {
    fn from(value: PlatformMilestoneArgument) -> Self {
        match value {
            PlatformMilestoneArgument::ApplicationEntry => Self::ApplicationEntry,
            PlatformMilestoneArgument::RuntimeInitialized => Self::RuntimeInitialized,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum PlatformEmulatorArgument {
    Qemu,
    Renode,
}

#[derive(Clone, Copy, ValueEnum)]
enum FailureKindArgument {
    Crash,
    ScenarioMismatch,
    StructuralViolation,
    Timeout,
    ToolFailure,
}

impl From<FailureKindArgument> for FailureKind {
    fn from(value: FailureKindArgument) -> Self {
        match value {
            FailureKindArgument::Crash => Self::Crash,
            FailureKindArgument::ScenarioMismatch => Self::ScenarioMismatch,
            FailureKindArgument::StructuralViolation => Self::StructuralViolation,
            FailureKindArgument::Timeout => Self::Timeout,
            FailureKindArgument::ToolFailure => Self::ToolFailure,
        }
    }
}

impl From<PlatformEmulatorArgument> for ToolKind {
    fn from(value: PlatformEmulatorArgument) -> Self {
        match value {
            PlatformEmulatorArgument::Qemu => Self::Qemu,
            PlatformEmulatorArgument::Renode => Self::Renode,
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
            RecordCommand::Failure(arguments) => {
                let (capability, failure) = match arguments.capability {
                    FailureCapabilityArguments::Miri(arguments) => (
                        FailureCapability::Miri {
                            component: ComponentId::parse(arguments.component)?,
                            rustc_version: arguments.rustc_version,
                            miri_version: arguments.miri_version,
                        },
                        arguments.failure,
                    ),
                    FailureCapabilityArguments::Isa(arguments) => (
                        FailureCapability::TargetIsa {
                            architecture: ArchitectureId::parse(arguments.architecture)?,
                            cargo_version: arguments.cargo_version,
                            rustc_version: arguments.rustc_version,
                            qemu_version: arguments.qemu_version,
                            qemu_executable: arguments.qemu_executable,
                        },
                        arguments.failure,
                    ),
                    FailureCapabilityArguments::Platform(arguments) => (
                        FailureCapability::Platform {
                            platform: PlatformId::parse(arguments.platform)?,
                            cargo_version: arguments.cargo_version,
                            rustc_version: arguments.rustc_version,
                            linker_version: arguments.linker_version,
                            emulator: arguments.emulator.into(),
                            emulator_version: arguments.emulator_version,
                            emulator_executable: arguments.emulator_executable,
                        },
                        arguments.failure,
                    ),
                };
                let output = failure.output.clone();
                record_failure(
                    &root,
                    FailureRecordRequest {
                        capability,
                        scenario: ScenarioId::parse(failure.scenario)?,
                        runner: RunnerId::parse(failure.runner)?,
                        kind: failure.failure_kind.into(),
                        diagnostic: failure.diagnostic,
                        sources: failure.source,
                        logs: failure.log,
                        output: failure.output,
                    },
                )?;
                println!("EMBEDDED_FAILURE_PROOF: {}", output.display());
            }
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
            RecordCommand::Platform(arguments) => {
                let output = arguments.output.clone();
                record_platform_emulation(
                    &root,
                    PlatformRecordRequest {
                        platform: PlatformId::parse(arguments.platform)?,
                        scenario: ScenarioId::parse(arguments.scenario)?,
                        runner: RunnerId::parse(arguments.runner)?,
                        milestone: arguments.milestone.into(),
                        cargo_version: arguments.cargo_version,
                        rustc_version: arguments.rustc_version,
                        linker_version: arguments.linker_version,
                        emulator: arguments.emulator.into(),
                        emulator_version: arguments.emulator_version,
                        emulator_executable: arguments.emulator_executable,
                        sources: arguments.source,
                        transcript: arguments.transcript,
                        executable: arguments.executable,
                        logs: arguments.log,
                        output: arguments.output,
                    },
                )?;
                println!("EMBEDDED_PLATFORM_PROOF: {}", output.display());
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
