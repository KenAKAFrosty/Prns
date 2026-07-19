use alloc::string::String;
use alloc::vec::Vec;

use super::super::*;
use crate::rnx::{
    ExecutedCommand, ExecutedCommandRef, ExecutionConclusion, ExecutionRequest,
    ExecutionRequestRef, ExecutionResult, ExecutionResultRef,
};

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
