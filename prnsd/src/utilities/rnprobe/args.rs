use std::path::PathBuf;

use clap::Args;
use personal_rns::routing::announce::{derive_single_destination_hash, ExpandNameError};
use personal_rns::wire::DestinationHash;

use super::super::arguments::{
    parse_hash_argument, parse_nonnegative_duration, parse_positive_duration, NonnegativeDuration,
    PositiveDuration, RnsHashArgument,
};

#[derive(Clone, Debug, PartialEq, Eq, Args)]
pub struct RnprobeArgs {
    #[arg(
        long,
        value_name = "DIR",
        help = "Use an alternate Reticulum config directory"
    )]
    pub config: Option<PathBuf>,

    #[arg(short = 's', long, value_name = "BYTES", default_value_t = 16)]
    pub size: usize,

    #[arg(short = 'n', long, value_name = "COUNT", default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
    pub probes: u32,

    #[arg(short = 't', long, value_name = "SECONDS", value_parser = parse_positive_duration, help = "Give up after this many seconds (default: adaptive, with a 15-second path floor)")]
    pub timeout: Option<PositiveDuration>,

    #[arg(short = 'w', long, value_name = "SECONDS", value_parser = parse_nonnegative_duration, default_value = "0")]
    pub wait: NonnegativeDuration,

    #[arg(long, help = "Print the utility and compatibility version")]
    pub version: bool,

    #[arg(value_name = "FULL_NAME")]
    pub full_name: Option<String>,

    #[arg(value_name = "DESTINATION_HASH", value_parser = parse_hash_argument)]
    pub destination: Option<RnsHashArgument>,

    #[arg(short = 'v', long, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeTarget<'a> {
    Help,
    MissingName,
    Destination {
        full_name: &'a str,
        destination: DestinationHash,
    },
}

impl RnprobeArgs {
    pub fn target(&self) -> ProbeTarget<'_> {
        match (&self.full_name, self.destination) {
            (None, None) => ProbeTarget::Help,
            (None, Some(_)) => ProbeTarget::MissingName,
            (Some(_), None) => ProbeTarget::Help,
            (Some(full_name), Some(destination)) => ProbeTarget::Destination {
                full_name,
                destination: destination.destination(),
            },
        }
    }
}

pub fn destination_matches_name(
    identity: personal_rns::identity::IdentityHash,
    full_name: &str,
    destination: DestinationHash,
) -> Result<bool, ExpandNameError> {
    let mut components = full_name.split('.');
    let app_name = components.next().unwrap_or_default();
    let aspects: Vec<_> = components.collect();
    derive_single_destination_hash(&identity, app_name, &aspects)
        .map(|derived| derived == destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    use personal_rns::identity::IdentityHash;

    #[test]
    fn destination_name_validation_uses_the_recalled_identity() {
        let identity = IdentityHash::new([0x42; 16]);
        let destination =
            derive_single_destination_hash(&identity, "rnstransport", &["probe"]).unwrap();
        assert_eq!(
            destination_matches_name(identity, "rnstransport.probe", destination),
            Ok(true)
        );
        assert_eq!(
            destination_matches_name(identity, "rnstransport.other", destination),
            Ok(false)
        );
    }
}
