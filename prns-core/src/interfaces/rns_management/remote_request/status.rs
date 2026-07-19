use rmp::Marker;

use super::super::message_pack::{MessagePackInteger, MessagePackReader};
use super::super::{MessagePackEncoder, RnsManagementEncodeError};
use super::{finish, RnsRemoteRequestDecodeError, REMOTE_REQUEST_MAXIMUM_DEPTH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RnsRemoteStatusRequest {
    InterfaceStats,
    InterfaceStatsAndLinkCount,
}

impl RnsRemoteStatusRequest {
    pub fn encode_message_pack(self) -> Result<alloc::vec::Vec<u8>, RnsManagementEncodeError> {
        let mut encoder = MessagePackEncoder::new();
        encoder.array(1)?;
        encoder.boolean(self == Self::InterfaceStatsAndLinkCount);
        Ok(encoder.finish())
    }
}

pub fn decode_remote_status_request(
    bytes: &[u8],
) -> Result<RnsRemoteStatusRequest, RnsRemoteRequestDecodeError> {
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

    let first = reader
        .marker()
        .map_err(|_| RnsRemoteRequestDecodeError::InvalidMessagePack)?;
    let include_link_count = equals_python_true(&mut reader, first)?;
    for _ in 1..length {
        let marker = reader
            .marker()
            .map_err(|_| RnsRemoteRequestDecodeError::InvalidMessagePack)?;
        reader
            .skip_value(marker, 1, REMOTE_REQUEST_MAXIMUM_DEPTH)
            .map_err(|_| RnsRemoteRequestDecodeError::InvalidMessagePack)?;
    }

    let request = if include_link_count {
        RnsRemoteStatusRequest::InterfaceStatsAndLinkCount
    } else {
        RnsRemoteStatusRequest::InterfaceStats
    };
    finish(reader, Ok(request))
}

fn equals_python_true(
    reader: &mut MessagePackReader<'_>,
    marker: Marker,
) -> Result<bool, RnsRemoteRequestDecodeError> {
    let result = match marker {
        Marker::True => true,
        Marker::False | Marker::Null => false,
        marker if MessagePackReader::is_integer(marker) => matches!(
            reader
                .integer(marker)
                .map_err(|_| RnsRemoteRequestDecodeError::InvalidMessagePack)?,
            Some(MessagePackInteger::Nonnegative(1))
        ),
        marker @ (Marker::F32 | Marker::F64) => {
            reader
                .float(marker)
                .map_err(|_| RnsRemoteRequestDecodeError::InvalidMessagePack)?
                == Some(1.0)
        }
        marker => {
            reader
                .skip_value(marker, 1, REMOTE_REQUEST_MAXIMUM_DEPTH)
                .map_err(|_| RnsRemoteRequestDecodeError::InvalidMessagePack)?;
            false
        }
    };
    Ok(result)
}
