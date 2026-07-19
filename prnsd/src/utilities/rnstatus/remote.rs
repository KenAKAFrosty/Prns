use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use personal_rns::engine::{EstablishLinkFailure, IdentifyFailure, SendRequestFailure};
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::vault::{read_identity_file, FileVaultError};
use personal_rns::identity::{IdentityHash, IdentitySigner};
use personal_rns::interfaces::rns_management::{
    RnsInterfaceStatsDecodeError, RnsManagementEncodeError, RnsRemoteInterfaceStatsReport,
    RnsRemoteStatusRequest,
};
use personal_rns::node_introspection::NodeIntrospection;
use personal_rns::routes;
use personal_rns::routing::announce::{derive_single_destination_hash, ExpandNameError};
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::runtime::{
    Manual, NonRoutingIdentityError, PrnsNode, PrnsNodeHandle, PrnsNodeRecipe, RequestPathError,
    SendError,
};
use personal_rns::shared_instance::{
    connect_existing_shared_instance, ExistingSharedInstanceUnavailable,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::wire::DestinationHash;

use super::super::configuration::{LoadedConfiguration, UtilityConfigurationError};

const REMOTE_MANAGEMENT_APP_NAME: &str = "rnstransport";
const REMOTE_MANAGEMENT_ASPECTS: &[&str] = &["remote", "management"];
const STATUS_PATH: &str = "/status";

pub async fn query(
    configuration: &LoadedConfiguration,
    transport_identity: IdentityHash,
    management_identity_path: &Path,
    include_link_count: bool,
    timeout: Duration,
) -> Result<RnsRemoteInterfaceStatsReport, RemoteStatusError> {
    let management_identity_path = expand_user_path(management_identity_path)
        .map_err(RemoteStatusError::ManagementIdentityPath)?;
    let management_identity = read_identity_file(&management_identity_path)
        .map_err(|source| RemoteStatusError::ReadManagementIdentity {
            path: management_identity_path.clone(),
            source,
        })?
        .ok_or_else(|| RemoteStatusError::ManagementIdentityMissing {
            path: management_identity_path.clone(),
        })?;
    let management_identity_hash =
        InMemoryNodeIdentity::from_secret_key_bytes(&management_identity).identity_hash();
    let destination = derive_single_destination_hash(
        &transport_identity,
        REMOTE_MANAGEMENT_APP_NAME,
        REMOTE_MANAGEMENT_ASPECTS,
    )
    .map_err(RemoteStatusError::Destination)?;
    let bus = configuration
        .local_bus_client_intent()
        .map_err(RemoteStatusError::Configuration)?;
    let operation = async move {
        let node = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: None,
            pre_configured_destinations: std::iter::empty(),
            app_state: (),
            storage: GrowableHeap,
            routes: routes![],
            interfaces: Manual,
            on_event: |_event, _state| {},
        })
        .with_non_routing_identity(management_identity)
        .map_err(RemoteStatusError::IdentityConfiguration)?;
        let handle = node.handle();
        connect_existing_shared_instance(&handle, bus)
            .await
            .map_err(RemoteStatusError::SharedInstanceUnavailable)?;
        tokio::select! {
            result = query_through_node(
                &handle,
                destination,
                management_identity_hash,
                include_link_count,
            ) => result,
            () = node.run() => Err(RemoteStatusError::NodeStopped),
        }
    };
    tokio::time::timeout(timeout, operation)
        .await
        .map_err(|_| RemoteStatusError::Timeout { timeout })?
}

async fn query_through_node(
    handle: &PrnsNodeHandle,
    destination: DestinationHash,
    management_identity: IdentityHash,
    include_link_count: bool,
) -> Result<RnsRemoteInterfaceStatsReport, RemoteStatusError> {
    if handle.route(destination).await.is_none() {
        handle
            .request_path(destination)
            .await
            .map_err(RemoteStatusError::Path)?;
    }
    let link = handle
        .establish_link(destination)
        .await
        .map_err(RemoteStatusError::Link)?;
    let result = query_link(handle, link, management_identity, include_link_count).await;
    handle.close_link(link);
    result
}

async fn query_link(
    handle: &PrnsNodeHandle,
    link: personal_rns::routing::links::LinkId,
    management_identity: IdentityHash,
    include_link_count: bool,
) -> Result<RnsRemoteInterfaceStatsReport, RemoteStatusError> {
    handle
        .identify(link, management_identity)
        .await
        .map_err(RemoteStatusError::Identify)?;
    let request = if include_link_count {
        RnsRemoteStatusRequest::InterfaceStatsAndLinkCount
    } else {
        RnsRemoteStatusRequest::InterfaceStats
    };
    let request = request
        .encode_message_pack()
        .map_err(RemoteStatusError::Encode)?;
    let (response, _) = handle
        .request(link, RequestPathHash::of(STATUS_PATH), &request)
        .await
        .map_err(RemoteStatusError::Request)?;
    RnsRemoteInterfaceStatsReport::decode_message_pack(&response).map_err(RemoteStatusError::Decode)
}

fn expand_user_path(path: &Path) -> Result<PathBuf, UserRelativePathError> {
    expand_user_path_with_home(
        path,
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .as_deref(),
    )
}

fn expand_user_path_with_home(
    path: &Path,
    home: Option<&std::ffi::OsStr>,
) -> Result<PathBuf, UserRelativePathError> {
    let Some(path_text) = path.to_str() else {
        return Ok(path.to_owned());
    };
    let remainder = path_text
        .strip_prefix("~/")
        .or_else(|| path_text.strip_prefix("~\\"));
    if path_text != "~" && remainder.is_none() {
        return Ok(path.to_owned());
    }
    let Some(home) = home.filter(|home| !home.is_empty()) else {
        return Err(UserRelativePathError {
            path: path.to_owned(),
        });
    };
    Ok(remainder.map_or_else(
        || PathBuf::from(home),
        |suffix| PathBuf::from(home).join(suffix),
    ))
}

#[derive(Debug)]
pub struct UserRelativePathError {
    path: PathBuf,
}

impl fmt::Display for UserRelativePathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "management identity path {} requires a home directory, but neither HOME nor USERPROFILE is available",
            self.path.display()
        )
    }
}

#[derive(Debug)]
pub enum RemoteStatusError {
    Configuration(UtilityConfigurationError),
    ReadManagementIdentity {
        path: PathBuf,
        source: FileVaultError,
    },
    ManagementIdentityMissing {
        path: PathBuf,
    },
    ManagementIdentityPath(UserRelativePathError),
    Destination(ExpandNameError),
    IdentityConfiguration(NonRoutingIdentityError),
    SharedInstanceUnavailable(ExistingSharedInstanceUnavailable),
    Path(RequestPathError),
    Link(SendError<EstablishLinkFailure>),
    Identify(SendError<IdentifyFailure>),
    Encode(RnsManagementEncodeError),
    Request(SendError<SendRequestFailure>),
    Decode(RnsInterfaceStatsDecodeError),
    Timeout {
        timeout: Duration,
    },
    NodeStopped,
}

impl fmt::Display for RemoteStatusError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration(source) => source.fmt(formatter),
            Self::ReadManagementIdentity { path, source } => write!(
                formatter,
                "could not read remote-management identity {}: {source}",
                path.display()
            ),
            Self::ManagementIdentityMissing { path } => write!(
                formatter,
                "remote-management identity {} does not exist; pass the private identity file whose hash is in the target's remote_management_allowed setting",
                path.display()
            ),
            Self::ManagementIdentityPath(source) => source.fmt(formatter),
            Self::Destination(source) => {
                write!(formatter, "could not derive the remote-management destination: {source:?}")
            }
            Self::IdentityConfiguration(source) => write!(
                formatter,
                "could not activate the remote-management identity: {source:?}"
            ),
            Self::SharedInstanceUnavailable(source) => write!(
                formatter,
                "{source}; start prnsd or a stock RNS shared instance first"
            ),
            Self::Path(source) => write!(
                formatter,
                "could not discover a path to the remote transport: {source:?}"
            ),
            Self::Link(source) => write!(
                formatter,
                "could not establish a link to remote management: {source:?}"
            ),
            Self::Identify(source) => write!(
                formatter,
                "could not identify the management link: {source:?}"
            ),
            Self::Encode(source) => {
                write!(formatter, "could not encode the stock status request: {source}")
            }
            Self::Request(source) => {
                write!(formatter, "remote /status request failed: {source:?}")
            }
            Self::Decode(source) => {
                write!(formatter, "remote /status returned an invalid response: {source}")
            }
            Self::Timeout { timeout } => write!(
                formatter,
                "remote status timed out after {:.3} seconds; increase -w if the path is slow",
                timeout.as_secs_f64()
            ),
            Self::NodeStopped => formatter.write_str(
                "the temporary remote-management client stopped before the status reply arrived",
            ),
        }
    }
}

impl std::error::Error for RemoteStatusError {}

#[cfg(test)]
mod tests {
    use super::{expand_user_path_with_home, UserRelativePathError};
    use std::path::Path;

    #[test]
    fn ordinary_identity_paths_are_not_rewritten() {
        assert_eq!(
            expand_user_path_with_home(Path::new("identities/operator"), None).unwrap(),
            Path::new("identities/operator")
        );
    }

    #[test]
    fn user_relative_identity_paths_require_a_home_directory() {
        assert!(matches!(
            expand_user_path_with_home(Path::new("~/operator"), None),
            Err(UserRelativePathError { .. })
        ));
    }
}
