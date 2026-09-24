//! Separate lab executable; never included in the application installation bundle.

#[path = "../src/command/mod.rs"]
mod command;

use clap::Parser;
use personal_hopspot_appliance::{ObserveRadioWrites, RadioCheckpoint, RadioFile};

mod power_cut;

#[derive(Parser)]
struct LabOptions {
    #[arg(long)]
    qualification_public_key: std::path::PathBuf,
    #[arg(long, value_enum, default_value = "uninterrupted")]
    qualification_radio_observation: RadioObservation,
    #[command(flatten)]
    options: command::Options,
}

#[derive(Clone, clap::ValueEnum)]
enum RadioObservation {
    Uninterrupted,
    PauseAfterWirelessReplacement,
}

fn main() -> std::process::ExitCode {
    let options = LabOptions::parse();
    let outcome = std::fs::read_to_string(options.qualification_public_key)
        .map_err(command::CommandError::from)
        .and_then(|key| match options.qualification_radio_observation {
            RadioObservation::Uninterrupted => command::run(options.options, &key),
            RadioObservation::PauseAfterWirelessReplacement => command::run_with_radio_observer(
                options.options,
                &key,
                power_cut::AwaitElectricalCut,
            ),
        });
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("qualification_error {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
