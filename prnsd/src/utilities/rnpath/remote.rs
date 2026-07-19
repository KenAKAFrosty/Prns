use std::fmt;
use std::path::Path;
use std::time::Duration;

use personal_rns::engine::{EstablishLinkFailure, SendRequestFailure};
use personal_rns::identity::IdentityHash;
use personal_rns::interfaces::rns_management::{
    RnsAnnounceRateTable, RnsAnnounceRateTableDecodeError, RnsBlackholeDecodeError,
    RnsBlackholeTable, RnsManagementEncodeError, RnsPathTable, RnsPathTableDecodeError,
    RnsRemotePathRequest, RnsRemotePathTableRequest, RnsRemoteRateTableRequest,
};
use personal_rns::routing::announce::{derive_single_destination_hash, ExpandNameError};
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::routing::BlackholedIdentity;
use personal_rns::runtime::SendError;
use personal_rns::units::InstantMillis;
use personal_rns::wire::DestinationHash;

use super::super::configuration::LoadedConfiguration;
use super::super::remote_management::{RemoteManagementError, RemoteManagementSession};
use super::super::session::{
    UtilityNodeIdentity, UtilityNodeSession, UtilityNodeSessionError, UtilityNodeStopped,
    UtilityPathError,
};

const PATH_PATH: &str = "/path";
const LIST_PATH: &str = "/list";
const BLACKHOLE_APP_NAME: &str = "rnstransport";
const BLACKHOLE_ASPECTS: &[&str] = &["info", "blackhole"];

pub async fn path_table(
    configuration: &LoadedConfiguration,
    transport_identity: IdentityHash,
    management_identity: &Path,
    destination: Option<DestinationHash>,
    maximum_hops: Option<i64>,
    timeout: Duration,
) -> Result<RnsPathTable, RemotePathQueryError> {
    let request =
        RnsRemotePathRequest::Table(RnsRemotePathTableRequest::new(destination, maximum_hops))
            .encode_message_pack()
            .map_err(RemotePathQueryError::Encode)?;
    let response = RemoteManagementSession::run(
        configuration,
        transport_identity,
        management_identity,
        timeout,
        move |session| async move { session.request(PATH_PATH, &request).await },
    )
    .await
    .map_err(RemotePathQueryError::Remote)?;
    let table =
        RnsPathTable::decode_message_pack(&response).map_err(RemotePathQueryError::PathDecode)?;
    if table.entries().is_empty() {
        return Err(RemotePathQueryError::EmptyReply);
    }
    Ok(table)
}

pub async fn rate_table(
    configuration: &LoadedConfiguration,
    transport_identity: IdentityHash,
    management_identity: &Path,
    destination: Option<DestinationHash>,
    timeout: Duration,
) -> Result<RnsAnnounceRateTable, RemotePathQueryError> {
    let request = RnsRemotePathRequest::Rates(RnsRemoteRateTableRequest::new(destination))
        .encode_message_pack()
        .map_err(RemotePathQueryError::Encode)?;
    let response = RemoteManagementSession::run(
        configuration,
        transport_identity,
        management_identity,
        timeout,
        move |session| async move { session.request(PATH_PATH, &request).await },
    )
    .await
    .map_err(RemotePathQueryError::Remote)?;
    let table = RnsAnnounceRateTable::decode_message_pack(&response)
        .map_err(RemotePathQueryError::RateDecode)?;
    if table.entries().is_empty() {
        return Err(RemotePathQueryError::EmptyReply);
    }
    Ok(table)
}

pub async fn published_blackholes(
    configuration: &LoadedConfiguration,
    source: IdentityHash,
    timeout: Duration,
    now: InstantMillis,
) -> Result<Vec<BlackholedIdentity<String>>, PublishedBlackholeError> {
    let destination =
        derive_single_destination_hash(&source, BLACKHOLE_APP_NAME, BLACKHOLE_ASPECTS)
            .map_err(PublishedBlackholeError::Destination)?;
    let utility =
        UtilityNodeSession::connect(configuration, UtilityNodeIdentity::Anonymous, timeout)
            .await
            .map_err(PublishedBlackholeError::Session)?;
    let response = tokio::time::timeout(
        timeout,
        utility.run(move |utility| async move {
            utility
                .ensure_path(destination, timeout)
                .await
                .map_err(PublishedBlackholeError::Path)?;
            let link = utility
                .handle()
                .establish_link(destination)
                .await
                .map_err(PublishedBlackholeError::Link)?;
            let response = utility
                .handle()
                .request(link, RequestPathHash::of(LIST_PATH), &[])
                .await;
            utility.handle().close_link(link);
            response
                .map(|(response, _)| response)
                .map_err(PublishedBlackholeError::Request)
        }),
    )
    .await
    .map_err(|_| PublishedBlackholeError::Timeout(timeout))?
    .map_err(PublishedBlackholeError::NodeStopped)??;
    RnsBlackholeTable::decode_published_table(&response, now)
        .map(RnsBlackholeTable::into_entries)
        .map_err(PublishedBlackholeError::Decode)
}

#[derive(Debug)]
pub enum RemotePathQueryError {
    Encode(RnsManagementEncodeError),
    Remote(RemoteManagementError),
    PathDecode(RnsPathTableDecodeError),
    RateDecode(RnsAnnounceRateTableDecodeError),
    EmptyReply,
}

impl fmt::Display for RemotePathQueryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Encode(source) => {
                write!(formatter, "could not encode remote path request: {source}")
            }
            Self::Remote(source) => source.fmt(formatter),
            Self::PathDecode(source) => {
                write!(
                    formatter,
                    "remote /path returned an invalid path table: {source}"
                )
            }
            Self::RateDecode(source) => {
                write!(
                    formatter,
                    "remote /path returned an invalid rate table: {source}"
                )
            }
            Self::EmptyReply => formatter.write_str(
                "the remote request failed; the result was empty or authentication was rejected",
            ),
        }
    }
}

impl std::error::Error for RemotePathQueryError {}

#[derive(Debug)]
pub enum PublishedBlackholeError {
    Destination(ExpandNameError),
    Session(UtilityNodeSessionError),
    Path(UtilityPathError),
    Link(SendError<EstablishLinkFailure>),
    Request(SendError<SendRequestFailure>),
    Timeout(Duration),
    NodeStopped(UtilityNodeStopped),
    Decode(RnsBlackholeDecodeError),
}

impl fmt::Display for PublishedBlackholeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Destination(source) => {
                write!(
                    formatter,
                    "could not derive blackhole destination: {source:?}"
                )
            }
            Self::Session(source) => source.fmt(formatter),
            Self::Path(source) => source.fmt(formatter),
            Self::Link(source) => write!(formatter, "could not establish list link: {source:?}"),
            Self::Request(source) => write!(formatter, "remote /list request failed: {source:?}"),
            Self::Timeout(timeout) => {
                write!(
                    formatter,
                    "remote /list request timed out after {timeout:?}"
                )
            }
            Self::NodeStopped(source) => source.fmt(formatter),
            Self::Decode(source) => {
                write!(formatter, "remote /list returned invalid data: {source}")
            }
        }
    }
}

impl std::error::Error for PublishedBlackholeError {}
