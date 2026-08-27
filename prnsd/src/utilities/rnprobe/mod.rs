mod args;

use std::fmt;
use std::time::Duration;

pub use args::RnprobeArgs;
use args::{destination_matches_name, ProbeTarget};
use personal_rns::engine::{
    DeliveryEvidence, SendSinglePacketFailure, MAX_SEND_SINGLE_PACKET_PLAINTEXT_LEN,
};
use personal_rns::node_introspection::NodeIntrospection;
use personal_rns::runtime::SendError;
use personal_rns::shared_instance::{SharedInstancePacketPhyStats, SharedInstanceRpcClientError};
use personal_rns::wire::DestinationHash;

use super::configuration::{LoadedConfiguration, UtilityConfigurationError};
use super::session::{
    UtilityNodeClient, UtilityNodeIdentity, UtilityNodeSession, UtilityNodeSessionError,
    UtilityNodeStopped, UtilityPathError,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(12);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeOutcome {
    exit_code: u8,
}

impl ProbeOutcome {
    pub const fn exit_code(self) -> u8 {
        self.exit_code
    }
}

pub async fn run(args: RnprobeArgs) -> Result<ProbeOutcome, RnprobeError> {
    if args.version {
        println!(
            "prnsd probe {} (RNS 1.4.2 compatibility)",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(ProbeOutcome { exit_code: 0 });
    }
    let (full_name, destination) = match args.target() {
        ProbeTarget::Help => {
            print!("{}", crate::cli::probe_help());
            return Ok(ProbeOutcome { exit_code: 0 });
        }
        ProbeTarget::MissingName => {
            println!(
                "The full destination name including application name aspects must be specified for the destination"
            );
            return Ok(ProbeOutcome { exit_code: 0 });
        }
        ProbeTarget::Destination {
            full_name,
            destination,
        } => (full_name.to_owned(), destination),
    };
    if args.size > MAX_SEND_SINGLE_PACKET_PLAINTEXT_LEN {
        return Err(RnprobeError::PacketTooLarge {
            size: args.size,
            maximum: MAX_SEND_SINGLE_PACKET_PLAINTEXT_LEN,
        });
    }
    let configuration =
        LoadedConfiguration::load(args.config.as_deref()).map_err(RnprobeError::Configuration)?;
    if args.verbose != 0 {
        for warning in &configuration.report.warnings {
            eprintln!("{warning}");
        }
    }
    let session = UtilityNodeSession::connect(
        &configuration,
        UtilityNodeIdentity::Anonymous,
        args.timeout
            .map_or(Duration::from_secs(15), |timeout| timeout.get()),
    )
    .await
    .map_err(RnprobeError::Session)?;
    session
        .run(|client| async move {
            run_probes(
                client,
                ProbeRequest {
                    full_name,
                    destination,
                    size: args.size,
                    probes: args.probes,
                    timeout: args.timeout.map(|timeout| timeout.get()),
                    wait: args.wait.get(),
                    verbose: args.verbose != 0,
                },
            )
            .await
        })
        .await
        .map_err(RnprobeError::NodeStopped)?
}

struct ProbeRequest {
    full_name: String,
    destination: DestinationHash,
    size: usize,
    probes: u32,
    timeout: Option<Duration>,
    wait: Duration,
    verbose: bool,
}

async fn run_probes(
    client: UtilityNodeClient,
    request: ProbeRequest,
) -> Result<ProbeOutcome, RnprobeError> {
    let effective_timeout = match request.timeout {
        Some(timeout) => timeout,
        None => DEFAULT_TIMEOUT
            .checked_add(
                client
                    .rpc()
                    .first_hop_timeout(request.destination)
                    .await
                    .map_err(RnprobeError::Rpc)?,
            )
            .unwrap_or(Duration::MAX)
            .max(
                client
                    .adaptive_path_timeout()
                    .await
                    .map_err(RnprobeError::Path)?,
            ),
    };
    client
        .ensure_path(request.destination, effective_timeout)
        .await
        .map_err(RnprobeError::Path)?;
    let identity = client
        .handle()
        .destination_identity_hash(request.destination)
        .await
        .ok_or(RnprobeError::IdentityUnavailable(request.destination))?;
    match destination_matches_name(identity, &request.full_name, request.destination) {
        Ok(true) => {}
        Ok(false) => return Err(RnprobeError::DestinationNameMismatch),
        Err(source) => return Err(RnprobeError::InvalidDestinationName(source)),
    }

    let mut replies = 0u32;
    for sent in 1..=request.probes {
        if sent > 1 {
            tokio::time::sleep(request.wait).await;
        }
        let payload = vec![(sent & 0xff) as u8; request.size];
        let more = if request.verbose {
            route_description(&client, request.destination).await?
        } else {
            String::new()
        };
        println!(
            "Sent probe {sent} ({} bytes) to {}{more}",
            request.size,
            pretty_hex(request.destination.as_bytes())
        );
        let delivery = tokio::time::timeout(
            effective_timeout,
            client
                .handle()
                .send_single_packet(request.destination, &payload),
        )
        .await;
        let delivered = match delivery {
            Err(_) => {
                println!("Probe timed out");
                continue;
            }
            Ok(Err(SendError::Failed(
                SendSinglePacketFailure::Timeout | SendSinglePacketFailure::Culled,
            ))) => {
                println!("Probe timed out");
                continue;
            }
            Ok(Err(source)) => return Err(RnprobeError::Send(source)),
            Ok(Ok(delivered)) => delivered,
        };
        let proof = match delivered.evidence {
            DeliveryEvidence::Proof(proof) => proof,
            DeliveryEvidence::Response => return Err(RnprobeError::UnexpectedResponseEvidence),
        };
        let phy = client
            .rpc()
            .packet_phy(proof.packet_hash())
            .await
            .map_err(RnprobeError::Rpc)?;
        let hops = client
            .handle()
            .route(request.destination)
            .await
            .map_or(0, |route| route.hops);
        replies += 1;
        println!(
            "Valid reply from {}",
            pretty_hex(request.destination.as_bytes())
        );
        println!(
            "Round-trip time is {} over {hops} hop{}{}\n",
            format_rtt(delivered.rtt.millis()),
            if hops == 1 { "" } else { "s" },
            format_phy(phy)
        );
    }
    let loss = (1.0 - f64::from(replies) / f64::from(request.probes)) * 100.0;
    println!(
        "Sent {}, received {replies}, packet loss {}%",
        request.probes,
        format_number((loss * 100.0).round() / 100.0)
    );
    Ok(ProbeOutcome {
        exit_code: if replies == request.probes { 0 } else { 2 },
    })
}

async fn route_description(
    client: &UtilityNodeClient,
    destination: DestinationHash,
) -> Result<String, RnprobeError> {
    let via = client
        .rpc()
        .next_hop(destination)
        .await
        .map_err(RnprobeError::Rpc)?
        .map(|transport| format!(" via {}", pretty_hex(transport.as_bytes())))
        .unwrap_or_default();
    let interface = client
        .rpc()
        .next_hop_interface_name(destination)
        .await
        .map_err(RnprobeError::Rpc)?
        .map(|name| format!(" on {name}"))
        .unwrap_or_default();
    Ok(format!("{via}{interface}"))
}

fn format_rtt(millis: u64) -> String {
    if millis >= 1_000 {
        format!("{} seconds", format_number(millis as f64 / 1_000.0))
    } else {
        format!("{} milliseconds", format_number(millis as f64))
    }
}

fn format_phy(stats: SharedInstancePacketPhyStats) -> String {
    let mut output = String::new();
    if let Some(rssi) = stats.rssi_dbm {
        output.push_str(&format!(" [RSSI {} dBm]", format_number(rssi)));
    }
    if let Some(snr) = stats.snr_db {
        output.push_str(&format!(" [SNR {} dB]", format_number(snr)));
    }
    if let Some(quality) = stats.quality_percent {
        output.push_str(&format!(" [Link Quality {}%]", format_number(quality)));
    }
    output
}

fn format_number(value: f64) -> String {
    let mut rendered = format!("{value:.3}");
    while rendered.contains('.') && rendered.ends_with('0') {
        rendered.pop();
    }
    if rendered.ends_with('.') {
        rendered.pop();
    }
    rendered
}

fn pretty_hex(bytes: &[u8]) -> String {
    format!(
        "<{}>",
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

#[derive(Debug)]
pub enum RnprobeError {
    Configuration(UtilityConfigurationError),
    Session(UtilityNodeSessionError),
    NodeStopped(UtilityNodeStopped),
    Rpc(SharedInstanceRpcClientError),
    Path(UtilityPathError),
    PacketTooLarge { size: usize, maximum: usize },
    IdentityUnavailable(DestinationHash),
    InvalidDestinationName(personal_rns::routing::announce::ExpandNameError),
    DestinationNameMismatch,
    Send(SendError<SendSinglePacketFailure>),
    UnexpectedResponseEvidence,
}

impl RnprobeError {
    pub const fn exit_code(&self) -> u8 {
        match self {
            Self::PacketTooLarge { .. } => 3,
            _ => 1,
        }
    }
}

impl fmt::Display for RnprobeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration(source) => source.fmt(formatter),
            Self::Session(source) => source.fmt(formatter),
            Self::NodeStopped(source) => source.fmt(formatter),
            Self::Rpc(source) => source.fmt(formatter),
            Self::Path(source) => source.fmt(formatter),
            Self::PacketTooLarge { size, maximum } => write!(
                formatter,
                "probe payload size of {size} bytes exceeds the maximum of {maximum} bytes"
            ),
            Self::IdentityUnavailable(destination) => write!(
                formatter,
                "no recalled identity is available for {}",
                pretty_hex(destination.as_bytes())
            ),
            Self::InvalidDestinationName(source) => {
                write!(formatter, "invalid destination name: {source:?}")
            }
            Self::DestinationNameMismatch => formatter.write_str(
                "the full destination name and recalled identity do not match the destination hash",
            ),
            Self::Send(source) => write!(formatter, "could not send probe: {source:?}"),
            Self::UnexpectedResponseEvidence => formatter
                .write_str("probe delivery settled with response evidence instead of a proof"),
        }
    }
}

impl std::error::Error for RnprobeError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stock_style_numbers_omit_redundant_fractional_zeroes() {
        assert_eq!(format_number(0.0), "0");
        assert_eq!(format_number(12.5), "12.5");
        assert_eq!(format_number(-71.25), "-71.25");
    }

    #[test]
    fn packet_loss_exit_codes_match_stock_probe() {
        assert_eq!(ProbeOutcome { exit_code: 0 }.exit_code(), 0);
        assert_eq!(ProbeOutcome { exit_code: 2 }.exit_code(), 2);
    }
}
