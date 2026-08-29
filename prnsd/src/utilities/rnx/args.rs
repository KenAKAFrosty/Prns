use std::path::PathBuf;

use clap::Args;
use personal_rns::identity::IdentityHash;

use super::super::arguments::{
    parse_hash_argument, parse_identity_hash, parse_nonnegative_duration, NonnegativeDuration,
    RnsHashArgument,
};

#[derive(Clone, Debug, PartialEq, Eq, Args)]
pub struct RnxArgs {
    #[arg(value_name = "DESTINATION", value_parser = parse_hash_argument)]
    pub destination: Option<RnsHashArgument>,

    #[arg(value_name = "COMMAND")]
    pub command: Option<String>,

    #[arg(long, value_name = "DIR")]
    pub config: Option<PathBuf>,

    #[arg(short = 'v', long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    #[arg(short = 'q', long, action = clap::ArgAction::Count)]
    pub quiet: u8,

    #[arg(short = 'p', long)]
    pub print_identity: bool,

    #[arg(short = 'l', long)]
    pub listen: bool,

    #[arg(short = 'i', value_name = "IDENTITY")]
    pub identity: Option<PathBuf>,

    #[arg(short = 'x', long)]
    pub interactive: bool,

    #[arg(short = 'b', long)]
    pub no_announce: bool,

    #[arg(short = 'a', value_name = "ALLOWED_HASH", value_parser = parse_identity_hash)]
    pub allowed: Vec<IdentityHash>,

    #[arg(short = 'n', long = "noauth")]
    pub no_auth: bool,

    #[arg(short = 'N', long = "noid")]
    pub no_id: bool,

    #[arg(short = 'd', long)]
    pub detailed: bool,

    #[arg(short = 'm')]
    pub mirror: bool,

    #[arg(short = 'w', value_name = "SECONDS", value_parser = parse_nonnegative_duration, help = "Set the command and connection timeout (path discovery defaults to adaptive with a 15-second floor)")]
    pub timeout: Option<NonnegativeDuration>,

    #[arg(short = 'W', value_name = "SECONDS", value_parser = parse_nonnegative_duration)]
    pub result_timeout: Option<NonnegativeDuration>,

    #[arg(long)]
    pub stdin: Option<String>,

    #[arg(long, value_name = "BYTES")]
    pub stdout: Option<u64>,

    #[arg(long, value_name = "BYTES")]
    pub stderr: Option<u64>,

    #[arg(long)]
    pub version: bool,
}
