use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use thiserror::Error;

use crate::baseline::{self, BaselineError};
use crate::contract::MatrixStatus;
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
    RefreshBaseline(RefreshBaselineArguments),
    Summarize(SummarizeArguments),
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
    Summary(#[from] SummaryError),
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
        AssuranceCommand::RefreshBaseline(arguments) => {
            let outcome = baseline::refresh(&arguments.matrix, &baseline::path(&root))?;
            println!(
                "EMBEDDED_ASSURANCE_BASELINE: targets={} capabilities={} path={}",
                outcome.targets(),
                outcome.capabilities(),
                outcome.path().display(),
            );
        }
        AssuranceCommand::Summarize(arguments) => {
            let outcome =
                report::summarize(&arguments.resources, &arguments.proofs, &arguments.output)?;
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
