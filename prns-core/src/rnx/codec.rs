use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::message_pack::{
    decode_owned, encode_owned, MessagePackDecodeLimits, MessagePackOwnedError, MessagePackValue,
};

use super::{
    ExecutedCommand, ExecutionConclusion, ExecutionRequest, ExecutionResult, MAX_COMMAND_BYTES,
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
    MessagePack(MessagePackOwnedError),
    ExpectedArray,
    WrongFieldCount,
    InvalidField(RnxField),
    InvalidUtf8,
    IncoherentResult,
}

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

fn array(value: MessagePackValue, length: usize) -> Result<Vec<MessagePackValue>, RnxCodecError> {
    match value {
        MessagePackValue::Array(fields) if fields.len() == length => Ok(fields),
        MessagePackValue::Array(_) => Err(RnxCodecError::WrongFieldCount),
        _ => Err(RnxCodecError::ExpectedArray),
    }
}

fn next(
    fields: &mut impl Iterator<Item = MessagePackValue>,
) -> Result<MessagePackValue, RnxCodecError> {
    fields.next().ok_or(RnxCodecError::WrongFieldCount)
}

fn optional_float(value: Option<f64>) -> MessagePackValue {
    value.map_or(MessagePackValue::Nil, MessagePackValue::Float)
}

fn optional_unsigned(value: Option<u64>) -> MessagePackValue {
    value.map_or(MessagePackValue::Nil, MessagePackValue::Unsigned)
}

fn decode_optional_number(
    value: MessagePackValue,
    field: RnxField,
) -> Result<Option<f64>, RnxCodecError> {
    match value {
        MessagePackValue::Nil => Ok(None),
        value => decode_number(value, field).map(Some),
    }
}

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

fn decode_optional_unsigned(
    value: MessagePackValue,
    field: RnxField,
) -> Result<Option<u64>, RnxCodecError> {
    match value {
        MessagePackValue::Nil => Ok(None),
        value => decode_unsigned(value, field).map(Some),
    }
}

fn decode_unsigned(value: MessagePackValue, field: RnxField) -> Result<u64, RnxCodecError> {
    match value {
        MessagePackValue::Unsigned(value) => Ok(value),
        MessagePackValue::Signed(value) => {
            u64::try_from(value).map_err(|_| RnxCodecError::InvalidField(field))
        }
        _ => Err(RnxCodecError::InvalidField(field)),
    }
}

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
