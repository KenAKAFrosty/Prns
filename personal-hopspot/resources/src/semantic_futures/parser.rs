use std::str;

use personal_hopspot_assurance_kernel::FUTURE_SIZE_SCENARIOS;
use thiserror::Error;

use super::NamedFutureSize;

const TYPE_PREFIX: &str = "print-type-size type: `";
const BYTES_SUFFIX: &str = " bytes";

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum EvidenceError {
    #[error("future-size compiler output is not UTF-8")]
    Encoding,
    #[error("future-size evidence for scenario {scenario:?} is missing")]
    Missing { scenario: String },
    #[error("future-size evidence for scenario {scenario:?} appears {occurrences} times")]
    Duplicate {
        scenario: String,
        occurrences: usize,
    },
    #[error("future-size evidence for scenario {scenario:?} has malformed byte count {value:?}")]
    InvalidBytes { scenario: String, value: String },
}

pub(super) fn parse(stdout: &[u8], stderr: &[u8]) -> Result<Vec<NamedFutureSize>, EvidenceError> {
    let stdout = str::from_utf8(stdout).map_err(|_| EvidenceError::Encoding)?;
    let stderr = str::from_utf8(stderr).map_err(|_| EvidenceError::Encoding)?;
    let lines = stdout.lines().chain(stderr.lines()).collect::<Vec<_>>();
    FUTURE_SIZE_SCENARIOS
        .iter()
        .map(|scenario| parse_scenario(&lines, scenario))
        .collect()
}

fn parse_scenario(
    lines: &[&str],
    scenario: &personal_hopspot_assurance_kernel::FutureSizeScenario,
) -> Result<NamedFutureSize, EvidenceError> {
    let matches = lines
        .iter()
        .filter_map(|line| type_size(line))
        .filter(|(type_name, _)| *type_name == scenario.rustc_type())
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Err(EvidenceError::Missing {
            scenario: scenario.identifier().to_string(),
        }),
        [(_, bytes)] => Ok(NamedFutureSize {
            scenario: scenario.identifier().to_string(),
            bytes: parse_bytes(scenario.identifier(), bytes)?,
        }),
        matches => Err(EvidenceError::Duplicate {
            scenario: scenario.identifier().to_string(),
            occurrences: matches.len(),
        }),
    }
}

fn type_size(line: &str) -> Option<(&str, &str)> {
    let remainder = line.strip_prefix(TYPE_PREFIX)?;
    let (type_name, size) = remainder.split_once("`: ")?;
    let (bytes, _) = size.split_once(", alignment: ")?;
    Some((type_name, bytes))
}

fn parse_bytes(scenario: &str, value: &str) -> Result<u64, EvidenceError> {
    let bytes = value
        .strip_suffix(BYTES_SUFFIX)
        .ok_or_else(|| EvidenceError::InvalidBytes {
            scenario: scenario.to_string(),
            value: value.to_string(),
        })?;
    bytes.parse().map_err(|_| EvidenceError::InvalidBytes {
        scenario: scenario.to_string(),
        value: value.to_string(),
    })
}
