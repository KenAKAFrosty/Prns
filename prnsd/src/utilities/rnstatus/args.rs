use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, ValueEnum};
use personal_rns::identity::IdentityHash;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PositiveDuration(Duration);

impl PositiveDuration {
    pub const fn get(self) -> Duration {
        self.0
    }
}

fn parse_positive_duration(value: &str) -> Result<PositiveDuration, String> {
    let seconds = value
        .parse::<f64>()
        .map_err(|_| format!("{value:?} is not a number of seconds"))?;
    if !seconds.is_finite() || seconds <= 0.0 {
        return Err(format!(
            "{value:?} must be a finite number greater than zero"
        ));
    }
    Ok(PositiveDuration(Duration::from_secs_f64(seconds)))
}

fn parse_identity_hash(value: &str) -> Result<IdentityHash, String> {
    if value.len() != 32 {
        return Err(format!(
            "{value:?} must contain exactly 32 hexadecimal characters (16 bytes)"
        ));
    }
    let mut bytes = [0u8; 16];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let pair =
            std::str::from_utf8(pair).map_err(|_| format!("{value:?} is not hexadecimal"))?;
        bytes[index] =
            u8::from_str_radix(pair, 16).map_err(|_| format!("{value:?} is not hexadecimal"))?;
    }
    Ok(IdentityHash::new(bytes))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum RnstatusSort {
    #[value(alias = "bitrate")]
    Rate,
    Traffic,
    Rx,
    Tx,
    Rxs,
    Txs,
    #[value(alias = "announce")]
    Announces,
    Arx,
    Atx,
    Prx,
    Ptx,
    Held,
}

#[derive(Clone, Debug, PartialEq, Eq, Args)]
pub struct RnstatusArgs {
    #[arg(
        long,
        value_name = "DIR",
        help = "Use an alternate Reticulum config directory"
    )]
    pub config: Option<PathBuf>,

    #[arg(long, help = "Print the utility and compatibility version")]
    pub version: bool,

    #[arg(short = 'a', long = "all", help = "Show all interfaces")]
    pub show_all: bool,

    #[arg(short = 'A', long, help = "Show announce statistics")]
    pub announce_stats: bool,

    #[arg(short = 'P', long = "pr-stats", help = "Show path-request statistics")]
    pub path_request_stats: bool,

    #[arg(short = 'l', long, help = "Show link statistics")]
    pub link_stats: bool,

    #[arg(short = 'B', long, help = "Only show interfaces with active bursts")]
    pub burst: bool,

    #[arg(short = 't', long, help = "Display traffic totals")]
    pub totals: bool,

    #[arg(short = 's', long, value_enum, help = "Sort displayed interfaces")]
    pub sort: Option<RnstatusSort>,

    #[arg(short = 'r', long, help = "Reverse interface sorting")]
    pub reverse: bool,

    #[arg(short = 'j', long, help = "Emit the stock status shape as JSON")]
    pub json: bool,

    #[arg(short = 'R', value_name = "HASH", value_parser = parse_identity_hash, requires = "management_identity", help = "Query the transport with this 16-byte identity hash")]
    pub remote: Option<IdentityHash>,

    #[arg(
        short = 'i',
        value_name = "PATH",
        requires = "remote",
        help = "Identify remote management with this private identity file"
    )]
    pub management_identity: Option<PathBuf>,

    #[arg(short = 'w', value_name = "SECONDS", value_parser = parse_positive_duration, default_value = "15", help = "Give up on a remote query after this many seconds")]
    pub remote_timeout: PositiveDuration,

    #[arg(short = 'd', long, help = "List discovered interfaces")]
    pub discovered: bool,

    #[arg(
        short = 'D',
        help = "Show discovered-interface details and config entries"
    )]
    pub discovery_details: bool,

    #[arg(short = 'm', long, help = "Continuously refresh status")]
    pub monitor: bool,

    #[arg(short = 'I', long, value_name = "SECONDS", value_parser = parse_positive_duration, default_value = "1", help = "Refresh monitor mode at this interval")]
    pub monitor_interval: PositiveDuration,

    #[arg(short = 'v', long, action = clap::ArgAction::Count, help = "Print config warnings; repeat for CLI compatibility")]
    pub verbose: u8,

    #[arg(
        value_name = "FILTER",
        help = "Only show interface names containing this text"
    )]
    pub filter: Option<String>,
}

pub(super) enum RnstatusTarget<'a> {
    Local,
    Remote {
        transport_identity: IdentityHash,
        management_identity: &'a std::path::Path,
    },
}

impl RnstatusArgs {
    pub(super) fn target(&self) -> Result<RnstatusTarget<'_>, &'static str> {
        match (self.remote, self.management_identity.as_deref()) {
            (None, None) => Ok(RnstatusTarget::Local),
            (Some(transport_identity), Some(management_identity)) => Ok(RnstatusTarget::Remote {
                transport_identity,
                management_identity,
            }),
            (Some(_), None) => Err("-R requires a management identity path supplied with -i"),
            (None, Some(_)) => Err("-i requires a remote transport identity supplied with -R"),
        }
    }
}
