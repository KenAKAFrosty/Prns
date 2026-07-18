use std::fmt;
use std::io;

use prns_core::interfaces::i2p::sam::SamProtocolError;

#[derive(Debug)]
pub enum SamControlError {
    Io(io::Error),
    EndOfStream,
    TruncatedReply,
    ReplyTooLong,
    InvalidUtf8,
    Protocol(SamProtocolError),
}

impl fmt::Display for SamControlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "SAM I/O failed: {error}"),
            Self::EndOfStream => formatter.write_str("SAM bridge closed before replying"),
            Self::TruncatedReply => formatter.write_str("SAM bridge closed during a reply"),
            Self::ReplyTooLong => formatter.write_str("SAM reply exceeded the protocol limit"),
            Self::InvalidUtf8 => formatter.write_str("SAM reply was not UTF-8"),
            Self::Protocol(error) => write!(formatter, "{error}"),
        }
    }
}

#[derive(Debug)]
pub enum SamStreamError {
    Control(SamControlError),
    PeerIdentification(SamProtocolError),
    PeerClosed,
    PeerDestinationTruncated,
    PeerDestinationTooLong,
    PeerDestinationInvalidUtf8,
}

impl fmt::Display for SamStreamError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Control(error) => write!(formatter, "{error}"),
            Self::PeerIdentification(error) => write!(formatter, "{error}"),
            Self::PeerClosed => {
                formatter.write_str("SAM bridge closed before identifying the incoming peer")
            }
            Self::PeerDestinationTruncated => {
                formatter.write_str("SAM bridge truncated the incoming peer destination")
            }
            Self::PeerDestinationTooLong => {
                formatter.write_str("SAM incoming peer destination exceeded the protocol limit")
            }
            Self::PeerDestinationInvalidUtf8 => {
                formatter.write_str("SAM incoming peer destination was not UTF-8")
            }
        }
    }
}

impl std::error::Error for SamStreamError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Control(error) => Some(error),
            Self::PeerIdentification(error) => Some(error),
            _ => None,
        }
    }
}

impl From<SamControlError> for SamStreamError {
    fn from(error: SamControlError) -> Self {
        Self::Control(error)
    }
}

impl std::error::Error for SamControlError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Protocol(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for SamControlError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<SamProtocolError> for SamControlError {
    fn from(error: SamProtocolError) -> Self {
        Self::Protocol(error)
    }
}

impl SamControlError {
    pub const fn protocol_error(&self) -> Option<&SamProtocolError> {
        match self {
            Self::Protocol(error) => Some(error),
            _ => None,
        }
    }
}

impl SamStreamError {
    pub const fn protocol_error(&self) -> Option<&SamProtocolError> {
        match self {
            Self::Control(error) => error.protocol_error(),
            Self::PeerIdentification(error) => Some(error),
            _ => None,
        }
    }
}
