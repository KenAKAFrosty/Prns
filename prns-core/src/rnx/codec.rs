#[cfg(feature = "alloc")]
use alloc::string::String;
#[cfg(feature = "alloc")]
use alloc::vec;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;
use core::str;

use rmp::encode;
use rmp::Marker;

#[cfg(feature = "alloc")]
use crate::message_pack::{
    decode_owned, encode_owned, MessagePackDecodeLimits, MessagePackOwnedError, MessagePackValue,
};
use crate::message_pack::{MessagePackInteger, MessagePackReader};

#[cfg(feature = "alloc")]
use super::{ExecutedCommand, ExecutionRequest, ExecutionResult};
use super::{
    ExecutionConclusion, ExecutionRequestRef, ExecutionResultRef, MAX_COMMAND_BYTES,
    MAX_EXECUTION_REQUEST_BYTES, MAX_RETURNED_STREAM_BYTES, MAX_STDIN_BYTES,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RnxField {
    Command,
    Timeout,
    StdoutLimit,
    StderrLimit,
    Stdin,
    Executed,
    ReturnCode,
    Stdout,
    Stderr,
    TotalStdout,
    TotalStderr,
    Started,
    Concluded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RnxCodecError {
    #[cfg(feature = "alloc")]
    MessagePack(MessagePackOwnedError),
    MalformedMessagePack,
    BufferTooShort,
    ExpectedArray,
    WrongFieldCount,
    InvalidField(RnxField),
    InvalidUtf8,
    IncoherentResult,
}

pub trait RnxEncodeSink {
    type Error;

    fn put(&mut self, bytes: &[u8]) -> Result<(), Self::Error>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeExecutionResultError<E> {
    Codec(RnxCodecError),
    Sink(E),
}

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
    let timeout_seconds = decode_borrowed_optional_number(&mut reader, RnxField::Timeout)?;
    let stdout_limit = decode_borrowed_optional_unsigned(&mut reader, RnxField::StdoutLimit)?;
    let stderr_limit = decode_borrowed_optional_unsigned(&mut reader, RnxField::StderrLimit)?;
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

pub fn encode_execution_result_to(
    result: ExecutionResultRef<'_>,
    output: &mut [u8],
) -> Result<usize, RnxCodecError> {
    let capacity = output.len();
    let mut sink = SliceSink { remaining: output };
    encode_execution_result_into(result, &mut sink).map_err(|error| match error {
        EncodeExecutionResultError::Codec(error) => error,
        EncodeExecutionResultError::Sink(()) => RnxCodecError::BufferTooShort,
    })?;
    Ok(capacity - sink.remaining.len())
}

pub fn encode_execution_result_into<S: RnxEncodeSink>(
    result: ExecutionResultRef<'_>,
    sink: &mut S,
) -> Result<(), EncodeExecutionResultError<S::Error>> {
    validate_result_ref(result).map_err(EncodeExecutionResultError::Codec)?;
    write_array_len(sink, 8)?;
    match result {
        ExecutionResultRef::NotExecuted { started_at } => {
            write_bool(sink, false)?;
            for _ in 0..5 {
                write_nil(sink)?;
            }
            write_f64(sink, started_at)?;
            write_nil(sink)?;
        }
        ExecutionResultRef::Executed(executed) => {
            write_bool(sink, true)?;
            match executed.return_code {
                Some(code) => write_i64(sink, i64::from(code))?,
                None => write_nil(sink)?,
            }
            write_binary(sink, executed.stdout)?;
            write_binary(sink, executed.stderr)?;
            write_u64(sink, executed.total_stdout)?;
            write_u64(sink, executed.total_stderr)?;
            write_f64(sink, executed.started_at)?;
            match executed.conclusion {
                ExecutionConclusion::CompletedAt(at) => write_f64(sink, at)?,
                ExecutionConclusion::TimedOut => write_nil(sink)?,
            }
        }
    }
    Ok(())
}

#[cfg(feature = "alloc")]
pub fn encode_execution_request(request: &ExecutionRequest) -> Result<Vec<u8>, RnxCodecError> {
    validate_request(request)?;
    let encoded = encode_owned(&MessagePackValue::Array(vec![
        MessagePackValue::Binary(request.command.as_bytes().to_vec()),
        optional_float(request.timeout_seconds),
        optional_unsigned(request.stdout_limit),
        optional_unsigned(request.stderr_limit),
        request
            .stdin
            .as_ref()
            .map_or(MessagePackValue::Nil, |stdin| {
                MessagePackValue::Binary(stdin.clone())
            }),
    ]))
    .map_err(RnxCodecError::MessagePack)?;
    if encoded.len() > MAX_EXECUTION_REQUEST_BYTES {
        return Err(RnxCodecError::InvalidField(RnxField::Stdin));
    }
    Ok(encoded)
}

#[cfg(feature = "alloc")]
pub fn decode_execution_request(input: &[u8]) -> Result<ExecutionRequest, RnxCodecError> {
    if input.len() > MAX_EXECUTION_REQUEST_BYTES {
        return Err(RnxCodecError::MessagePack(
            MessagePackOwnedError::LimitExceeded,
        ));
    }
    let fields = array(
        decode_owned(
            input,
            MessagePackDecodeLimits {
                maximum_depth: 1,
                maximum_values: 6,
                maximum_container_length: 5,
                maximum_blob_length: MAX_STDIN_BYTES,
            },
        )
        .map_err(RnxCodecError::MessagePack)?,
        5,
    )?;
    let mut fields = fields.into_iter();
    let command = match next(&mut fields)? {
        MessagePackValue::Binary(command) if command.len() <= MAX_COMMAND_BYTES => {
            String::from_utf8(command).map_err(|_| RnxCodecError::InvalidUtf8)?
        }
        _ => return Err(RnxCodecError::InvalidField(RnxField::Command)),
    };
    let timeout_seconds = decode_optional_number(next(&mut fields)?, RnxField::Timeout)?;
    let stdout_limit = decode_optional_unsigned(next(&mut fields)?, RnxField::StdoutLimit)?;
    let stderr_limit = decode_optional_unsigned(next(&mut fields)?, RnxField::StderrLimit)?;
    let stdin = match next(&mut fields)? {
        MessagePackValue::Nil => None,
        MessagePackValue::Binary(stdin) => Some(stdin),
        _ => return Err(RnxCodecError::InvalidField(RnxField::Stdin)),
    };
    let request = ExecutionRequest {
        command,
        timeout_seconds,
        stdout_limit,
        stderr_limit,
        stdin,
    };
    validate_request(&request)?;
    Ok(request)
}

#[cfg(feature = "alloc")]
pub fn encode_execution_result(result: &ExecutionResult) -> Result<Vec<u8>, RnxCodecError> {
    let fields = match result {
        ExecutionResult::NotExecuted { started_at } => {
            validate_timestamp(*started_at, RnxField::Started)?;
            vec![
                MessagePackValue::Boolean(false),
                MessagePackValue::Nil,
                MessagePackValue::Nil,
                MessagePackValue::Nil,
                MessagePackValue::Nil,
                MessagePackValue::Nil,
                MessagePackValue::Float(*started_at),
                MessagePackValue::Nil,
            ]
        }
        ExecutionResult::Executed(executed) => {
            validate_executed(executed)?;
            vec![
                MessagePackValue::Boolean(true),
                executed.return_code.map_or(MessagePackValue::Nil, |code| {
                    MessagePackValue::Signed(i64::from(code))
                }),
                MessagePackValue::Binary(executed.stdout.clone()),
                MessagePackValue::Binary(executed.stderr.clone()),
                MessagePackValue::Unsigned(executed.total_stdout),
                MessagePackValue::Unsigned(executed.total_stderr),
                MessagePackValue::Float(executed.started_at),
                match executed.conclusion {
                    ExecutionConclusion::CompletedAt(at) => MessagePackValue::Float(at),
                    ExecutionConclusion::TimedOut => MessagePackValue::Nil,
                },
            ]
        }
    };
    encode_owned(&MessagePackValue::Array(fields)).map_err(RnxCodecError::MessagePack)
}

#[cfg(feature = "alloc")]
pub fn decode_execution_result(input: &[u8]) -> Result<ExecutionResult, RnxCodecError> {
    let fields = array(
        decode_owned(
            input,
            MessagePackDecodeLimits {
                maximum_depth: 1,
                maximum_values: 9,
                maximum_container_length: 8,
                maximum_blob_length: MAX_RETURNED_STREAM_BYTES,
            },
        )
        .map_err(RnxCodecError::MessagePack)?,
        8,
    )?;
    let mut fields = fields.into_iter();
    let executed = match next(&mut fields)? {
        MessagePackValue::Boolean(executed) => executed,
        _ => return Err(RnxCodecError::InvalidField(RnxField::Executed)),
    };
    let return_code = next(&mut fields)?;
    let stdout = next(&mut fields)?;
    let stderr = next(&mut fields)?;
    let total_stdout = next(&mut fields)?;
    let total_stderr = next(&mut fields)?;
    let started_at = decode_number(next(&mut fields)?, RnxField::Started)?;
    validate_timestamp(started_at, RnxField::Started)?;
    let concluded = next(&mut fields)?;
    if !executed {
        if !matches!(return_code, MessagePackValue::Nil)
            || !matches!(stdout, MessagePackValue::Nil)
            || !matches!(stderr, MessagePackValue::Nil)
            || !matches!(total_stdout, MessagePackValue::Nil)
            || !matches!(total_stderr, MessagePackValue::Nil)
            || !matches!(concluded, MessagePackValue::Nil)
        {
            return Err(RnxCodecError::IncoherentResult);
        }
        return Ok(ExecutionResult::NotExecuted { started_at });
    }
    let return_code = decode_optional_i32(return_code, RnxField::ReturnCode)?;
    let MessagePackValue::Binary(stdout) = stdout else {
        return Err(RnxCodecError::InvalidField(RnxField::Stdout));
    };
    let MessagePackValue::Binary(stderr) = stderr else {
        return Err(RnxCodecError::InvalidField(RnxField::Stderr));
    };
    let total_stdout = decode_unsigned(total_stdout, RnxField::TotalStdout)?;
    let total_stderr = decode_unsigned(total_stderr, RnxField::TotalStderr)?;
    let conclusion = match concluded {
        MessagePackValue::Nil => ExecutionConclusion::TimedOut,
        value => {
            let concluded_at = decode_number(value, RnxField::Concluded)?;
            validate_timestamp(concluded_at, RnxField::Concluded)?;
            ExecutionConclusion::CompletedAt(concluded_at)
        }
    };
    let executed = ExecutedCommand {
        return_code,
        stdout,
        stderr,
        total_stdout,
        total_stderr,
        started_at,
        conclusion,
    };
    validate_executed(&executed)?;
    Ok(ExecutionResult::Executed(executed))
}

#[cfg(feature = "alloc")]
fn validate_request(request: &ExecutionRequest) -> Result<(), RnxCodecError> {
    if request.command.len() > MAX_COMMAND_BYTES {
        return Err(RnxCodecError::InvalidField(RnxField::Command));
    }
    if request
        .stdin
        .as_ref()
        .is_some_and(|stdin| stdin.len() > MAX_STDIN_BYTES)
    {
        return Err(RnxCodecError::InvalidField(RnxField::Stdin));
    }
    if request
        .timeout_seconds
        .is_some_and(|timeout| !timeout.is_finite() || timeout < 0.0)
    {
        return Err(RnxCodecError::InvalidField(RnxField::Timeout));
    }
    Ok(())
}

#[cfg(feature = "alloc")]
fn validate_executed(executed: &ExecutedCommand) -> Result<(), RnxCodecError> {
    validate_timestamp(executed.started_at, RnxField::Started)?;
    if executed.stdout.len() > MAX_RETURNED_STREAM_BYTES
        || executed.stderr.len() > MAX_RETURNED_STREAM_BYTES
        || executed.total_stdout < executed.stdout.len() as u64
        || executed.total_stderr < executed.stderr.len() as u64
    {
        return Err(RnxCodecError::IncoherentResult);
    }
    if let ExecutionConclusion::CompletedAt(concluded_at) = executed.conclusion {
        validate_timestamp(concluded_at, RnxField::Concluded)?;
        if concluded_at < executed.started_at {
            return Err(RnxCodecError::IncoherentResult);
        }
    }
    Ok(())
}

fn validate_timestamp(timestamp: f64, field: RnxField) -> Result<(), RnxCodecError> {
    if timestamp.is_finite() && timestamp >= 0.0 {
        Ok(())
    } else {
        Err(RnxCodecError::InvalidField(field))
    }
}

fn validate_request_ref(request: ExecutionRequestRef<'_>) -> Result<(), RnxCodecError> {
    if request.command.len() > MAX_COMMAND_BYTES {
        return Err(RnxCodecError::InvalidField(RnxField::Command));
    }
    if request
        .stdin
        .is_some_and(|stdin| stdin.len() > MAX_STDIN_BYTES)
    {
        return Err(RnxCodecError::InvalidField(RnxField::Stdin));
    }
    if request
        .timeout_seconds
        .is_some_and(|timeout| !timeout.is_finite() || timeout < 0.0)
    {
        return Err(RnxCodecError::InvalidField(RnxField::Timeout));
    }
    Ok(())
}

fn validate_result_ref(result: ExecutionResultRef<'_>) -> Result<(), RnxCodecError> {
    let executed = match result {
        ExecutionResultRef::NotExecuted { started_at } => {
            return validate_timestamp(started_at, RnxField::Started);
        }
        ExecutionResultRef::Executed(executed) => executed,
    };
    validate_timestamp(executed.started_at, RnxField::Started)?;
    if executed.stdout.len() > MAX_RETURNED_STREAM_BYTES
        || executed.stderr.len() > MAX_RETURNED_STREAM_BYTES
        || executed.total_stdout < executed.stdout.len() as u64
        || executed.total_stderr < executed.stderr.len() as u64
    {
        return Err(RnxCodecError::IncoherentResult);
    }
    if let ExecutionConclusion::CompletedAt(concluded_at) = executed.conclusion {
        validate_timestamp(concluded_at, RnxField::Concluded)?;
        if concluded_at < executed.started_at {
            return Err(RnxCodecError::IncoherentResult);
        }
    }
    Ok(())
}

fn marker(reader: &mut MessagePackReader<'_>) -> Result<Marker, RnxCodecError> {
    reader
        .marker()
        .map_err(|_| RnxCodecError::MalformedMessagePack)
}

fn decode_borrowed_optional_number(
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

fn decode_borrowed_optional_unsigned(
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

struct SliceSink<'a> {
    remaining: &'a mut [u8],
}

impl RnxEncodeSink for SliceSink<'_> {
    type Error = ();

    fn put(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        if bytes.len() > self.remaining.len() {
            return Err(());
        }
        let remaining = core::mem::take(&mut self.remaining);
        let (written, tail) = remaining.split_at_mut(bytes.len());
        written.copy_from_slice(bytes);
        self.remaining = tail;
        Ok(())
    }
}

fn write_array_len<S: RnxEncodeSink>(
    sink: &mut S,
    length: u32,
) -> Result<(), EncodeExecutionResultError<S::Error>> {
    write_header(sink, |output| {
        encode::write_array_len(output, length)
            .map(|_| ())
            .map_err(|_| ())
    })
}

fn write_bool<S: RnxEncodeSink>(
    sink: &mut S,
    value: bool,
) -> Result<(), EncodeExecutionResultError<S::Error>> {
    write_header(sink, |output| {
        encode::write_bool(output, value).map_err(|_| ())
    })
}

fn write_nil<S: RnxEncodeSink>(sink: &mut S) -> Result<(), EncodeExecutionResultError<S::Error>> {
    write_header(sink, |output| encode::write_nil(output).map_err(|_| ()))
}

fn write_i64<S: RnxEncodeSink>(
    sink: &mut S,
    value: i64,
) -> Result<(), EncodeExecutionResultError<S::Error>> {
    write_header(sink, |output| {
        encode::write_sint(output, value)
            .map(|_| ())
            .map_err(|_| ())
    })
}

fn write_u64<S: RnxEncodeSink>(
    sink: &mut S,
    value: u64,
) -> Result<(), EncodeExecutionResultError<S::Error>> {
    write_header(sink, |output| {
        encode::write_uint(output, value)
            .map(|_| ())
            .map_err(|_| ())
    })
}

fn write_f64<S: RnxEncodeSink>(
    sink: &mut S,
    value: f64,
) -> Result<(), EncodeExecutionResultError<S::Error>> {
    write_header(sink, |output| {
        encode::write_f64(output, value).map(|_| ()).map_err(|_| ())
    })
}

fn write_binary<S: RnxEncodeSink>(
    sink: &mut S,
    value: &[u8],
) -> Result<(), EncodeExecutionResultError<S::Error>> {
    let length = u32::try_from(value.len())
        .map_err(|_| EncodeExecutionResultError::Codec(RnxCodecError::IncoherentResult))?;
    write_header(sink, |output| {
        encode::write_bin_len(output, length)
            .map(|_| ())
            .map_err(|_| ())
    })?;
    sink.put(value).map_err(EncodeExecutionResultError::Sink)
}

fn write_header<S: RnxEncodeSink>(
    sink: &mut S,
    encode: impl FnOnce(&mut &mut [u8]) -> Result<(), ()>,
) -> Result<(), EncodeExecutionResultError<S::Error>> {
    let mut header = [0u8; 9];
    let capacity = header.len();
    let mut remaining = header.as_mut_slice();
    encode(&mut remaining)
        .map_err(|()| EncodeExecutionResultError::Codec(RnxCodecError::IncoherentResult))?;
    let written = capacity - remaining.len();
    sink.put(&header[..written])
        .map_err(EncodeExecutionResultError::Sink)
}

#[cfg(feature = "alloc")]
fn array(value: MessagePackValue, length: usize) -> Result<Vec<MessagePackValue>, RnxCodecError> {
    match value {
        MessagePackValue::Array(fields) if fields.len() == length => Ok(fields),
        MessagePackValue::Array(_) => Err(RnxCodecError::WrongFieldCount),
        _ => Err(RnxCodecError::ExpectedArray),
    }
}

#[cfg(feature = "alloc")]
fn next(
    fields: &mut impl Iterator<Item = MessagePackValue>,
) -> Result<MessagePackValue, RnxCodecError> {
    fields.next().ok_or(RnxCodecError::WrongFieldCount)
}

#[cfg(feature = "alloc")]
fn optional_float(value: Option<f64>) -> MessagePackValue {
    value.map_or(MessagePackValue::Nil, MessagePackValue::Float)
}

#[cfg(feature = "alloc")]
fn optional_unsigned(value: Option<u64>) -> MessagePackValue {
    value.map_or(MessagePackValue::Nil, MessagePackValue::Unsigned)
}

#[cfg(feature = "alloc")]
fn decode_optional_number(
    value: MessagePackValue,
    field: RnxField,
) -> Result<Option<f64>, RnxCodecError> {
    match value {
        MessagePackValue::Nil => Ok(None),
        value => decode_number(value, field).map(Some),
    }
}

#[cfg(feature = "alloc")]
fn decode_number(value: MessagePackValue, field: RnxField) -> Result<f64, RnxCodecError> {
    let number = match value {
        MessagePackValue::Float(value) => value,
        MessagePackValue::Unsigned(value) => value as f64,
        MessagePackValue::Signed(value) => value as f64,
        _ => return Err(RnxCodecError::InvalidField(field)),
    };
    if number.is_finite() {
        Ok(number)
    } else {
        Err(RnxCodecError::InvalidField(field))
    }
}

#[cfg(feature = "alloc")]
fn decode_optional_unsigned(
    value: MessagePackValue,
    field: RnxField,
) -> Result<Option<u64>, RnxCodecError> {
    match value {
        MessagePackValue::Nil => Ok(None),
        value => decode_unsigned(value, field).map(Some),
    }
}

#[cfg(feature = "alloc")]
fn decode_unsigned(value: MessagePackValue, field: RnxField) -> Result<u64, RnxCodecError> {
    match value {
        MessagePackValue::Unsigned(value) => Ok(value),
        MessagePackValue::Signed(value) => {
            u64::try_from(value).map_err(|_| RnxCodecError::InvalidField(field))
        }
        _ => Err(RnxCodecError::InvalidField(field)),
    }
}

#[cfg(feature = "alloc")]
fn decode_optional_i32(
    value: MessagePackValue,
    field: RnxField,
) -> Result<Option<i32>, RnxCodecError> {
    match value {
        MessagePackValue::Nil => Ok(None),
        MessagePackValue::Signed(value) => i32::try_from(value)
            .map(Some)
            .map_err(|_| RnxCodecError::InvalidField(field)),
        MessagePackValue::Unsigned(value) => i32::try_from(value)
            .map(Some)
            .map_err(|_| RnxCodecError::InvalidField(field)),
        _ => Err(RnxCodecError::InvalidField(field)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rnx::ExecutedCommandRef;

    #[test]
    fn request_round_trip_preserves_stock_fields() {
        let request = ExecutionRequest {
            command: String::from("printf hello"),
            timeout_seconds: Some(15.0),
            stdout_limit: Some(5),
            stderr_limit: None,
            stdin: Some(b"input".to_vec()),
        };
        let encoded = encode_execution_request(&request).unwrap();
        assert_eq!(decode_execution_request(&encoded), Ok(request));
        assert_eq!(
            decode_execution_request_ref(&encoded),
            Ok(ExecutionRequestRef {
                command: "printf hello",
                timeout_seconds: Some(15.0),
                stdout_limit: Some(5),
                stderr_limit: None,
                stdin: Some(b"input"),
            })
        );
    }

    #[test]
    fn bounded_result_encoding_matches_owned_encoding_and_names_capacity_failure() {
        let result = ExecutionResultRef::Executed(ExecutedCommandRef {
            return_code: Some(7),
            stdout: b"out",
            stderr: b"err",
            total_stdout: 8,
            total_stderr: 3,
            started_at: 2.0,
            conclusion: ExecutionConclusion::CompletedAt(3.0),
        });
        let owned = ExecutionResult::Executed(ExecutedCommand {
            return_code: Some(7),
            stdout: b"out".to_vec(),
            stderr: b"err".to_vec(),
            total_stdout: 8,
            total_stderr: 3,
            started_at: 2.0,
            conclusion: ExecutionConclusion::CompletedAt(3.0),
        });
        let expected = encode_execution_result(&owned).unwrap();
        let mut output = [0u8; 64];
        let written = encode_execution_result_to(result, &mut output).unwrap();
        assert_eq!(&output[..written], expected);
        assert_eq!(
            encode_execution_result_to(result, &mut output[..written - 1]),
            Err(RnxCodecError::BufferTooShort)
        );
    }

    #[test]
    fn result_round_trip_distinguishes_completion_timeout_and_spawn_failure() {
        for result in [
            ExecutionResult::NotExecuted { started_at: 1.0 },
            ExecutionResult::Executed(ExecutedCommand {
                return_code: Some(7),
                stdout: b"out".to_vec(),
                stderr: b"err".to_vec(),
                total_stdout: 8,
                total_stderr: 3,
                started_at: 2.0,
                conclusion: ExecutionConclusion::CompletedAt(3.0),
            }),
            ExecutionResult::Executed(ExecutedCommand {
                return_code: None,
                stdout: Vec::new(),
                stderr: Vec::new(),
                total_stdout: 0,
                total_stderr: 0,
                started_at: 4.0,
                conclusion: ExecutionConclusion::TimedOut,
            }),
        ] {
            let encoded = encode_execution_result(&result).unwrap();
            assert_eq!(decode_execution_result(&encoded), Ok(result));
        }
    }

    #[test]
    fn incoherent_results_are_rejected() {
        let result = ExecutionResult::Executed(ExecutedCommand {
            return_code: Some(0),
            stdout: b"too long".to_vec(),
            stderr: Vec::new(),
            total_stdout: 2,
            total_stderr: 0,
            started_at: 2.0,
            conclusion: ExecutionConclusion::CompletedAt(1.0),
        });
        assert_eq!(
            encode_execution_result(&result),
            Err(RnxCodecError::IncoherentResult)
        );
    }
}
