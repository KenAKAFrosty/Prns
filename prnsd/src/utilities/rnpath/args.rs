use std::path::{Path, PathBuf};

use clap::{ArgGroup, Args};
use personal_rns::identity::IdentityHash;
use personal_rns::wire::{DestinationHash, TransportId};

use super::super::arguments::{
    parse_hash_argument, parse_identity_hash, parse_positive_duration, PositiveDuration,
    RnsHashArgument,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlackholeDurationHours(f64);

impl Eq for BlackholeDurationHours {}

impl BlackholeDurationHours {
    pub const fn get(self) -> f64 {
        self.0
    }
}

fn parse_blackhole_duration(value: &str) -> Result<BlackholeDurationHours, String> {
    let hours = value
        .parse::<f64>()
        .map_err(|_| format!("{value:?} is not a number of hours"))?;
    if !hours.is_finite() {
        return Err(format!("{value:?} must be a finite number of hours"));
    }
    Ok(BlackholeDurationHours(hours))
}

#[derive(Clone, Debug, PartialEq, Eq, Args)]
#[command(group(
    ArgGroup::new("operation")
        .args([
            "table",
            "rates",
            "drop",
            "drop_announces",
            "drop_via",
            "blackholed",
            "blackhole",
            "unblackhole",
            "blackholed_list",
        ])
        .multiple(false)
))]
pub struct RnpathArgs {
    #[arg(
        long,
        value_name = "DIR",
        help = "Use an alternate Reticulum config directory"
    )]
    pub config: Option<PathBuf>,

    #[arg(long, help = "Print the utility and compatibility version")]
    pub version: bool,

    #[arg(short = 't', long, help = "Show all known paths")]
    pub table: bool,

    #[arg(
        short = 'm',
        long = "max",
        value_name = "HOPS",
        requires = "table",
        help = "Only show paths at or below this hop count"
    )]
    pub maximum_hops: Option<i64>,

    #[arg(short = 'r', long, help = "Show announce-rate information")]
    pub rates: bool,

    #[arg(short = 'd', long, help = "Remove the path to a destination")]
    pub drop: bool,

    #[arg(
        short = 'D',
        long = "drop-announces",
        help = "Drop all queued announces"
    )]
    pub drop_announces: bool,

    #[arg(
        short = 'x',
        long = "drop-via",
        help = "Drop all paths through a transport instance"
    )]
    pub drop_via: bool,

    #[arg(short = 'w', value_name = "SECONDS", value_parser = parse_positive_duration, help = "Give up on a path request after this many seconds (default: adaptive, with a 15-second floor)")]
    pub path_timeout: Option<PositiveDuration>,

    #[arg(short = 'R', value_name = "HASH", value_parser = parse_identity_hash, requires = "management_identity", help = "Inspect the transport with this 16-byte identity hash")]
    pub remote: Option<IdentityHash>,

    #[arg(
        short = 'i',
        value_name = "PATH",
        requires = "remote",
        help = "Identify remote management with this private identity file"
    )]
    pub management_identity: Option<PathBuf>,

    #[arg(short = 'W', value_name = "SECONDS", value_parser = parse_positive_duration, default_value = "15", help = "Give up on a remote query after this many seconds")]
    pub remote_timeout: PositiveDuration,

    #[arg(short = 'b', long, help = "List locally known blackholed identities")]
    pub blackholed: bool,

    #[arg(short = 'B', long, help = "Blackhole an identity")]
    pub blackhole: bool,

    #[arg(short = 'U', long, help = "Lift an identity blackhole")]
    pub unblackhole: bool,

    #[arg(long, value_name = "HOURS", value_parser = parse_blackhole_duration, requires = "blackhole", help = "Limit blackhole enforcement to this duration")]
    pub duration: Option<BlackholeDurationHours>,

    #[arg(
        long,
        requires = "blackhole",
        help = "Record a reason for blackholing the identity"
    )]
    pub reason: Option<String>,

    #[arg(
        short = 'p',
        long = "blackholed-list",
        help = "View a transport's published blackhole list"
    )]
    pub blackholed_list: bool,

    #[arg(short = 'j', long, help = "Emit path or rate data as JSON")]
    pub json: bool,

    #[arg(value_name = "DESTINATION", value_parser = parse_hash_argument)]
    pub destination: Option<RnsHashArgument>,

    #[arg(value_name = "LIST_FILTER")]
    pub list_filter: Option<String>,

    #[arg(short = 'v', long, action = clap::ArgAction::Count, help = "Print config warnings; repeat for CLI compatibility")]
    pub verbose: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RnpathOperation {
    Help,
    Table {
        destination: Option<DestinationHash>,
        maximum_hops: Option<i64>,
    },
    Rates {
        destination: Option<DestinationHash>,
    },
    DropPath(DestinationHash),
    DropAnnounces,
    DropVia(TransportId),
    ListBlackholes {
        filter: Option<String>,
    },
    Blackhole {
        identity: IdentityHash,
        duration: Option<BlackholeDurationHours>,
        reason: Option<String>,
    },
    Unblackhole(IdentityHash),
    PublishedBlackholes {
        source: IdentityHash,
        filter: Option<String>,
    },
    RequestPath(DestinationHash),
}

pub enum RnpathTarget<'a> {
    Local,
    Remote {
        transport_identity: IdentityHash,
        management_identity: &'a Path,
    },
}

impl RnpathArgs {
    pub fn operation(&self) -> Result<RnpathOperation, String> {
        if self.list_filter.is_some() && !self.blackholed_list {
            return Err("LIST_FILTER is only valid with --blackholed-list".into());
        }
        if self.table {
            return Ok(RnpathOperation::Table {
                destination: self.destination.map(RnsHashArgument::destination),
                maximum_hops: self.maximum_hops,
            });
        }
        if self.rates {
            return Ok(RnpathOperation::Rates {
                destination: self.destination.map(RnsHashArgument::destination),
            });
        }
        if self.drop {
            return required(self.destination, "--drop")
                .map(RnsHashArgument::destination)
                .map(RnpathOperation::DropPath);
        }
        if self.drop_announces {
            return Ok(RnpathOperation::DropAnnounces);
        }
        if self.drop_via {
            return required(self.destination, "--drop-via")
                .map(RnsHashArgument::transport)
                .map(RnpathOperation::DropVia);
        }
        if self.blackholed {
            return Ok(RnpathOperation::ListBlackholes {
                filter: self.destination.map(|hash| hex(hash.identity().as_bytes())),
            });
        }
        if self.blackhole {
            return required(self.destination, "--blackhole")
                .map(RnsHashArgument::identity)
                .map(|identity| RnpathOperation::Blackhole {
                    identity,
                    duration: self.duration,
                    reason: self.reason.clone(),
                });
        }
        if self.unblackhole {
            return required(self.destination, "--unblackhole")
                .map(RnsHashArgument::identity)
                .map(RnpathOperation::Unblackhole);
        }
        if self.blackholed_list {
            return required(self.destination, "--blackholed-list")
                .map(RnsHashArgument::identity)
                .map(|source| RnpathOperation::PublishedBlackholes {
                    source,
                    filter: self.list_filter.clone(),
                });
        }
        Ok(self
            .destination
            .map_or(RnpathOperation::Help, |destination| {
                RnpathOperation::RequestPath(destination.destination())
            }))
    }

    pub fn target(&self) -> Result<RnpathTarget<'_>, String> {
        match (self.remote, self.management_identity.as_deref()) {
            (None, None) => Ok(RnpathTarget::Local),
            (Some(transport_identity), Some(management_identity)) => Ok(RnpathTarget::Remote {
                transport_identity,
                management_identity,
            }),
            (Some(_), None) => {
                Err("-R requires a management identity path supplied with -i".into())
            }
            (None, Some(_)) => {
                Err("-i requires a remote transport identity supplied with -R".into())
            }
        }
    }
}

fn required(value: Option<RnsHashArgument>, operation: &str) -> Result<RnsHashArgument, String> {
    value.ok_or_else(|| format!("{operation} requires a 16-byte destination hash"))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
