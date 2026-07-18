use rmp::Marker;

use crate::wire::DestinationHash;

use super::super::message_pack::{MessagePackInteger, MessagePackReader};
use super::super::wire_names::remote_path;
use super::{finish, RnsRemoteRequestDecodeError, REMOTE_REQUEST_MAXIMUM_DEPTH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DestinationSelection {
    All,
    Exact(DestinationHash),
    NoMatch,
}

impl DestinationSelection {
    fn includes(self, destination: DestinationHash) -> bool {
        match self {
            Self::All => true,
            Self::Exact(selected) => selected == destination,
            Self::NoMatch => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HopSelection {
    All,
    AtMost(u64),
    NoMatch,
}

impl HopSelection {
    fn includes(self, hops: u8) -> bool {
        match self {
            Self::All => true,
            Self::AtMost(maximum) => u64::from(hops) <= maximum,
            Self::NoMatch => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RnsRemotePathTableRequest {
    pub(super) destination: DestinationSelection,
    pub(super) hops: HopSelection,
}

impl RnsRemotePathTableRequest {
    pub fn includes(&self, destination: DestinationHash, hops: u8) -> bool {
        self.destination.includes(destination) && self.hops.includes(hops)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RnsRemoteRateTableRequest {
    pub(super) destination: DestinationSelection,
}

impl RnsRemoteRateTableRequest {
    pub fn includes(&self, destination: DestinationHash) -> bool {
        self.destination.includes(destination)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RnsRemotePathRequest {
    Table(RnsRemotePathTableRequest),
    Rates(RnsRemoteRateTableRequest),
}

enum Command {
    Table,
    Rates,
    Unsupported,
    InvalidShape,
}

pub fn decode_remote_path_request(
    bytes: &[u8],
) -> Result<RnsRemotePathRequest, RnsRemoteRequestDecodeError> {
    let mut reader = MessagePackReader::new(bytes);
    let root = reader
        .marker()
        .map_err(|_| RnsRemoteRequestDecodeError::InvalidMessagePack)?;
    let Some(length) = reader
        .array_length(root)
        .map_err(|_| RnsRemoteRequestDecodeError::InvalidMessagePack)?
    else {
        reader
            .skip_value(root, 0, REMOTE_REQUEST_MAXIMUM_DEPTH)
            .map_err(|_| RnsRemoteRequestDecodeError::InvalidMessagePack)?;
        return finish(reader, Err(RnsRemoteRequestDecodeError::InvalidShape));
    };
    if length == 0 {
        return finish(reader, Err(RnsRemoteRequestDecodeError::InvalidShape));
    }

    let command = decode_command(&mut reader)?;
    let destination = if length >= 2 {
        decode_destination(&mut reader)?
    } else {
        DestinationSelection::All
    };
    let hops = if length >= 3 {
        match command {
            Command::Table => decode_hops(&mut reader)?,
            Command::Rates | Command::Unsupported | Command::InvalidShape => {
                skip_next(&mut reader, 1)?;
                Some(HopSelection::All)
            }
        }
    } else {
        Some(HopSelection::All)
    };
    for _ in 3..length {
        skip_next(&mut reader, 1)?;
    }

    let request = match command {
        Command::Table => hops.map_or(Err(RnsRemoteRequestDecodeError::InvalidShape), |hops| {
            Ok(RnsRemotePathRequest::Table(RnsRemotePathTableRequest {
                destination,
                hops,
            }))
        }),
        Command::Rates => Ok(RnsRemotePathRequest::Rates(RnsRemoteRateTableRequest {
            destination,
        })),
        Command::Unsupported => Err(RnsRemoteRequestDecodeError::UnsupportedCommand),
        Command::InvalidShape => Err(RnsRemoteRequestDecodeError::InvalidShape),
    };
    finish(reader, request)
}

fn decode_command(
    reader: &mut MessagePackReader<'_>,
) -> Result<Command, RnsRemoteRequestDecodeError> {
    let marker = reader
        .marker()
        .map_err(|_| RnsRemoteRequestDecodeError::InvalidMessagePack)?;
    if !MessagePackReader::is_string(marker) {
        reader
            .skip_value(marker, 1, REMOTE_REQUEST_MAXIMUM_DEPTH)
            .map_err(|_| RnsRemoteRequestDecodeError::InvalidMessagePack)?;
        return Ok(Command::InvalidShape);
    }
    match reader
        .string(marker)
        .map_err(|_| RnsRemoteRequestDecodeError::InvalidMessagePack)?
    {
        Some(remote_path::TABLE) => Ok(Command::Table),
        Some(remote_path::RATES) => Ok(Command::Rates),
        Some(_) => Ok(Command::Unsupported),
        None => Ok(Command::InvalidShape),
    }
}

fn decode_destination(
    reader: &mut MessagePackReader<'_>,
) -> Result<DestinationSelection, RnsRemoteRequestDecodeError> {
    let marker = reader
        .marker()
        .map_err(|_| RnsRemoteRequestDecodeError::InvalidMessagePack)?;
    if marker == Marker::Null {
        return Ok(DestinationSelection::All);
    }
    if MessagePackReader::is_binary(marker) {
        let bytes = reader
            .binary(marker)
            .map_err(|_| RnsRemoteRequestDecodeError::InvalidMessagePack)?;
        let exact = bytes.and_then(|bytes| <[u8; 16]>::try_from(bytes).ok());
        return Ok(exact.map_or(DestinationSelection::NoMatch, |bytes| {
            DestinationSelection::Exact(DestinationHash::new(bytes))
        }));
    }
    reader
        .skip_value(marker, 1, REMOTE_REQUEST_MAXIMUM_DEPTH)
        .map_err(|_| RnsRemoteRequestDecodeError::InvalidMessagePack)?;
    Ok(DestinationSelection::NoMatch)
}

fn decode_hops(
    reader: &mut MessagePackReader<'_>,
) -> Result<Option<HopSelection>, RnsRemoteRequestDecodeError> {
    let marker = reader
        .marker()
        .map_err(|_| RnsRemoteRequestDecodeError::InvalidMessagePack)?;
    if marker == Marker::Null {
        return Ok(Some(HopSelection::All));
    }
    if MessagePackReader::is_integer(marker) {
        return match reader
            .integer(marker)
            .map_err(|_| RnsRemoteRequestDecodeError::InvalidMessagePack)?
        {
            Some(MessagePackInteger::Negative(_)) => Ok(Some(HopSelection::NoMatch)),
            Some(MessagePackInteger::Nonnegative(maximum)) => {
                Ok(Some(HopSelection::AtMost(maximum)))
            }
            None => Ok(None),
        };
    }
    reader
        .skip_value(marker, 1, REMOTE_REQUEST_MAXIMUM_DEPTH)
        .map_err(|_| RnsRemoteRequestDecodeError::InvalidMessagePack)?;
    Ok(None)
}

fn skip_next(
    reader: &mut MessagePackReader<'_>,
    depth: usize,
) -> Result<(), RnsRemoteRequestDecodeError> {
    let marker = reader
        .marker()
        .map_err(|_| RnsRemoteRequestDecodeError::InvalidMessagePack)?;
    reader
        .skip_value(marker, depth, REMOTE_REQUEST_MAXIMUM_DEPTH)
        .map_err(|_| RnsRemoteRequestDecodeError::InvalidMessagePack)
}
