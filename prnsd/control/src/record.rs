use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};

use crate::ServicePaths;

const RECORD_VERSION: &str = "prnsd-session-v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLane {
    Human,
    Json,
}

impl LogLane {
    pub fn path(self, paths: &ServicePaths) -> &Path {
        match self {
            Self::Human => &paths.human_log,
            Self::Json => &paths.json_log,
        }
    }

    pub(crate) fn previous_path(self, paths: &ServicePaths) -> &Path {
        match self {
            Self::Human => &paths.human_previous_log,
            Self::Json => &paths.json_previous_log,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Json => "json",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "human" => Some(Self::Human),
            "json" => Some(Self::Json),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceState {
    Starting,
    Running,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceRecord {
    pub generation: u128,
    pub pid: u32,
    pub signature: u64,
    pub log_lane: LogLane,
    pub binary: PathBuf,
    pub version: String,
    pub state: ServiceState,
}

impl ServiceRecord {
    pub fn log<'a>(&self, paths: &'a ServicePaths) -> &'a Path {
        self.log_lane.path(paths)
    }

    pub(crate) fn encode(&self) -> String {
        format!(
            "{RECORD_VERSION}\n{:032x}\n{}\n{}\n{}\n{}\n{}\n",
            self.generation,
            self.pid,
            self.signature,
            self.log_lane.as_str(),
            encode_os(self.binary.as_os_str()),
            encode_bytes(self.version.as_bytes()),
        )
    }

    pub(crate) fn decode(text: &str) -> Result<Self, RecordError> {
        let mut lines = text.lines();
        if lines.next() != Some(RECORD_VERSION) {
            return Err(RecordError);
        }
        let generation = lines
            .next()
            .and_then(|value| u128::from_str_radix(value, 16).ok())
            .ok_or(RecordError)?;
        let pid = lines
            .next()
            .and_then(|value| value.parse().ok())
            .ok_or(RecordError)?;
        let signature = lines
            .next()
            .and_then(|value| value.parse().ok())
            .ok_or(RecordError)?;
        let log_lane = lines.next().and_then(LogLane::parse).ok_or(RecordError)?;
        let binary = lines
            .next()
            .and_then(decode_os)
            .map(PathBuf::from)
            .ok_or(RecordError)?;
        let version = lines
            .next()
            .and_then(decode_bytes)
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .ok_or(RecordError)?;
        if lines.next().is_some() {
            return Err(RecordError);
        }
        Ok(Self {
            generation,
            pid,
            signature,
            log_lane,
            binary,
            version,
            state: ServiceState::Starting,
        })
    }
}

#[derive(Debug)]
pub(crate) struct RecordError;

impl fmt::Display for RecordError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid prnsd session record")
    }
}

fn encode_bytes(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn decode_bytes(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).ok()?;
            u8::from_str_radix(pair, 16).ok()
        })
        .collect()
}

#[cfg(unix)]
fn encode_os(value: &OsStr) -> String {
    use std::os::unix::ffi::OsStrExt;
    encode_bytes(value.as_bytes())
}

#[cfg(unix)]
fn decode_os(value: &str) -> Option<OsString> {
    use std::os::unix::ffi::OsStringExt;
    decode_bytes(value).map(OsString::from_vec)
}

#[cfg(windows)]
fn encode_os(value: &OsStr) -> String {
    use std::os::windows::ffi::OsStrExt;
    let bytes: Vec<_> = value.encode_wide().flat_map(u16::to_le_bytes).collect();
    encode_bytes(&bytes)
}

#[cfg(windows)]
fn decode_os(value: &str) -> Option<OsString> {
    use std::os::windows::ffi::OsStringExt;
    let bytes = decode_bytes(value)?;
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let wide: Option<Vec<_>> = bytes
        .chunks_exact(2)
        .map(|pair| Some(u16::from_le_bytes([pair[0], pair[1]])))
        .collect();
    wide.map(|wide| OsString::from_wide(&wide))
}

#[cfg(not(any(unix, windows)))]
fn encode_os(value: &OsStr) -> String {
    encode_bytes(value.to_string_lossy().as_bytes())
}

#[cfg(not(any(unix, windows)))]
fn decode_os(value: &str) -> Option<OsString> {
    decode_bytes(value)
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(OsString::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_round_trips() {
        let record = ServiceRecord {
            generation: 42,
            pid: 17,
            signature: 99,
            log_lane: LogLane::Json,
            binary: PathBuf::from("/tmp/prns d"),
            version: "0.2.3".to_string(),
            state: ServiceState::Running,
        };
        let mut decoded = ServiceRecord::decode(&record.encode()).unwrap();
        decoded.state = ServiceState::Running;
        assert_eq!(decoded, record);
    }

    #[test]
    fn malformed_records_are_rejected() {
        assert!(ServiceRecord::decode("not-a-session").is_err());
    }
}
