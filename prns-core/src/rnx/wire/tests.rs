use super::*;
use crate::rnx::{
    ExecutedCommandRef, ExecutionConclusion, ExecutionRequestRef, ExecutionResultRef,
};

#[test]
fn borrowed_request_decoding_preserves_stock_fields() {
    let encoded = [
        0x95, 0xc4, 0x0c, b'p', b'r', b'i', b'n', b't', b'f', b' ', b'h', b'e', b'l', b'l', b'o',
        0xcb, 0x40, 0x2e, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0xc0, 0xc4, 0x05, b'i', b'n',
        b'p', b'u', b't',
    ];
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
fn bounded_result_encoding_preserves_stock_wire_and_names_capacity_failure() {
    let result = ExecutionResultRef::Executed(ExecutedCommandRef {
        return_code: Some(7),
        stdout: b"out",
        stderr: b"err",
        total_stdout: 8,
        total_stderr: 3,
        started_at: 2.0,
        conclusion: ExecutionConclusion::CompletedAt(3.0),
    });
    let expected = [
        0x98, 0xc3, 0x07, 0xc4, 0x03, b'o', b'u', b't', 0xc4, 0x03, b'e', b'r', b'r', 0x08, 0x03,
        0xcb, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xcb, 0x40, 0x08, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00,
    ];
    let mut output = [0u8; 64];
    let written = encode_execution_result_to(result, &mut output).unwrap();
    assert_eq!(&output[..written], expected);
    assert_eq!(
        encode_execution_result_to(result, &mut output[..written - 1]),
        Err(RnxCodecError::BufferTooShort)
    );
}
