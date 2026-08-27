use std::path::PathBuf;

use clap::Args;
use personal_rns::identity::IdentityHash;

use super::super::arguments::{
    parse_hash_argument, parse_identity_hash, parse_positive_duration, PositiveDuration,
    RnsHashArgument,
};

#[derive(Clone, Debug, PartialEq, Eq, Args)]
pub struct RncpArgs {
    #[arg(value_name = "FILE")]
    pub file: Option<String>,

    #[arg(value_name = "DESTINATION", value_parser = parse_hash_argument)]
    pub destination: Option<RnsHashArgument>,

    #[arg(long, value_name = "DIR")]
    pub config: Option<PathBuf>,

    #[arg(short = 'v', long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    #[arg(short = 'q', long, action = clap::ArgAction::Count)]
    pub quiet: u8,

    #[arg(short = 'S', long)]
    pub silent: bool,

    #[arg(short = 'l', long)]
    pub listen: bool,

    #[arg(short = 'C', long)]
    pub no_compress: bool,

    #[arg(short = 'F', long)]
    pub allow_fetch: bool,

    #[arg(short = 'f', long)]
    pub fetch: bool,

    #[arg(short = 'j', long, value_name = "PATH")]
    pub jail: Option<PathBuf>,

    #[arg(short = 's', long, value_name = "PATH")]
    pub save: Option<PathBuf>,

    #[arg(short = 'O', long)]
    pub overwrite: bool,

    #[arg(short = 'b', value_name = "SECONDS", default_value_t = -1)]
    pub announce: i64,

    #[arg(short = 'a', value_name = "ALLOWED_HASH", value_parser = parse_identity_hash)]
    pub allowed: Vec<IdentityHash>,

    #[arg(short = 'n', long)]
    pub no_auth: bool,

    #[arg(short = 'p', long)]
    pub print_identity: bool,

    #[arg(short = 'i', value_name = "IDENTITY")]
    pub identity: Option<PathBuf>,

    #[arg(short = 'w', value_name = "SECONDS", value_parser = parse_positive_duration, help = "Give up after this many seconds (default: adaptive, with a 15-second path floor)")]
    pub timeout: Option<PositiveDuration>,

    #[arg(short = 'P', long)]
    pub phy_rates: bool,

    #[arg(long)]
    pub version: bool,
}
