use core::str;

use rmp::Marker;

use super::core::validate_request_ref;
use super::{RnxCodecError, RnxField};
use crate::message_pack::{MessagePackInteger, MessagePackReader};
use crate::rnx::{
    ExecutionRequestRef, MAX_COMMAND_BYTES, MAX_EXECUTION_REQUEST_BYTES, MAX_STDIN_BYTES,
};

pub fn decode_execution_request_ref(
    input: &[u8],
) -> Result<ExecutionRequestRef<'_>, RnxCodecError> {
    if input.len() > MAX_EXECUTION_REQUEST_BYTES {
        return Err(RnxCodecError::InvalidField(RnxField::Stdin));
    }
    let mut reader = MessagePackReader::new(input);
    let root_marker = marker(&mut reader)?;
    if reader
        .array_length(root_marker)
        .map_err(|_| RnxCodecError::MalformedMessagePack)?
        != Some(5)
    {
        return if matches!(
            root_marker,
            Marker::FixArray(_) | Marker::Array16 | Marker::Array32
        ) {
            Err(RnxCodecError::WrongFieldCount)
        } else {
            Err(RnxCodecError::ExpectedArray)
        };
    }
    let command_marker = marker(&mut reader)?;
    if !MessagePackReader::is_binary(command_marker) {
        return Err(RnxCodecError::InvalidField(RnxField::Command));
    }
    let command = reader
        .binary(command_marker)
        .map_err(|_| RnxCodecError::MalformedMessagePack)?
        .filter(|command| command.len() <= MAX_COMMAND_BYTES)
        .and_then(|command| str::from_utf8(command).ok())
        .ok_or(RnxCodecError::InvalidField(RnxField::Command))?;
    let timeout_seconds = decode_optional_number(&mut reader, RnxField::Timeout)?;
    let stdout_limit = decode_optional_unsigned(&mut reader, RnxField::StdoutLimit)?;
    let stderr_limit = decode_optional_unsigned(&mut reader, RnxField::StderrLimit)?;
    let stdin_marker = marker(&mut reader)?;
    let stdin = if stdin_marker == Marker::Null {
        None
    } else {
        if !MessagePackReader::is_binary(stdin_marker) {
            return Err(RnxCodecError::InvalidField(RnxField::Stdin));
        }
        Some(
            reader
                .binary(stdin_marker)
                .map_err(|_| RnxCodecError::MalformedMessagePack)?
                .filter(|stdin| stdin.len() <= MAX_STDIN_BYTES)
                .ok_or(RnxCodecError::InvalidField(RnxField::Stdin))?,
        )
    };
    if !reader.is_finished() {
        return Err(RnxCodecError::MalformedMessagePack);
    }
    let request = ExecutionRequestRef {
        command,
        timeout_seconds,
        stdout_limit,
        stderr_limit,
        stdin,
    };
    validate_request_ref(request)?;
    Ok(request)
}

fn marker(reader: &mut MessagePackReader<'_>) -> Result<Marker, RnxCodecError> {
    reader
        .marker()
        .map_err(|_| RnxCodecError::MalformedMessagePack)
}

fn decode_optional_number(
    reader: &mut MessagePackReader<'_>,
    field: RnxField,
) -> Result<Option<f64>, RnxCodecError> {
    let marker = marker(reader)?;
    if marker == Marker::Null {
        return Ok(None);
    }
    if !MessagePackReader::is_integer(marker) && !matches!(marker, Marker::F32 | Marker::F64) {
        return Err(RnxCodecError::InvalidField(field));
    }
    let number = match reader
        .integer(marker)
        .map_err(|_| RnxCodecError::MalformedMessagePack)?
    {
        Some(MessagePackInteger::Negative(value)) => value as f64,
        Some(MessagePackInteger::Nonnegative(value)) => value as f64,
        None => reader
            .float(marker)
            .map_err(|_| RnxCodecError::MalformedMessagePack)?
            .ok_or(RnxCodecError::InvalidField(field))?,
    };
    if number.is_finite() {
        Ok(Some(number))
    } else {
        Err(RnxCodecError::InvalidField(field))
    }
}

fn decode_optional_unsigned(
    reader: &mut MessagePackReader<'_>,
    field: RnxField,
) -> Result<Option<u64>, RnxCodecError> {
    let marker = marker(reader)?;
    if marker == Marker::Null {
        return Ok(None);
    }
    if !MessagePackReader::is_integer(marker) {
        return Err(RnxCodecError::InvalidField(field));
    }
    match reader
        .integer(marker)
        .map_err(|_| RnxCodecError::MalformedMessagePack)?
    {
        Some(MessagePackInteger::Nonnegative(value)) => Ok(Some(value)),
        _ => Err(RnxCodecError::InvalidField(field)),
    }
}
