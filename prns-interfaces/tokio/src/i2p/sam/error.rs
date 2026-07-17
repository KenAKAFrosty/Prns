use std::fmt;
use std::io;

use super::reply::{SamRejection, SamReplyKind};
use super::value::SamValueError;

#[derive(Debug)]
pub enum SamProtocolError {
    Io(io::Error),
    EndOfStream,
    TruncatedReply,
    ReplyTooLong,
    InvalidUtf8,
    MalformedReply(&'static str),
    InvalidToken {
        field: &'static str,
        source: SamValueError,
    },
    InvalidVersion(String),
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
            Self::Io(error) => write!(formatter, "SAM I/O failed: {error}"),
            Self::EndOfStream => formatter.write_str("SAM bridge closed before replying"),
            Self::TruncatedReply => formatter.write_str("SAM bridge closed during a reply"),
            Self::ReplyTooLong => formatter.write_str("SAM reply exceeded the protocol limit"),
            Self::InvalidUtf8 => formatter.write_str("SAM reply was not UTF-8"),
            Self::MalformedReply(reason) => write!(formatter, "malformed SAM reply: {reason}"),
            Self::InvalidToken { field, source } => {
                write!(formatter, "invalid SAM {field} field: {source}")
            }
            Self::InvalidVersion(version) => write!(formatter, "invalid SAM version {version:?}"),
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

impl std::error::Error for SamProtocolError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::InvalidToken { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<io::Error> for SamProtocolError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
