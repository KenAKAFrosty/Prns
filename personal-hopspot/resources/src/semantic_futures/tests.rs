use super::parser::{parse, EvidenceError};
use super::NamedFutureSize;

const SX126X: &str =
    "print-type-size type: `{async fn body of sx126x::run()}`: 828 bytes, alignment: 4 bytes";
const LR1110: &str =
    "print-type-size type: `{async fn body of lr1110::run()}`: 928 bytes, alignment: 4 bytes";

#[test]
fn rustc_future_sizes_are_selected_in_scenario_order() -> Result<(), EvidenceError> {
    let stderr = format!("noise\n{LR1110}\n{SX126X}\nmore noise\n");
    assert_eq!(
        parse(b"cargo output\n", stderr.as_bytes())?,
        [
            NamedFutureSize {
                scenario: "sx126x".to_string(),
                bytes: 828,
            },
            NamedFutureSize {
                scenario: "lr1110".to_string(),
                bytes: 928,
            },
        ]
    );
    Ok(())
}

#[test]
fn missing_future_size_is_rejected() {
    assert_eq!(
        parse(b"", SX126X.as_bytes()),
        Err(EvidenceError::Missing {
            scenario: "lr1110".to_string(),
        })
    );
}

#[test]
fn duplicate_future_size_is_rejected() {
    let stderr = format!("{SX126X}\n{SX126X}\n{LR1110}\n");
    assert_eq!(
        parse(b"", stderr.as_bytes()),
        Err(EvidenceError::Duplicate {
            scenario: "sx126x".to_string(),
            occurrences: 2,
        })
    );
}

#[test]
fn malformed_future_size_is_rejected() {
    let malformed = SX126X.replace("828 bytes", "many bytes");
    let stderr = format!("{malformed}\n{LR1110}\n");
    assert_eq!(
        parse(b"", stderr.as_bytes()),
        Err(EvidenceError::InvalidBytes {
            scenario: "sx126x".to_string(),
            value: "many bytes".to_string(),
        })
    );
}

#[test]
fn non_utf8_future_size_evidence_is_rejected() {
    assert_eq!(parse(&[0xff], b""), Err(EvidenceError::Encoding));
}
