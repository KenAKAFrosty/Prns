mod args;
mod remote;
mod render;

use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub use args::RnpathArgs;
use args::{RnpathOperation, RnpathTarget};
use personal_rns::interfaces::shared_instance::rns_rpc::RnsNumber;
use personal_rns::shared_instance::{
    SharedInstanceBlackholeOutcome, SharedInstanceRpcClientError, SharedInstanceUnblackholeOutcome,
};
use personal_rns::units::InstantMillis;

use super::configuration::{LoadedConfiguration, UtilityConfigurationError};
use super::session::{
    UtilityNodeIdentity, UtilityNodeSession, UtilityNodeSessionError, UtilityNodeStopped,
};

pub async fn run(args: RnpathArgs) -> Result<(), RnpathError> {
    if args.version {
        println!(
            "prnsd path {} (RNS 1.4.2 compatibility)",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }
    let operation = args.operation().map_err(RnpathError::Arguments)?;
    if operation == RnpathOperation::Help {
        print!("{}", crate::cli::path_help());
        return Ok(());
    }
    let target = args.target().map_err(RnpathError::Arguments)?;
    if matches!(target, RnpathTarget::Remote { .. })
        && !matches!(
            operation,
            RnpathOperation::Table { .. } | RnpathOperation::Rates { .. }
        )
    {
        return Err(RnpathError::UnsupportedRemote);
    }
    let configuration =
        LoadedConfiguration::load(args.config.as_deref()).map_err(RnpathError::Configuration)?;
    let rpc_timeout = args
        .path_timeout
        .map_or(Duration::from_secs(15), |timeout| timeout.get());
    if args.verbose != 0 {
        for warning in &configuration.report.warnings {
            eprintln!("{warning}");
        }
    }
    match (target, operation) {
        (
            RnpathTarget::Local,
            RnpathOperation::Table {
                destination,
                maximum_hops,
            },
        ) => {
            let client = configuration
                .local_rpc_client(rpc_timeout)
                .map_err(RnpathError::Configuration)?;
            let mut entries = client
                .path_table(maximum_hops)
                .await
                .map_err(RnpathError::Rpc)?
                .into_entries();
            entries.retain(|entry| destination.is_none_or(|hash| entry.destination() == hash));
            entries.sort_by(|left, right| {
                left.interface()
                    .cmp(right.interface())
                    .then_with(|| left.hops().cmp(&right.hops()))
            });
            if destination.is_some() && entries.is_empty() {
                return Err(RnpathError::NoPathKnown);
            }
            print!(
                "{}",
                render::path_table(&entries, args.json).map_err(RnpathError::Json)?
            );
            Ok(())
        }
        (
            RnpathTarget::Remote {
                transport_identity,
                management_identity,
            },
            RnpathOperation::Table {
                destination,
                maximum_hops,
            },
        ) => {
            let table = remote::path_table(
                &configuration,
                transport_identity,
                management_identity,
                destination,
                maximum_hops,
                args.remote_timeout.get(),
            )
            .await
            .map_err(RnpathError::Remote)?;
            print!(
                "{}",
                render::path_table(table.entries(), args.json).map_err(RnpathError::Json)?
            );
            Ok(())
        }
        (RnpathTarget::Local, RnpathOperation::Rates { destination }) => {
            let client = configuration
                .local_rpc_client(rpc_timeout)
                .map_err(RnpathError::Configuration)?;
            let mut entries = client
                .announce_rate_table()
                .await
                .map_err(RnpathError::Rpc)?
                .into_entries();
            entries.retain(|entry| destination.is_none_or(|hash| entry.destination() == hash));
            entries.sort_by(|left, right| {
                left.last_allowed_announce_at_seconds()
                    .total_cmp(&right.last_allowed_announce_at_seconds())
            });
            if destination.is_some() && entries.is_empty() {
                return Err(RnpathError::NoRateInformation);
            }
            print!(
                "{}",
                render::rates(&entries, args.json, unix_time_seconds())
                    .map_err(RnpathError::Json)?
            );
            Ok(())
        }
        (
            RnpathTarget::Remote {
                transport_identity,
                management_identity,
            },
            RnpathOperation::Rates { destination },
        ) => {
            let table = remote::rate_table(
                &configuration,
                transport_identity,
                management_identity,
                destination,
                args.remote_timeout.get(),
            )
            .await
            .map_err(RnpathError::Remote)?;
            let mut entries = table.into_entries();
            entries.sort_by(|left, right| {
                left.last_allowed_announce_at_seconds()
                    .total_cmp(&right.last_allowed_announce_at_seconds())
            });
            print!(
                "{}",
                render::rates(&entries, args.json, unix_time_seconds())
                    .map_err(RnpathError::Json)?
            );
            Ok(())
        }
        (RnpathTarget::Local, RnpathOperation::DropPath(destination)) => {
            let client = configuration
                .local_rpc_client(rpc_timeout)
                .map_err(RnpathError::Configuration)?;
            if client
                .drop_path(destination)
                .await
                .map_err(RnpathError::Rpc)?
            {
                println!(
                    "Dropped path to {}",
                    render::pretty_hex(destination.as_bytes())
                );
                Ok(())
            } else {
                Err(RnpathError::PathDropFailed(destination))
            }
        }
        (RnpathTarget::Local, RnpathOperation::DropAnnounces) => {
            let client = configuration
                .local_rpc_client(rpc_timeout)
                .map_err(RnpathError::Configuration)?;
            println!("Dropping announce queues on all interfaces...");
            client
                .drop_announce_queues()
                .await
                .map_err(RnpathError::Rpc)
        }
        (RnpathTarget::Local, RnpathOperation::DropVia(transport)) => {
            let client = configuration
                .local_rpc_client(rpc_timeout)
                .map_err(RnpathError::Configuration)?;
            let dropped = client
                .drop_all_via(transport)
                .await
                .map_err(RnpathError::Rpc)?;
            if dropped == 0 {
                Err(RnpathError::TransportDropFailed(transport))
            } else {
                println!(
                    "Dropped all paths via {}",
                    render::pretty_hex(transport.as_bytes())
                );
                Ok(())
            }
        }
        (RnpathTarget::Local, RnpathOperation::ListBlackholes { filter }) => {
            let client = configuration
                .local_rpc_client(rpc_timeout)
                .map_err(RnpathError::Configuration)?;
            let now = unix_time_millis();
            let entries = client
                .blackholed_identities(now)
                .await
                .map_err(RnpathError::Rpc)?;
            display_blackholes(&configuration, &entries, filter.as_deref(), now)
        }
        (
            RnpathTarget::Local,
            RnpathOperation::Blackhole {
                identity,
                duration,
                reason,
            },
        ) => {
            let client = configuration
                .local_rpc_client(rpc_timeout)
                .map_err(RnpathError::Configuration)?;
            let until = duration.and_then(|duration| {
                (duration.get() != 0.0)
                    .then(|| RnsNumber::Float(unix_time_seconds() + duration.get() * 3_600.0))
            });
            match client
                .blackhole_identity(identity, until, reason)
                .await
                .map_err(RnpathError::Rpc)?
            {
                SharedInstanceBlackholeOutcome::Added => {
                    println!("Blackholed identity {}", render::hex(identity.as_bytes()))
                }
                SharedInstanceBlackholeOutcome::AlreadyPresent => println!(
                    "Identity {} already blackholed",
                    render::hex(identity.as_bytes())
                ),
                SharedInstanceBlackholeOutcome::Rejected => {
                    return Err(RnpathError::BlackholeRejected(identity));
                }
            }
            Ok(())
        }
        (RnpathTarget::Local, RnpathOperation::Unblackhole(identity)) => {
            let client = configuration
                .local_rpc_client(rpc_timeout)
                .map_err(RnpathError::Configuration)?;
            match client
                .unblackhole_identity(identity)
                .await
                .map_err(RnpathError::Rpc)?
            {
                SharedInstanceUnblackholeOutcome::Removed => println!(
                    "Lifted blackhole for identity {}",
                    render::hex(identity.as_bytes())
                ),
                SharedInstanceUnblackholeOutcome::NotFound => println!(
                    "Identity {} not blackholed",
                    render::hex(identity.as_bytes())
                ),
                SharedInstanceUnblackholeOutcome::Rejected => {
                    return Err(RnpathError::UnblackholeRejected(identity));
                }
            }
            Ok(())
        }
        (RnpathTarget::Local, RnpathOperation::PublishedBlackholes { source, filter }) => {
            let now = unix_time_millis();
            let entries = remote::published_blackholes(
                &configuration,
                source,
                args.remote_timeout.get(),
                now,
            )
            .await
            .map_err(RnpathError::Published)?;
            display_blackholes(&configuration, &entries, filter.as_deref(), now)
        }
        (RnpathTarget::Local, RnpathOperation::RequestPath(destination)) => {
            request_path(
                &configuration,
                destination,
                args.path_timeout.map(|timeout| timeout.get()),
            )
            .await
        }
        (_, RnpathOperation::Help) => Ok(()),
        (RnpathTarget::Remote { .. }, _) => Err(RnpathError::UnsupportedRemote),
    }
}

async fn request_path(
    configuration: &LoadedConfiguration,
    destination: personal_rns::wire::DestinationHash,
    explicit_timeout: Option<Duration>,
) -> Result<(), RnpathError> {
    let rpc_timeout = explicit_timeout.unwrap_or(Duration::from_secs(15));
    let utility =
        UtilityNodeSession::connect(configuration, UtilityNodeIdentity::Anonymous, rpc_timeout)
            .await
            .map_err(RnpathError::Session)?;
    let (found, next_hop, interface) = utility
        .run(move |utility| async move {
            let timeout = match explicit_timeout {
                Some(timeout) => timeout,
                None => utility
                    .adaptive_path_timeout()
                    .await
                    .map_err(|_| RnpathError::PathNotFound)?,
            };
            let found = utility
                .ensure_path(destination, timeout)
                .await
                .map_err(|_| RnpathError::PathNotFound)?;
            let next_hop = utility
                .rpc()
                .next_hop(destination)
                .await
                .map_err(RnpathError::Rpc)?
                .ok_or(RnpathError::InvalidPathData)?;
            let interface = utility
                .rpc()
                .next_hop_interface_name(destination)
                .await
                .map_err(RnpathError::Rpc)?
                .ok_or(RnpathError::InvalidPathData)?;
            Ok::<_, RnpathError>((found, next_hop, interface))
        })
        .await
        .map_err(RnpathError::NodeStopped)??;
    print!(
        "{}",
        render::found_path(
            destination.as_bytes(),
            found.hops.0,
            next_hop.as_bytes(),
            &interface,
        )
    );
    Ok(())
}

fn display_blackholes(
    configuration: &LoadedConfiguration,
    entries: &[personal_rns::routing::BlackholedIdentity<String>],
    filter: Option<&str>,
    now: InstantMillis,
) -> Result<(), RnpathError> {
    if entries.is_empty() {
        return Err(RnpathError::NoBlackholeData);
    }
    let local_transport = configuration
        .local_transport_identity_hash()
        .map_err(RnpathError::Configuration)?;
    let (output, _) = render::blackholes(entries, filter, local_transport, now.0 as f64 / 1_000.0);
    print!("{output}");
    Ok(())
}

fn unix_time_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |duration| duration.as_secs_f64())
}

fn unix_time_millis() -> InstantMillis {
    InstantMillis(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| {
                duration.as_millis().min(u128::from(u64::MAX)) as u64
            }),
    )
}

#[derive(Debug)]
pub enum RnpathError {
    Arguments(String),
    Configuration(UtilityConfigurationError),
    Rpc(SharedInstanceRpcClientError),
    Remote(remote::RemotePathQueryError),
    Published(remote::PublishedBlackholeError),
    Session(UtilityNodeSessionError),
    NodeStopped(UtilityNodeStopped),
    Json(serde_json::Error),
    UnsupportedRemote,
    NoPathKnown,
    NoRateInformation,
    PathDropFailed(personal_rns::wire::DestinationHash),
    TransportDropFailed(personal_rns::wire::TransportId),
    BlackholeRejected(personal_rns::identity::IdentityHash),
    UnblackholeRejected(personal_rns::identity::IdentityHash),
    NoBlackholeData,
    PathNotFound,
    InvalidPathData,
}

impl RnpathError {
    pub const fn exit_code(&self) -> u8 {
        match self {
            Self::Arguments(_) => 2,
            Self::UnsupportedRemote => 255,
            Self::NoBlackholeData => 20,
            Self::Remote(_) => 10,
            Self::Published(remote::PublishedBlackholeError::Path(_)) => 12,
            Self::Published(_) => 10,
            Self::Configuration(_)
            | Self::Rpc(_)
            | Self::Session(_)
            | Self::NodeStopped(_)
            | Self::Json(_)
            | Self::NoPathKnown
            | Self::NoRateInformation
            | Self::PathDropFailed(_)
            | Self::TransportDropFailed(_)
            | Self::BlackholeRejected(_)
            | Self::UnblackholeRejected(_)
            | Self::PathNotFound
            | Self::InvalidPathData => 1,
        }
    }
}

impl fmt::Display for RnpathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Arguments(source) => formatter.write_str(source),
            Self::Configuration(source) => source.fmt(formatter),
            Self::Rpc(source) => source.fmt(formatter),
            Self::Remote(source) => source.fmt(formatter),
            Self::Published(source) => source.fmt(formatter),
            Self::Session(source) => source.fmt(formatter),
            Self::NodeStopped(source) => source.fmt(formatter),
            Self::Json(source) => write!(formatter, "could not encode JSON output: {source}"),
            Self::UnsupportedRemote => formatter.write_str(
                "remote mutation and path requests are not implemented by RNS 1.4.2; only --table and --rates support -R",
            ),
            Self::NoPathKnown => formatter.write_str("No path known"),
            Self::NoRateInformation => formatter.write_str("No information available"),
            Self::PathDropFailed(destination) => write!(
                formatter,
                "Unable to drop path to {}. Does it exist?",
                render::pretty_hex(destination.as_bytes())
            ),
            Self::TransportDropFailed(transport) => write!(
                formatter,
                "Unable to drop paths via {}. Does the transport instance exist?",
                render::pretty_hex(transport.as_bytes())
            ),
            Self::BlackholeRejected(identity) => write!(
                formatter,
                "Could not blackhole identity {}",
                render::hex(identity.as_bytes())
            ),
            Self::UnblackholeRejected(identity) => write!(
                formatter,
                "Could not unblackhole identity {}",
                render::hex(identity.as_bytes())
            ),
            Self::NoBlackholeData => {
                formatter.write_str("No blackholed identity data available")
            }
            Self::PathNotFound => formatter.write_str("Path not found"),
            Self::InvalidPathData => formatter.write_str("Error: Invalid path data returned"),
        }
    }
}

impl std::error::Error for RnpathError {}
