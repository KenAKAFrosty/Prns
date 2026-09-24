#![forbid(unsafe_code)]

mod command;

use clap::Parser;

fn main() -> std::process::ExitCode {
    match command::run(
        command::Options::parse(),
        prns_flash_manifest::PINNED_MINISIGN_PUBLIC_KEY,
    ) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("appliance_error {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
