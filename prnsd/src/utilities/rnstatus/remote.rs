use std::fmt;
use std::path::Path;
use std::time::Duration;

use personal_rns::identity::IdentityHash;
use personal_rns::interfaces::rns_management::{
    RnsInterfaceStatsDecodeError, RnsManagementEncodeError, RnsRemoteInterfaceStatsReport,
    RnsRemoteStatusRequest,
};

use super::super::configuration::LoadedConfiguration;
use super::super::remote_management::{RemoteManagementError, RemoteManagementSession};

const STATUS_PATH: &str = "/status";

pub async fn query(
    configuration: &LoadedConfiguration,
    transport_identity: IdentityHash,
    management_identity_path: &Path,
    include_link_count: bool,
    timeout: Duration,
) -> Result<RnsRemoteInterfaceStatsReport, RemoteStatusError> {
    let request = if include_link_count {
        RnsRemoteStatusRequest::InterfaceStatsAndLinkCount
    } else {
        RnsRemoteStatusRequest::InterfaceStats
    };
    let request = request
        .encode_message_pack()
        .map_err(RemoteStatusError::Encode)?;
    let response = RemoteManagementSession::run(
        configuration,
        transport_identity,
        management_identity_path,
        timeout,
        move |session| async move { session.request(STATUS_PATH, &request).await },
    )
    .await
    .map_err(RemoteStatusError::Remote)?;
    RnsRemoteInterfaceStatsReport::decode_message_pack(&response).map_err(RemoteStatusError::Decode)
}

#[derive(Debug)]
pub enum RemoteStatusError {
    Remote(RemoteManagementError),
    Encode(RnsManagementEncodeError),
    Decode(RnsInterfaceStatsDecodeError),
}

impl fmt::Display for RemoteStatusError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Remote(source) => source.fmt(formatter),
            Self::Encode(source) => {
                write!(
                    formatter,
                    "could not encode the stock status request: {source}"
                )
            }
            Self::Decode(source) => {
                write!(
                    formatter,
                    "remote /status returned an invalid response: {source}"
                )
            }
        }
    }
}

impl std::error::Error for RemoteStatusError {}
