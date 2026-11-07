use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, Subcommand, ValueEnum};
use personal_rns::identity::IDENTITY_PUBLIC_KEY_LEN;
use personal_rns::interfaces::subghz::{RegulatoryRegion, SubGRegion};

#[derive(Clone, Debug, PartialEq, Eq, Args)]
pub struct BoardArgs {
    #[command(subcommand)]
    pub command: BoardCommand,
}

impl BoardArgs {
    pub fn config(&self) -> Option<PathBuf> {
        match &self.command {
            BoardCommand::Identity(options) => options.config.clone(),
            #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
            BoardCommand::Enrol(options) => options.config.clone(),
            BoardCommand::Lora(options) => options.config.clone(),
            BoardCommand::Status(options) => options.config.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Subcommand)]
pub enum BoardCommand {
    #[command(about = "Print the controller identity this daemon enrols on boards")]
    Identity(BoardIdentityArgs),
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    #[command(about = "Enrol this daemon's controller identity on the attached Prns board")]
    Enrol(BoardEnrolArgs),
    #[command(about = "Apply a region's auto LoRa profile to a Prns board")]
    Lora(BoardLoraArgs),
    #[command(about = "Show the interfaces a Prns board reports")]
    Status(BoardStatusArgs),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Args)]
pub struct BoardIdentityArgs {
    #[arg(long, value_name = "DIR")]
    pub config: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Args)]
pub struct BoardEnrolArgs {
    #[arg(long, value_name = "DIR")]
    pub config: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq, Args)]
pub struct BoardLoraArgs {
    #[arg(value_name = "REGION", value_enum)]
    pub region: BoardRegion,

    #[arg(
        long,
        value_name = "PUBLIC_KEY",
        value_parser = parse_public_key,
        help = "Board target key to address; defaults to the last enrolled board"
    )]
    pub target_key: Option<[u8; IDENTITY_PUBLIC_KEY_LEN]>,

    #[arg(
        long,
        value_parser = parse_path_timeout,
        default_value = "60",
        help = "Seconds to wait for a path to the board"
    )]
    pub path_timeout: Duration,

    #[arg(long, value_name = "DIR")]
    pub config: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq, Args)]
pub struct BoardStatusArgs {
    #[arg(
        long,
        value_name = "PUBLIC_KEY",
        value_parser = parse_public_key,
        help = "Board target key to address; defaults to the last enrolled board"
    )]
    pub target_key: Option<[u8; IDENTITY_PUBLIC_KEY_LEN]>,

    #[arg(
        long,
        value_parser = parse_path_timeout,
        default_value = "60",
        help = "Seconds to wait for a path to the board"
    )]
    pub path_timeout: Duration,

    #[arg(long, value_name = "DIR")]
    pub config: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum BoardRegion {
    Us915,
    Au915,
    Eu433,
    Eu865,
    Eu868,
    Eu869,
    As923,
    In865,
    Cn470,
    Kr920,
    Jp920,
}

impl BoardRegion {
    pub const fn region(self) -> SubGRegion {
        SubGRegion::Regulated(match self {
            Self::Us915 => RegulatoryRegion::Us915,
            Self::Au915 => RegulatoryRegion::Au915,
            Self::Eu433 => RegulatoryRegion::Eu433,
            Self::Eu865 => RegulatoryRegion::Eu865,
            Self::Eu868 => RegulatoryRegion::Eu868,
            Self::Eu869 => RegulatoryRegion::Eu869,
            Self::As923 => RegulatoryRegion::As923,
            Self::In865 => RegulatoryRegion::In865,
            Self::Cn470 => RegulatoryRegion::Cn470,
            Self::Kr920 => RegulatoryRegion::Kr920,
            Self::Jp920 => RegulatoryRegion::Jp920,
        })
    }
}

fn parse_path_timeout(value: &str) -> Result<Duration, String> {
    let seconds = value
        .parse::<f64>()
        .map_err(|_| format!("{value:?} is not a number of seconds"))?;
    if !seconds.is_finite() || seconds <= 0.0 {
        return Err(format!(
            "{value:?} must be a finite number greater than zero"
        ));
    }
    Ok(Duration::from_secs_f64(seconds))
}

fn parse_public_key(value: &str) -> Result<[u8; IDENTITY_PUBLIC_KEY_LEN], String> {
    if value.len() != IDENTITY_PUBLIC_KEY_LEN * 2 {
        return Err(format!(
            "{value:?} must contain {} hexadecimal characters ({} bytes)",
            IDENTITY_PUBLIC_KEY_LEN * 2,
            IDENTITY_PUBLIC_KEY_LEN
        ));
    }
    let mut bytes = [0u8; IDENTITY_PUBLIC_KEY_LEN];
    for (index, pair) in value.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        let pair =
            std::str::from_utf8(pair).map_err(|_| format!("{value:?} is not hexadecimal"))?;
        bytes[index] =
            u8::from_str_radix(pair, 16).map_err(|_| format!("{value:?} is not hexadecimal"))?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn board_region_names_map_to_regulated_regions() {
        assert_eq!(
            BoardRegion::Us915.region(),
            SubGRegion::Regulated(RegulatoryRegion::Us915)
        );
        assert_eq!(
            BoardRegion::Jp920.region(),
            SubGRegion::Regulated(RegulatoryRegion::Jp920)
        );
    }

    #[test]
    fn public_key_parser_needs_a_complete_key() {
        let key = "ab".repeat(IDENTITY_PUBLIC_KEY_LEN);
        assert_eq!(parse_public_key(&key).map(|bytes| bytes[0]), Ok(0xab));
        assert!(parse_public_key("ab").is_err());
        assert!(parse_public_key(&"zz".repeat(IDENTITY_PUBLIC_KEY_LEN)).is_err());
    }

    #[test]
    fn path_timeout_rejects_a_non_positive_duration() {
        assert_eq!(parse_path_timeout("60"), Ok(Duration::from_secs(60)));
        assert!(parse_path_timeout("0").is_err());
        assert!(parse_path_timeout("-3").is_err());
    }
}
