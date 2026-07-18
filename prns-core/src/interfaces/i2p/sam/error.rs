use alloc::string::String;
use core::fmt;

use super::{SamRejection, SamReplyKind, SamValueError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SamProtocolError {
    MalformedReply(&'static str),
    InvalidToken {
        field: &'static str,
        source: SamValueError,
    },
    InvalidVersion(String),
    MissingTransientSessionDestination,
    InvalidPeerDestination(SamValueError),
    UnexpectedPeerIdentification(SamReplyKind),
    UnexpectedReply {
        expected: SamReplyKind,
        actual: SamReplyKind,
    },
    Rejected {
        kind: SamReplyKind,
        rejection: SamRejection,
        message: Option<String>,
    },
}

impl fmt::Display for SamProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedReply(reason) => write!(formatter, "malformed SAM reply: {reason}"),
            Self::InvalidToken { field, source } => {
                write!(formatter, "invalid SAM {field} field: {source}")
            }
            Self::InvalidVersion(version) => write!(formatter, "invalid SAM version {version:?}"),
            Self::MissingTransientSessionDestination => {
                formatter.write_str("SAM bridge omitted the transient session destination")
            }
            Self::InvalidPeerDestination(source) => {
                write!(formatter, "invalid SAM incoming peer destination: {source}")
            }
            Self::UnexpectedPeerIdentification(actual) => write!(
                formatter,
                "expected an incoming peer destination, received a {actual:?} reply"
            ),
            Self::UnexpectedReply { expected, actual } => {
                write!(
                    formatter,
                    "expected {expected:?} reply, received {actual:?}"
                )
            }
            Self::Rejected {
                kind,
                rejection,
                message,
            } => {
                write!(
                    formatter,
                    "SAM {kind:?} request was rejected with {rejection:?}"
                )?;
                if let Some(message) = message {
                    write!(formatter, ": {message}")?;
                }
                Ok(())
            }
        }
    }
}

impl core::error::Error for SamProtocolError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::InvalidToken { source, .. } | Self::InvalidPeerDestination(source) => {
                Some(source)
            }
            _ => None,
        }
    }
}

impl SamProtocolError {
    pub const fn rejection(&self) -> Option<&SamRejection> {
        match self {
            Self::Rejected { rejection, .. } => Some(rejection),
            _ => None,
        }
    }
}
