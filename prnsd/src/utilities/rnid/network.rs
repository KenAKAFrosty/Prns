use std::time::Duration;

use personal_rns::identity::{IdentityHash, PublicIdentityMaterial, Zeroizing};
use personal_rns::node_introspection::{DestinationIdentityQuery, DestinationIdentitySnapshot};
use personal_rns::routing::announce::{derive_single_destination_hash, ExpandNameError};
use personal_rns::runtime::AnnounceNowError;
use personal_rns::wire::DestinationHash;

use super::args::RnidArgs;
use super::identity::{pretty_hash, LocalIdentity, LocalIdentityError};
use crate::utilities::configuration::{LoadedConfiguration, UtilityConfigurationError};
use crate::utilities::session::{
    UtilityBusClient, UtilityBusSession, UtilityNodeIdentity, UtilityNodeSessionError,
    UtilityNodeStopped,
};

#[derive(Debug)]
pub enum IdentityNetworkError {
    Configuration(UtilityConfigurationError),
    Session(UtilityNodeSessionError),
    NodeStopped(UtilityNodeStopped),
    DestinationName(ExpandNameError),
    InvalidAspects,
    LookupTimedOut {
        identity: IdentityHash,
        timeout: Duration,
    },
    Announce(AnnounceNowError),
    Identity(LocalIdentityError),
}

pub async fn resolve(
    args: &RnidArgs,
    identity: Option<LocalIdentity>,
) -> Result<Option<LocalIdentity>, IdentityNetworkError> {
    let Some(LocalIdentity::Hash(requested)) = identity else {
        return Ok(identity);
    };
    if args.no_cache || !args.request {
        if requires_working_identity(args) {
            return Ok(None);
        }
        return Ok(Some(LocalIdentity::Hash(requested)));
    }
    let configuration = LoadedConfiguration::load(args.config.as_deref())
        .map_err(IdentityNetworkError::Configuration)?;
    let timeout = args.timeout.duration();
    let session = UtilityBusSession::connect(&configuration, UtilityNodeIdentity::Anonymous)
        .await
        .map_err(IdentityNetworkError::Session)?;
    let public = session
        .run(|client| lookup(client, requested, timeout))
        .await
        .map_err(IdentityNetworkError::NodeStopped)??;
    println!(
        "Received Identity {} for destination {} from the network",
        pretty_hash(public.identity_hash()),
        pretty_hash(requested)
    );
    Ok(Some(LocalIdentity::Public(public)))
}

fn requires_working_identity(args: &RnidArgs) -> bool {
    args.print_identity
        || args.export_public
        || args.export_private
        || args.encrypt.is_some()
        || args.decrypt.is_some()
        || args.sign.is_some()
        || args.sign_message.is_some()
        || args.announce.is_some()
        || (args.write.is_some() && args.sign_message.is_none())
}

pub async fn announce(
    args: &RnidArgs,
    identity: &LocalIdentity,
    full_name: &str,
) -> Result<(), IdentityNetworkError> {
    let private = identity.private().map_err(IdentityNetworkError::Identity)?;
    let mut components = full_name.split('.');
    let app_name = components.next().unwrap_or_default();
    let aspects: Vec<_> = components.collect();
    if app_name.is_empty() || aspects.is_empty() {
        return Err(IdentityNetworkError::InvalidAspects);
    }
    let configuration = LoadedConfiguration::load(args.config.as_deref())
        .map_err(IdentityNetworkError::Configuration)?;
    let (session, destination) = UtilityBusSession::connect_announcing(
        &configuration,
        Zeroizing::new(*private.as_bytes()),
        app_name,
        &aspects,
    )
    .await
    .map_err(IdentityNetworkError::Session)?;
    println!(
        "Announcing {full_name} destination <{}> for identity {}",
        hex(destination.as_bytes()),
        pretty_hash(identity.identity_hash())
    );
    session
        .run(|client| async move {
            client
                .announce(destination)
                .await
                .map_err(IdentityNetworkError::Announce)?;
            tokio::time::sleep(Duration::from_millis(250)).await;
            Ok(())
        })
        .await
        .map_err(IdentityNetworkError::NodeStopped)?
}

async fn lookup(
    client: UtilityBusClient,
    requested: IdentityHash,
    timeout: Duration,
) -> Result<PublicIdentityMaterial, IdentityNetworkError> {
    let requested_destination = DestinationHash::new(*requested.as_bytes());
    let identity_destination = derive_single_destination_hash(&requested, "rns", &["id"])
        .map_err(IdentityNetworkError::DestinationName)?;
    if let Some(public) = query(
        &client,
        requested,
        requested_destination,
        identity_destination,
    )
    .await
    {
        return Ok(public);
    }
    let destination_request = client.handle().request_path(requested_destination);
    let identity_request = client.handle().request_path(identity_destination);
    tokio::pin!(destination_request);
    tokio::pin!(identity_request);
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    let mut destination_finished = false;
    let mut identity_finished = false;
    let mut polling = tokio::time::interval(Duration::from_millis(50));
    loop {
        tokio::select! {
            _ = &mut destination_request, if !destination_finished => {
                destination_finished = true;
            }
            _ = &mut identity_request, if !identity_finished => {
                identity_finished = true;
            }
            _ = polling.tick() => {
                if let Some(public) = query(
                    &client,
                    requested,
                    requested_destination,
                    identity_destination,
                ).await {
                    return Ok(public);
                }
            }
            () = &mut deadline => {
                return Err(IdentityNetworkError::LookupTimedOut { identity: requested, timeout });
            }
        }
    }
}

async fn query(
    client: &UtilityBusClient,
    requested: IdentityHash,
    requested_destination: DestinationHash,
    identity_destination: DestinationHash,
) -> Option<PublicIdentityMaterial> {
    for query in [
        DestinationIdentityQuery::Destination(requested_destination),
        DestinationIdentityQuery::Identity(requested),
        DestinationIdentityQuery::Destination(identity_destination),
    ] {
        if let Some(snapshot) = client.destination_identity(query).await {
            if matches_request(snapshot, requested, requested_destination) {
                return Some(snapshot.public);
            }
        }
    }
    None
}

fn matches_request(
    snapshot: DestinationIdentitySnapshot,
    requested: IdentityHash,
    requested_destination: DestinationHash,
) -> bool {
    snapshot.identity == requested || snapshot.destination == requested_destination
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

impl std::fmt::Display for IdentityNetworkError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Configuration(source) => source.fmt(formatter),
            Self::Session(source) => source.fmt(formatter),
            Self::NodeStopped(source) => source.fmt(formatter),
            Self::DestinationName(source) => {
                write!(formatter, "invalid destination aspects: {source:?}")
            }
            Self::InvalidAspects => formatter.write_str("invalid destination aspects specified"),
            Self::LookupTimedOut { identity, timeout } => write!(
                formatter,
                "identity request for {} timed out after {:.3} seconds",
                pretty_hash(*identity),
                timeout.as_secs_f64()
            ),
            Self::Announce(source) => {
                write!(formatter, "could not send identity announce: {source:?}")
            }
            Self::Identity(source) => source.fmt(formatter),
        }
    }
}

impl std::error::Error for IdentityNetworkError {}
