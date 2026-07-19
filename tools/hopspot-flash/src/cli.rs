use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::boards::BoardId;

#[derive(Parser)]
#[command(
    name = "hopspot-flash",
    about = "Interactive firmware flasher for Personal Hopspot boards.",
    long_about = "Run without a subcommand for a guided board flashing flow."
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Option<CommandMode>,
}

#[derive(Subcommand)]
pub(crate) enum CommandMode {
    /// List boards known to the helper.
    List,
    /// Build a hosted docs firmware artifact.
    #[command(hide = true)]
    Build {
        #[arg(value_enum)]
        board: BoardId,
        #[arg(long, value_name = "DIR")]
        out_root: Option<PathBuf>,
    },
    /// Flash Hopspot firmware to the board.
    Flash {
        #[arg(value_enum)]
        board: BoardId,
        #[arg(long, value_name = "PORT", help = "Serial port for ESP boards")]
        port: Option<String>,
        #[arg(
            long,
            value_name = "SSID",
            help = "Explicit Wi-Fi SSID to write into Hopspot config"
        )]
        wifi_ssid: Option<String>,
        #[arg(
            long,
            value_name = "PASSWORD",
            help = "Explicit Wi-Fi password to write into Hopspot config"
        )]
        wifi_password: Option<String>,
        #[arg(
            long,
            help = "Load Wi-Fi Auto credentials from HOPSPOT_WIFI_* or .wifi-env"
        )]
        wifi_from_env: bool,
        #[arg(
            long,
            help = "Explicitly clear/omit Wi-Fi Auto credentials for this flash"
        )]
        no_wifi_creds: bool,
        #[arg(long, help = "Open espflash monitor after flashing ESP boards")]
        monitor: bool,
        #[arg(long, value_name = "DIR", help = "Mounted TECHOBOOT directory")]
        mount: Option<PathBuf>,
    },
    /// Show board-specific flashing steps.
    Steps {
        #[arg(value_enum)]
        board: BoardId,
    },
}
