use std::path::PathBuf;

use clap::{Args, Subcommand};
use prns_config::editing::{InterfaceName, RNodeMultiRadioDefinition};
use prns_config::InterfaceKind;

#[derive(Clone, Debug, PartialEq, Args)]
pub struct InterfacesArgs {
    #[arg(long, global = true, value_name = "DIR")]
    pub config: Option<PathBuf>,

    #[arg(long, global = true)]
    pub show_secrets: bool,

    #[command(subcommand)]
    pub command: Option<InterfacesCommand>,
}

#[derive(Clone, Debug, PartialEq, Subcommand)]
pub enum InterfacesCommand {
    List,
    #[command(name = "validate", visible_alias = "check")]
    Validate(ValidateArgs),
    Add(AddArgs),
    Edit(EditArgs),
    Enable(NameArgs),
    Disable(NameArgs),
    Remove(RemoveArgs),
    Repair(RepairArgs),
    Apply,
}

#[derive(Clone, Debug, Default, PartialEq, Args)]
pub struct ValidateArgs {
    #[arg(long)]
    pub details: bool,
}

#[derive(Clone, Debug, PartialEq, Args)]
pub struct MutationArgs {
    #[arg(long)]
    pub dry_run: bool,

    #[arg(long)]
    pub apply: bool,
}

#[derive(Clone, Debug, PartialEq, Args)]
pub struct AddArgs {
    #[arg(value_name = "TYPE", value_parser = parse_interface_kind)]
    pub kind: Option<InterfaceKind>,

    #[arg(long)]
    pub name: Option<String>,

    #[arg(long)]
    pub disabled: bool,

    #[command(flatten)]
    pub options: InterfaceOptions,

    #[command(flatten)]
    pub mutation: MutationArgs,
}

#[derive(Clone, Debug, PartialEq, Args)]
pub struct EditArgs {
    #[arg(value_name = "NAME")]
    pub name: Option<String>,

    #[arg(long)]
    pub rename: Option<String>,

    #[command(flatten)]
    pub options: InterfaceOptions,

    #[command(flatten)]
    pub mutation: MutationArgs,
}

#[derive(Clone, Debug, PartialEq, Args)]
pub struct NameArgs {
    #[arg(value_name = "NAME")]
    pub name: Option<String>,

    #[command(flatten)]
    pub mutation: MutationArgs,
}

#[derive(Clone, Debug, PartialEq, Args)]
pub struct RemoveArgs {
    #[arg(value_name = "NAME")]
    pub name: Option<String>,

    #[arg(long)]
    pub yes: bool,

    #[command(flatten)]
    pub mutation: MutationArgs,
}

#[derive(Clone, Debug, PartialEq, Args)]
pub struct RepairArgs {
    #[arg(long)]
    pub safe: bool,

    #[command(flatten)]
    pub mutation: MutationArgs,
}

#[derive(Clone, Debug, Default, PartialEq, Args)]
pub struct InterfaceOptions {
    #[arg(long)]
    pub mode: Option<String>,
    #[arg(long)]
    pub outgoing: Option<bool>,
    #[arg(long, value_name = "BPS")]
    pub bitrate: Option<u64>,
    #[arg(long)]
    pub gravity: Option<i64>,
    #[arg(long)]
    pub announce_cap: Option<f64>,
    #[arg(long)]
    pub announce_rate_target: Option<u64>,
    #[arg(long)]
    pub announce_rate_grace: Option<u64>,
    #[arg(long)]
    pub announce_rate_penalty: Option<u64>,
    #[arg(long)]
    pub network_name: Option<String>,
    #[arg(long)]
    pub pass_phrase: Option<String>,
    #[arg(long)]
    pub ifac_size: Option<u64>,
    #[arg(long)]
    pub discoverable: Option<bool>,
    #[arg(long)]
    pub announce_interval: Option<i64>,
    #[arg(long)]
    pub discovery_stamp_value: Option<u8>,
    #[arg(long)]
    pub discovery_name: Option<String>,
    #[arg(long)]
    pub discovery_encrypt: Option<bool>,
    #[arg(long)]
    pub reachable_on: Option<String>,
    #[arg(long, value_name = "PORT")]
    pub reachable_port: Option<u16>,
    #[arg(long)]
    pub publish_ifac: Option<bool>,
    #[arg(long)]
    pub latitude: Option<f64>,
    #[arg(long)]
    pub longitude: Option<f64>,
    #[arg(long)]
    pub height: Option<f64>,
    #[arg(long, value_name = "HERTZ")]
    pub discovery_frequency: Option<u64>,
    #[arg(long, value_name = "HERTZ")]
    pub discovery_bandwidth: Option<u32>,
    #[arg(long)]
    pub discovery_modulation: Option<String>,
    #[arg(long)]
    pub ingress_control: Option<bool>,
    #[arg(long)]
    pub egress_control: Option<bool>,
    #[arg(long)]
    pub ic_max_held_announces: Option<i64>,
    #[arg(long)]
    pub ic_burst_hold: Option<f64>,
    #[arg(long)]
    pub ic_burst_freq_new: Option<f64>,
    #[arg(long)]
    pub ic_burst_freq: Option<f64>,
    #[arg(long)]
    pub ic_pr_burst_freq_new: Option<f64>,
    #[arg(long)]
    pub ic_pr_burst_freq: Option<f64>,
    #[arg(long)]
    pub ec_pr_freq: Option<f64>,
    #[arg(long)]
    pub ic_new_time: Option<f64>,
    #[arg(long)]
    pub ic_burst_penalty: Option<f64>,
    #[arg(long)]
    pub ic_held_release_interval: Option<f64>,
    #[arg(long)]
    pub bootstrap_only: Option<bool>,
    #[arg(long)]
    pub recursive_prs: Option<bool>,
    #[arg(long)]
    pub announces_from_internal: Option<bool>,
    #[arg(long)]
    pub ignore_config_warnings: Option<bool>,
    #[arg(long)]
    pub group_id: Option<String>,
    #[arg(long)]
    pub discovery_scope: Option<String>,
    #[arg(long)]
    pub discovery_port: Option<u16>,
    #[arg(long)]
    pub data_port: Option<u16>,
    #[arg(long, value_delimiter = ',')]
    pub devices: Option<Vec<String>>,
    #[arg(long, value_delimiter = ',')]
    pub ignored_devices: Option<Vec<String>>,
    #[arg(long)]
    pub multicast_address_type: Option<String>,
    #[arg(long)]
    pub target_host: Option<String>,
    #[arg(long)]
    pub target_port: Option<u16>,
    #[arg(long)]
    pub target: Option<String>,
    #[arg(long)]
    pub kiss_framing: Option<bool>,
    #[arg(long)]
    pub i2p_tunneled: Option<bool>,
    #[arg(long)]
    pub listen_ip: Option<String>,
    #[arg(long)]
    pub listen_port: Option<u16>,
    #[arg(long)]
    pub forward_ip: Option<String>,
    #[arg(long)]
    pub forward_port: Option<u16>,
    #[arg(long)]
    pub device: Option<String>,
    #[arg(long)]
    pub port: Option<String>,
    #[arg(
        long = "radio",
        value_name = "NAME:VPORT:FREQUENCY_HZ:BANDWIDTH_HZ:TX_POWER_DBM:SPREADING_FACTOR:CODING_RATE_DENOMINATOR",
        value_parser = parse_rnode_multi_radio
    )]
    pub rnode_multi_radios: Vec<RNodeMultiRadioDefinition>,
    #[arg(long)]
    pub prefer_ipv6: Option<bool>,
    #[arg(long, value_name = "BPS")]
    pub speed: Option<u32>,
    #[arg(long)]
    pub databits: Option<u8>,
    #[arg(long)]
    pub parity: Option<String>,
    #[arg(long)]
    pub stopbits: Option<u8>,
    #[arg(long)]
    pub flow_control: Option<bool>,
    #[arg(long)]
    pub preamble: Option<u32>,
    #[arg(long)]
    pub txtail: Option<u32>,
    #[arg(long)]
    pub persistence: Option<u32>,
    #[arg(long)]
    pub slottime: Option<u32>,
    #[arg(long)]
    pub id_callsign: Option<String>,
    #[arg(long)]
    pub id_interval: Option<u64>,
    #[arg(long)]
    pub callsign: Option<String>,
    #[arg(long)]
    pub ssid: Option<u8>,
    #[arg(long, value_name = "HERTZ")]
    pub frequency: Option<u64>,
    #[arg(long, value_name = "HERTZ")]
    pub bandwidth: Option<u32>,
    #[arg(long)]
    pub spreading_factor: Option<u8>,
    #[arg(long)]
    pub coding_rate: Option<u8>,
    #[arg(long, value_name = "DBM")]
    pub txpower: Option<i16>,
    #[arg(long)]
    pub airtime_limit_short: Option<f64>,
    #[arg(long)]
    pub airtime_limit_long: Option<f64>,
    #[arg(long)]
    pub command: Option<String>,
    #[arg(long, value_name = "SECONDS")]
    pub respawn_delay: Option<f64>,
    #[arg(long, value_delimiter = ',')]
    pub peers: Option<Vec<String>>,
    #[arg(long)]
    pub connectable: Option<bool>,
    #[arg(long, value_name = "SECONDS")]
    pub connect_timeout: Option<u64>,
    #[arg(long)]
    pub max_reconnect_tries: Option<u32>,
    #[arg(long, value_name = "BYTES")]
    pub fixed_mtu: Option<u16>,
    #[arg(long)]
    pub remote: Option<String>,
    #[arg(long)]
    pub listen_on: Option<String>,
}

fn parse_interface_kind(value: &str) -> Result<InterfaceKind, String> {
    InterfaceKind::parse_cli(value).ok_or_else(|| {
        format!(
            "unknown interface type {value:?}; use one of: {}",
            InterfaceKind::CANONICAL_NAMES.join(", ")
        )
    })
}

fn parse_rnode_multi_radio(value: &str) -> Result<RNodeMultiRadioDefinition, String> {
    let fields = value.split(':').collect::<Vec<_>>();
    let [name, vport, frequency, bandwidth, txpower, spreading_factor, coding_rate] =
        fields.as_slice()
    else {
        return Err(
            "expected NAME:VPORT:FREQUENCY_HZ:BANDWIDTH_HZ:TX_POWER_DBM:SPREADING_FACTOR:CODING_RATE_DENOMINATOR".to_string(),
        );
    };
    let name = InterfaceName::new(*name).map_err(|error| error.to_string())?;
    let vport = vport
        .parse::<u8>()
        .map_err(|_| "VPORT must be an unsigned 8-bit integer".to_string())?;
    let frequency = frequency
        .parse::<u64>()
        .map_err(|_| "FREQUENCY_HZ must be an unsigned 64-bit integer".to_string())?;
    let bandwidth = bandwidth
        .parse::<u32>()
        .map_err(|_| "BANDWIDTH_HZ must be an unsigned 32-bit integer".to_string())?;
    let txpower = txpower
        .parse::<i16>()
        .map_err(|_| "TX_POWER_DBM must be a signed 16-bit integer".to_string())?;
    let spreading_factor = spreading_factor
        .parse::<u8>()
        .map_err(|_| "SPREADING_FACTOR must be an unsigned 8-bit integer".to_string())?;
    let coding_rate = coding_rate
        .parse::<u8>()
        .map_err(|_| "CODING_RATE_DENOMINATOR must be an unsigned 8-bit integer".to_string())?;
    RNodeMultiRadioDefinition::new(
        name,
        vport,
        frequency,
        bandwidth,
        txpower,
        spreading_factor,
        coding_rate,
    )
    .map_err(|error| error.to_string())
}
