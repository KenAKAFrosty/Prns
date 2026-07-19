use std::fmt;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use personal_rns::engine::{EstablishLinkFailure, IdentifyFailure, SendRequestFailure};
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::vault::{read_identity_file, FileVaultError};
use personal_rns::identity::{IdentityHash, IdentitySigner};
use personal_rns::routing::announce::{derive_single_destination_hash, ExpandNameError};
use personal_rns::routing::links::LinkId;
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::runtime::SendError;

use super::configuration::LoadedConfiguration;
use super::session::{
    UtilityNodeClient, UtilityNodeIdentity, UtilityNodeSession, UtilityNodeSessionError,
    UtilityNodeStopped, UtilityPathError,
};

const APP_NAME: &str = "rnstransport";
const ASPECTS: &[&str] = &["remote", "management"];

pub struct RemoteManagementSession {
    utility: UtilityNodeClient,
    link: LinkId,
    timeout: Duration,
}

impl RemoteManagementSession {
    pub async fn run<T, F, Operation>(
        configuration: &LoadedConfiguration,
        transport_identity: IdentityHash,
        management_identity_path: &Path,
        timeout: Duration,
        operation: F,
    ) -> Result<T, RemoteManagementError>
    where
        F: FnOnce(RemoteManagementSession) -> Operation,
        Operation: Future<Output = Result<T, RemoteManagementError>>,
    {
        let operation = async {
            let path = expand_user_path(management_identity_path)
                .map_err(RemoteManagementError::ManagementIdentityPath)?;
            let identity = read_identity_file(&path)
                .map_err(|source| RemoteManagementError::ReadManagementIdentity {
                    path: path.clone(),
                    source,
                })?
                .ok_or_else(|| RemoteManagementError::ManagementIdentityMissing {
                    path: path.clone(),
                })?;
            let identity_hash =
                InMemoryNodeIdentity::from_secret_key_bytes(&identity).identity_hash();
            let destination =
                derive_single_destination_hash(&transport_identity, APP_NAME, ASPECTS)
                    .map_err(RemoteManagementError::Destination)?;
            let utility = UtilityNodeSession::connect(
                configuration,
                UtilityNodeIdentity::Private(identity),
                timeout,
            )
            .await
            .map_err(RemoteManagementError::Session)?;
            utility
                .run(move |utility| async move {
                    utility
                        .ensure_path(destination, timeout)
                        .await
                        .map_err(RemoteManagementError::Path)?;
                    let link = utility
                        .handle()
                        .establish_link(destination)
                        .await
                        .map_err(RemoteManagementError::Link)?;
                    utility
                        .handle()
                        .identify(link, identity_hash)
                        .await
                        .map_err(RemoteManagementError::Identify)?;
                    operation(Self {
                        utility,
                        link,
                        timeout,
                    })
                    .await
                })
                .await
                .map_err(RemoteManagementError::NodeStopped)?
        };
        tokio::time::timeout(timeout, operation)
            .await
            .map_err(|_| RemoteManagementError::Timeout {
                phase: RemoteManagementPhase::Connect,
                timeout,
            })?
    }

    pub async fn request(&self, path: &str, data: &[u8]) -> Result<Vec<u8>, RemoteManagementError> {
        let request = self
            .utility
            .handle()
            .request(self.link, RequestPathHash::of(path), data);
        let (response, _) = tokio::time::timeout(self.timeout, request)
            .await
            .map_err(|_| RemoteManagementError::Timeout {
                phase: RemoteManagementPhase::Request,
                timeout: self.timeout,
            })?
            .map_err(RemoteManagementError::Request)?;
        Ok(response)
    }
}

impl Drop for RemoteManagementSession {
    fn drop(&mut self) {
        self.utility.handle().close_link(self.link);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteManagementPhase {
    Connect,
    Request,
}

impl fmt::Display for RemoteManagementPhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Connect => "connection",
            Self::Request => "request",
        })
    }
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
pub enum RemoteManagementError {
    ReadManagementIdentity {
        path: PathBuf,
        source: FileVaultError,
    },
    ManagementIdentityMissing {
        path: PathBuf,
    },
    ManagementIdentityPath(UserRelativePathError),
    Destination(ExpandNameError),
    Session(UtilityNodeSessionError),
    Path(UtilityPathError),
    Link(SendError<EstablishLinkFailure>),
    Identify(SendError<IdentifyFailure>),
    Request(SendError<SendRequestFailure>),
    Timeout {
        phase: RemoteManagementPhase,
        timeout: Duration,
    },
    NodeStopped(UtilityNodeStopped),
}

impl fmt::Display for RemoteManagementError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
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
            Self::Destination(source) => write!(
                formatter,
                "could not derive the remote-management destination: {source:?}"
            ),
            Self::Session(source) => source.fmt(formatter),
            Self::Path(source) => write!(
                formatter,
                "could not discover a path to the remote transport: {source}"
            ),
            Self::Link(source) => write!(
                formatter,
                "could not establish a link to remote management: {source:?}"
            ),
            Self::Identify(source) => write!(
                formatter,
                "could not identify the management link: {source:?}"
            ),
            Self::Request(source) => write!(formatter, "remote request failed: {source:?}"),
            Self::Timeout { phase, timeout } => write!(
                formatter,
                "remote-management {phase} timed out after {:.3} seconds",
                timeout.as_secs_f64()
            ),
            Self::NodeStopped(source) => source.fmt(formatter),
        }
    }
}

impl std::error::Error for RemoteManagementError {}

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
