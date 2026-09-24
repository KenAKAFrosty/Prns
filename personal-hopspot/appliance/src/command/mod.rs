use std::{
    fs,
    io::Read,
    num::{NonZeroU64, NonZeroU8},
    path::{Path, PathBuf},
    process::Command,
};

use clap::{Parser, Subcommand};
use personal_hopspot_appliance::{
    Appliance, Board, Budgets, CandidateId, Error, LaunchBudget, RadioProfileError, SpaceBudget,
    UnobservedWrites,
};

#[derive(Parser)]
pub struct Options {
    #[arg(long)]
    root: PathBuf,
    #[arg(long)]
    max_compressed_bytes: u64,
    #[arg(long)]
    max_executable_bytes: u64,
    #[arg(long)]
    flash_reserve_bytes: u64,
    #[arg(long)]
    ram_reserve_bytes: u64,
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    Inspect {
        #[arg(long)]
        profile: PathBuf,
    },
    Radio {
        #[command(subcommand)]
        action: radio::RadioAction,
    },
    RadioPlan {
        #[arg(long)]
        profile: PathBuf,
    },
    Status,
    Stage {
        #[arg(long)]
        package: PathBuf,
        #[arg(long)]
        trial_launches: NonZeroU8,
    },
    Confirm {
        #[arg(long)]
        revision: NonZeroU64,
        #[arg(long)]
        executable_sha256: String,
    },
    Rollback,
    Run {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        ram_directory: PathBuf,
    },
    Qualify {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        ram_directory: PathBuf,
        #[arg(long)]
        controller_public_key: launch::ControllerPublicKey,
        #[arg(long, value_enum)]
        controller_access: launch::ControllerAccess,
        #[arg(long)]
        run_for: launch::QualificationWindow,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error(transparent)]
    Appliance(#[from] Error),
    #[error(transparent)]
    Package(#[from] personal_hopspot_appliance::PackageError),
    #[error("host I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("configuration or output: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid explicit launch configuration")]
    Configuration,
    #[error(transparent)]
    RadioProfile(#[from] RadioProfileError),
    #[error(transparent)]
    RadioTransaction(#[from] personal_hopspot_appliance::RadioTransactionError),
    #[error("named radio/interface sections do not match the Morse binding")]
    RadioBinding,
    #[error("pending UCI changes must be resolved before radio planning")]
    PendingUciChanges,
    #[error("could not inspect filesystem capacity")]
    Capacity,
    #[cfg(not(unix))]
    #[error("this launcher requires Unix exec")]
    UnsupportedHost,
}

pub fn run(options: Options, public_key: &str) -> Result<(), CommandError> {
    run_with_radio_observer(options, public_key, UnobservedWrites)
}

pub fn run_with_radio_observer(
    options: Options,
    public_key: &str,
    observer: impl personal_hopspot_appliance::ObserveRadioWrites,
) -> Result<(), CommandError> {
    if !options.root.is_absolute() {
        return Err(CommandError::Configuration);
    }
    let board = Board::from_vendor_name(&fs::read_to_string("/tmp/sysinfo/board_name")?)?;
    if let Action::Inspect { profile } = &options.action {
        return inspection::print(&options, &board, profile, public_key);
    }
    if let Action::Radio { action } = options.action {
        return radio::run(
            action,
            radio::RadioContext {
                manager_root: &options.root,
                flash_reserve_bytes: options.flash_reserve_bytes,
                board,
            },
            observer,
        );
    }
    if let Action::RadioPlan { profile } = &options.action {
        radio::print_plan(profile, &board)?;
        return Ok(());
    }
    let mut appliance = Appliance::open(
        &options.root,
        public_key,
        board,
        Budgets {
            compressed_bytes: options.max_compressed_bytes,
            executable_bytes: options.max_executable_bytes,
        },
        UnobservedWrites,
    )?;
    match options.action {
        Action::Inspect { .. } => unreachable!("inspection returned before opening slots"),
        Action::Radio { .. } => {
            unreachable!("radio commands returned before opening application slots")
        }
        Action::RadioPlan { .. } => unreachable!("radio planning returned before opening slots"),
        Action::Status => println!("{}", serde_json::to_string(appliance.activation())?),
        Action::Stage {
            package,
            trial_launches,
        } => {
            let space = filesystem_space(&options.root, options.flash_reserve_bytes)?;
            let verified = appliance.verify_directory(&package)?;
            let candidate = appliance.stage(verified, LaunchBudget::new(trial_launches), space)?;
            println!("{}", serde_json::to_string(&candidate)?);
        }
        Action::Confirm {
            revision,
            executable_sha256,
        } => {
            appliance.confirm(&CandidateId {
                revision,
                executable_sha256: executable_sha256
                    .try_into()
                    .map_err(|_| CommandError::Configuration)?,
            })?;
            println!("{}", serde_json::to_string(appliance.activation())?);
        }
        Action::Rollback => {
            appliance.rollback()?;
            println!("{}", serde_json::to_string(appliance.activation())?);
        }
        Action::Run {
            config,
            ram_directory,
        } => launch::execute(
            appliance,
            launch::LaunchInputs {
                root: &options.root,
                config: &config,
                ram_directory: &ram_directory,
                ram_reserve_bytes: options.ram_reserve_bytes,
            },
            launch::Purpose::Service,
        )?,
        Action::Qualify {
            config,
            ram_directory,
            controller_public_key,
            controller_access,
            run_for,
        } => launch::execute(
            appliance,
            launch::LaunchInputs {
                root: &options.root,
                config: &config,
                ram_directory: &ram_directory,
                ram_reserve_bytes: options.ram_reserve_bytes,
            },
            launch::Purpose::Qualification {
                controller: controller_public_key,
                access: controller_access,
                window: run_for,
            },
        )?,
    }
    Ok(())
}

fn filesystem_space(path: &Path, reserve_bytes: u64) -> Result<SpaceBudget, CommandError> {
    let result = Command::new("/bin/df").arg("-Pk").arg(path).output()?;
    if !result.status.success() {
        return Err(CommandError::Capacity);
    }
    let output = String::from_utf8(result.stdout).map_err(|_| CommandError::Capacity)?;
    let row = output.lines().last().ok_or(CommandError::Capacity)?;
    let kib = row
        .split_whitespace()
        .nth(3)
        .ok_or(CommandError::Capacity)?
        .parse::<u64>()
        .map_err(|_| CommandError::Capacity)?;
    let available_bytes = kib.checked_mul(1024).ok_or(CommandError::Capacity)?;
    Ok(SpaceBudget {
        available_bytes,
        reserve_bytes,
    })
}

mod inspection;
mod launch;
mod radio;

#[cfg(test)]
mod tests;
