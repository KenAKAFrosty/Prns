use super::{RnxCodecError, RnxField};
use crate::rnx::{
    ExecutionConclusion, ExecutionRequestRef, ExecutionResultRef, MAX_COMMAND_BYTES,
    MAX_RETURNED_STREAM_BYTES, MAX_STDIN_BYTES,
};

pub(super) fn validate_timestamp(timestamp: f64, field: RnxField) -> Result<(), RnxCodecError> {
    if timestamp.is_finite() && timestamp >= 0.0 {
        Ok(())
    } else {
        Err(RnxCodecError::InvalidField(field))
    }
}

pub(super) fn validate_request_ref(request: ExecutionRequestRef<'_>) -> Result<(), RnxCodecError> {
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

pub(super) fn validate_result_ref(result: ExecutionResultRef<'_>) -> Result<(), RnxCodecError> {
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
