use std::collections::BTreeMap;

use super::error::SamProtocolError;
use super::value::{I2pAddress, I2pPrivateDestination, I2pPublicDestination, SamValueError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SamVersion {
    pub major: u8,
    pub minor: u8,
}

impl SamVersion {
    pub const V3_1: Self = Self { major: 3, minor: 1 };
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SamRejection {
    NoVersion,
    DuplicatedDestination,
    DuplicatedId,
    I2pError,
    InvalidKey,
    InvalidId,
    CantReachPeer,
    Timeout,
    KeyNotFound,
    PeerNotFound,
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SamReplyKind {
    Hello,
    Destination,
    Session,
    Stream,
    Naming,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SamReply {
    Hello(SamVersion),
    DestinationGenerated {
        public: I2pPublicDestination,
        private: I2pPrivateDestination,
    },
    SessionCreated {
        destination: I2pPrivateDestination,
    },
    StreamReady,
    NameResolved {
        name: I2pAddress,
        destination: I2pPublicDestination,
    },
    Rejected {
        kind: SamReplyKind,
        rejection: SamRejection,
        message: Option<String>,
    },
}

impl SamReply {
    pub const fn kind(&self) -> SamReplyKind {
        match self {
            Self::Hello(_) => SamReplyKind::Hello,
            Self::DestinationGenerated { .. } => SamReplyKind::Destination,
            Self::SessionCreated { .. } => SamReplyKind::Session,
            Self::StreamReady => SamReplyKind::Stream,
            Self::NameResolved { .. } => SamReplyKind::Naming,
            Self::Rejected { kind, .. } => *kind,
        }
    }
}

pub(super) fn parse_reply(line: &str) -> Result<SamReply, SamProtocolError> {
    let words = tokenize(line.trim_end_matches(['\r', '\n']))?;
    let domain = words
        .first()
        .ok_or(SamProtocolError::MalformedReply("missing reply domain"))?;
    let action = words
        .get(1)
        .ok_or(SamProtocolError::MalformedReply("missing reply action"))?;
    let mut fields = fields(&words[2..])?;
    let kind = reply_kind(domain, action)?;
    let rejection = match fields.remove("RESULT") {
        Some(result) => sam_rejection(&result),
        None if kind == SamReplyKind::Destination => None,
        None => return Err(SamProtocolError::MalformedReply("missing result")),
    };
    let message = fields.remove("MESSAGE");
    if let Some(rejection) = rejection {
        return Ok(SamReply::Rejected {
            kind,
            rejection,
            message,
        });
    }
    match kind {
        SamReplyKind::Hello => {
            let version = required(&mut fields, "VERSION", "missing negotiated version")?;
            Ok(SamReply::Hello(parse_version(version)?))
        }
        SamReplyKind::Destination => Ok(SamReply::DestinationGenerated {
            public: required_token(&mut fields, "PUB", "missing public destination")?,
            private: required_token(&mut fields, "PRIV", "missing private destination")?,
        }),
        SamReplyKind::Session => Ok(SamReply::SessionCreated {
            destination: required_token(&mut fields, "DESTINATION", "missing session destination")?,
        }),
        SamReplyKind::Stream => Ok(SamReply::StreamReady),
        SamReplyKind::Naming => Ok(SamReply::NameResolved {
            name: required_token(&mut fields, "NAME", "missing lookup name")?,
            destination: required_token(&mut fields, "VALUE", "missing lookup destination")?,
        }),
    }
}

fn tokenize(line: &str) -> Result<Vec<String>, SamProtocolError> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for character in line.chars() {
        if escaped {
            current.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '"' {
            quoted = !quoted;
        } else if character.is_whitespace() && !quoted {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
        } else {
            current.push(character);
        }
    }
    if escaped || quoted {
        return Err(SamProtocolError::MalformedReply(
            "unterminated escape or quoted value",
        ));
    }
    if !current.is_empty() {
        words.push(current);
    }
    Ok(words)
}

fn fields(words: &[String]) -> Result<BTreeMap<String, String>, SamProtocolError> {
    let mut fields = BTreeMap::new();
    for word in words {
        let (key, value) = word
            .split_once('=')
            .ok_or(SamProtocolError::MalformedReply("field has no value"))?;
        if key.is_empty() || fields.insert(key.to_string(), value.to_string()).is_some() {
            return Err(SamProtocolError::MalformedReply(
                "field name is empty or duplicated",
            ));
        }
    }
    Ok(fields)
}

fn reply_kind(domain: &str, action: &str) -> Result<SamReplyKind, SamProtocolError> {
    match (domain, action) {
        ("HELLO", "REPLY") => Ok(SamReplyKind::Hello),
        ("DEST", "REPLY") => Ok(SamReplyKind::Destination),
        ("SESSION", "STATUS") => Ok(SamReplyKind::Session),
        ("STREAM", "STATUS") => Ok(SamReplyKind::Stream),
        ("NAMING", "REPLY") => Ok(SamReplyKind::Naming),
        _ => Err(SamProtocolError::MalformedReply("unknown reply kind")),
    }
}

fn required(
    fields: &mut BTreeMap<String, String>,
    key: &str,
    reason: &'static str,
) -> Result<String, SamProtocolError> {
    fields
        .remove(key)
        .filter(|value| !value.is_empty())
        .ok_or(SamProtocolError::MalformedReply(reason))
}

fn required_token<Token>(
    fields: &mut BTreeMap<String, String>,
    key: &'static str,
    reason: &'static str,
) -> Result<Token, SamProtocolError>
where
    Token: TryFrom<String, Error = SamValueError>,
{
    required(fields, key, reason)?
        .try_into()
        .map_err(|source| SamProtocolError::InvalidToken { field: key, source })
}

fn parse_version(version: String) -> Result<SamVersion, SamProtocolError> {
    let (major, minor) = version
        .split_once('.')
        .ok_or_else(|| SamProtocolError::InvalidVersion(version.clone()))?;
    Ok(SamVersion {
        major: major
            .parse()
            .map_err(|_| SamProtocolError::InvalidVersion(version.clone()))?,
        minor: minor
            .parse()
            .map_err(|_| SamProtocolError::InvalidVersion(version))?,
    })
}

fn sam_rejection(result: &str) -> Option<SamRejection> {
    match result {
        "OK" => None,
        "NOVERSION" => Some(SamRejection::NoVersion),
        "DUPLICATED_DEST" => Some(SamRejection::DuplicatedDestination),
        "DUPLICATED_ID" => Some(SamRejection::DuplicatedId),
        "I2P_ERROR" => Some(SamRejection::I2pError),
        "INVALID_KEY" => Some(SamRejection::InvalidKey),
        "INVALID_ID" => Some(SamRejection::InvalidId),
        "CANT_REACH_PEER" => Some(SamRejection::CantReachPeer),
        "TIMEOUT" => Some(SamRejection::Timeout),
        "KEY_NOT_FOUND" => Some(SamRejection::KeyNotFound),
        "PEER_NOT_FOUND" => Some(SamRejection::PeerNotFound),
        other => Some(SamRejection::Other(other.to_string())),
    }
}
