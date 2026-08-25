use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use personal_rns::engine::{EstablishLinkFailure, IdentifyFailure, SendRequestFailure};
use personal_rns::rnx::RnxCodecError;
use personal_rns::routing::announce::ExpandNameError;
use personal_rns::routing::request_handlers::RequestHandlerError;
use personal_rns::runtime::{AnnounceNowError, IdentitySecretFileError, NodeRunError, SendError};
use personal_rns::shared_instance::ExistingSharedInstanceUnavailable;
use personal_rns::wire::DestinationHash;

use crate::utilities::configuration::UtilityConfigurationError;
use crate::utilities::session::{UtilityNodeSessionError, UtilityNodeStopped, UtilityPathError};

use super::identity::pretty_hash;

#[derive(Debug)]
pub enum RnxError {
    Configuration(UtilityConfigurationError),
    Identity {
        path: PathBuf,
        source: IdentitySecretFileError,
    },
    HomeUnavailable(PathBuf),
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    AllowedIdentity(PathBuf),
    Destination(ExpandNameError),
    DestinationNameMismatch,
    IdentityUnavailable(DestinationHash),
    Session(UtilityNodeSessionError),
    NodeStopped(UtilityNodeStopped),
    Path(UtilityPathError),
    Link(SendError<EstablishLinkFailure>),
    Identify(SendError<IdentifyFailure>),
    Request(SendError<SendRequestFailure>),
    ResponseTimeout(Duration),
    Encode(RnxCodecError),
    ResponseCodec(RnxCodecError),
    RemoteCouldNotExecute,
    SharedInstance(ExistingSharedInstanceUnavailable),
    RequestAcl(RequestHandlerError),
    Announce(AnnounceNowError),
    ListenerStopped,
    ListenerPanicked(NodeRunError),
    Stdin(std::io::Error),
    Stdout(std::io::Error),
    Stderr(std::io::Error),
}

impl fmt::Display for RnxError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration(source) => source.fmt(formatter),
            Self::Identity { path, source } => {
                write!(
                    formatter,
                    "could not load identity {}: {source}",
                    path.display()
                )
            }
            Self::HomeUnavailable(path) => write!(
                formatter,
                "path {} requires a home directory, but neither HOME nor USERPROFILE is available",
                path.display()
            ),
            Self::Io { path, source } => {
                write!(formatter, "could not read {}: {source}", path.display())
            }
            Self::AllowedIdentity(path) => {
                write!(formatter, "invalid identity in {}", path.display())
            }
            Self::Destination(source) => write!(formatter, "invalid x destination: {source:?}"),
            Self::DestinationNameMismatch => {
                formatter.write_str("destination is not an rnx.execute destination")
            }
            Self::IdentityUnavailable(destination) => write!(
                formatter,
                "identity for destination {} is unavailable",
                pretty_hash(destination.as_bytes())
            ),
            Self::Session(source) => source.fmt(formatter),
            Self::NodeStopped(source) => source.fmt(formatter),
            Self::Path(source) => source.fmt(formatter),
            Self::Link(source) => write!(formatter, "link establishment failed: {source:?}"),
            Self::Identify(source) => write!(formatter, "link identification failed: {source:?}"),
            Self::Request(source) => write!(formatter, "execution request failed: {source:?}"),
            Self::ResponseTimeout(timeout) => write!(
                formatter,
                "execution response timed out after {} seconds",
                timeout.as_secs_f64()
            ),
            Self::Encode(source) => write!(formatter, "could not encode x request: {source:?}"),
            Self::ResponseCodec(source) => {
                write!(formatter, "received invalid x result: {source:?}")
            }
            Self::RemoteCouldNotExecute => formatter.write_str("remote could not execute command"),
            Self::SharedInstance(source) => source.fmt(formatter),
            Self::RequestAcl(source) => {
                write!(formatter, "could not configure command ACL: {source:?}")
            }
            Self::Announce(source) => write!(formatter, "x announce failed: {source:?}"),
            Self::ListenerStopped => formatter.write_str("x listener stopped"),
            Self::ListenerPanicked(source) => write!(formatter, "x listener stopped: {source}"),
            Self::Stdin(source) => write!(formatter, "could not read stdin: {source}"),
            Self::Stdout(source) => write!(formatter, "could not write stdout: {source}"),
            Self::Stderr(source) => write!(formatter, "could not write stderr: {source}"),
        }
    }
}

impl std::error::Error for RnxError {}

impl RnxError {
    pub const fn exit_code(&self) -> u8 {
        match self {
            Self::Destination(_) | Self::DestinationNameMismatch | Self::IdentityUnavailable(_) => {
                241
            }
            Self::Path(_) => 242,
            Self::Link(_) => 243,
            Self::Identify(_) | Self::Request(_) => 244,
            Self::ResponseTimeout(_) => 246,
            Self::ResponseCodec(_) => 247,
            Self::RemoteCouldNotExecute => 248,
            Self::Configuration(_)
            | Self::Identity { .. }
            | Self::HomeUnavailable(_)
            | Self::Io { .. }
            | Self::AllowedIdentity(_)
            | Self::Session(_)
            | Self::NodeStopped(_)
            | Self::Encode(_)
            | Self::SharedInstance(_)
            | Self::RequestAcl(_)
            | Self::Announce(_)
            | Self::ListenerStopped
            | Self::ListenerPanicked(_)
            | Self::Stdin(_)
            | Self::Stdout(_)
            | Self::Stderr(_) => 1,
        }
    }
}
