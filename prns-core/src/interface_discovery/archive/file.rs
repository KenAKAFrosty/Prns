use std::fs;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use crate::interface_discovery::{
    AdvertisedInterfaceType, DiscoveryCatalogRestoreError, StampValueError,
};

use super::DISCOVERED_INTERFACES_FILE;

pub(super) fn replace_archive_file(path: &Path, bytes: &[u8]) -> Result<(), DiscoveryArchiveError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| DiscoveryArchiveError::Io {
        operation: ArchiveFileOperation::CreateDirectory,
        path: parent.to_path_buf(),
        source,
    })?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(DISCOVERED_INTERFACES_FILE);
    let staging = parent.join(format!(".{file_name}.{}.staging", std::process::id()));
    if let Err(source) = stage(&staging, bytes) {
        let _ = fs::remove_file(&staging);
        return Err(DiscoveryArchiveError::Io {
            operation: ArchiveFileOperation::Stage,
            path: staging,
            source,
        });
    }
    match replace_file(&staging, path) {
        Ok(()) => Ok(()),
        Err(source) => {
            let _ = fs::remove_file(&staging);
            Err(DiscoveryArchiveError::Io {
                operation: ArchiveFileOperation::Replace,
                path: path.to_path_buf(),
                source,
            })
        }
    }
}

fn stage(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn replace_file(staging: &Path, final_path: &Path) -> std::io::Result<()> {
    fs::rename(staging, final_path)
}

pub(super) fn encode_hex(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(ALPHABET[usize::from(byte >> 4)]));
        encoded.push(char::from(ALPHABET[usize::from(byte & 0x0f)]));
    }
    encoded
}

pub(super) fn decode_hex<const N: usize>(encoded: &str) -> Result<[u8; N], HexDecodeError> {
    let expected = N * 2;
    if encoded.len() != expected {
        return Err(HexDecodeError::Length {
            expected,
            actual: encoded.len(),
        });
    }
    let mut decoded = [0u8; N];
    for (index, pair) in encoded.as_bytes().chunks_exact(2).enumerate() {
        let high = decode_nibble(pair[0], index * 2)?;
        let low = decode_nibble(pair[1], index * 2 + 1)?;
        decoded[index] = (high << 4) | low;
    }
    Ok(decoded)
}

fn decode_nibble(byte: u8, index: usize) -> Result<u8, HexDecodeError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(HexDecodeError::Digit { index, byte }),
    }
}

#[derive(Debug)]
pub enum DiscoveryArchiveError {
    Io {
        operation: ArchiveFileOperation,
        path: PathBuf,
        source: std::io::Error,
    },
    Decode {
        path: PathBuf,
        source: serde_json::Error,
    },
    Encode {
        path: PathBuf,
        source: serde_json::Error,
    },
    TooLarge {
        path: PathBuf,
        bytes: usize,
        maximum: usize,
    },
    UnsupportedFormat {
        path: PathBuf,
        found: u32,
        supported: u32,
    },
    InvalidRecord {
        path: PathBuf,
        id: String,
        source: ArchiveRecordError,
    },
}

impl DiscoveryArchiveError {
    pub(super) fn read(path: PathBuf, source: std::io::Error) -> Self {
        Self::Io {
            operation: ArchiveFileOperation::Read,
            path,
            source,
        }
    }
}

impl core::fmt::Display for DiscoveryArchiveError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io {
                operation,
                path,
                source,
            } => write!(
                formatter,
                "could not {operation} discovery archive {}: {source}",
                path.display()
            ),
            Self::Decode { path, source } => write!(
                formatter,
                "could not decode discovery archive {}: {source}",
                path.display()
            ),
            Self::Encode { path, source } => write!(
                formatter,
                "could not encode discovery archive {}: {source}",
                path.display()
            ),
            Self::TooLarge {
                path,
                bytes,
                maximum,
            } => write!(
                formatter,
                "discovery archive {} is {bytes} bytes, exceeding the {maximum}-byte limit",
                path.display()
            ),
            Self::UnsupportedFormat {
                path,
                found,
                supported,
            } => write!(
                formatter,
                "discovery archive {} uses format {found}, but this build supports format {supported}",
                path.display()
            ),
            Self::InvalidRecord { path, id, source } => write!(
                formatter,
                "discovery archive {} has invalid record {id}: {source}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for DiscoveryArchiveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Decode { source, .. } | Self::Encode { source, .. } => Some(source),
            Self::InvalidRecord { source, .. } => Some(source),
            Self::TooLarge { .. } | Self::UnsupportedFormat { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFileOperation {
    CreateDirectory,
    Read,
    Stage,
    Replace,
}

impl core::fmt::Display for ArchiveFileOperation {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::CreateDirectory => formatter.write_str("create the directory for"),
            Self::Read => formatter.write_str("read"),
            Self::Stage => formatter.write_str("stage"),
            Self::Replace => formatter.write_str("replace"),
        }
    }
}

#[derive(Debug)]
pub enum ArchiveRecordError {
    EmptyName,
    ZeroObservationCount,
    InvalidHex {
        field: &'static str,
        source: HexDecodeError,
    },
    UnsupportedInterfaceType {
        value: String,
    },
    MismatchedDetails {
        interface_type: AdvertisedInterfaceType,
    },
    InvalidFloat {
        field: &'static str,
        value: String,
    },
    InvalidReachableAddress {
        value: String,
    },
    StampValue(StampValueError),
    Catalog(DiscoveryCatalogRestoreError),
}

impl core::fmt::Display for ArchiveRecordError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::EmptyName => formatter.write_str("interface name is empty"),
            Self::ZeroObservationCount => formatter.write_str("observation count is zero"),
            Self::InvalidHex { field, source } => write!(formatter, "invalid {field}: {source}"),
            Self::UnsupportedInterfaceType { value } => {
                write!(formatter, "unsupported interface type {value}")
            }
            Self::MismatchedDetails { interface_type } => write!(
                formatter,
                "advertisement details do not match {}",
                interface_type.rns_name()
            ),
            Self::InvalidFloat { field, value } => {
                write!(formatter, "invalid {field} value {value}")
            }
            Self::InvalidReachableAddress { value } => {
                write!(formatter, "invalid reachable address {value}")
            }
            Self::StampValue(source) => source.fmt(formatter),
            Self::Catalog(source) => source.fmt(formatter),
        }
    }
}

impl std::error::Error for ArchiveRecordError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidHex { source, .. } => Some(source),
            Self::StampValue(source) => Some(source),
            Self::Catalog(source) => Some(source),
            Self::EmptyName
            | Self::ZeroObservationCount
            | Self::UnsupportedInterfaceType { .. }
            | Self::MismatchedDetails { .. }
            | Self::InvalidFloat { .. }
            | Self::InvalidReachableAddress { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HexDecodeError {
    Length { expected: usize, actual: usize },
    Digit { index: usize, byte: u8 },
}

impl core::fmt::Display for HexDecodeError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Digit { index, byte } => write!(
                formatter,
                "byte {:?} at hexadecimal position {index} is not a hexadecimal digit",
                char::from(*byte)
            ),
            Self::Length { expected, actual } => write!(
                formatter,
                "expected {expected} hexadecimal characters, found {actual}"
            ),
        }
    }
}

impl std::error::Error for HexDecodeError {}
