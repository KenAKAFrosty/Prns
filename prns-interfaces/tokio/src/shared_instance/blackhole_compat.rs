use std::collections::HashMap;
use std::fs;
use std::io::{Cursor, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::string::String;
use std::vec::Vec;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use prns_core::identity::IdentityHash;
use prns_core::routing::{BlackholeExpiry, BlackholedIdentity};
use prns_core::units::InstantMillis;
use rmpv::Value;

const VALUE_MAX_DEPTH: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RnsBlackholeDecodeError {
    MessagePack,
    TrailingData,
    ExpectedMap,
    ExpectedIdentityHash,
    ExpectedEntryMap,
    InvalidUntil,
    InvalidReason,
}

impl core::fmt::Display for RnsBlackholeDecodeError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(match self {
            Self::MessagePack => "invalid MessagePack",
            Self::TrailingData => "trailing data",
            Self::ExpectedMap => "expected an identity map",
            Self::ExpectedIdentityHash => "expected a binary identity hash",
            Self::ExpectedEntryMap => "expected a blackhole entry map",
            Self::InvalidUntil => "invalid until value",
            Self::InvalidReason => "invalid reason value",
        })
    }
}

impl std::error::Error for RnsBlackholeDecodeError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RnsBlackholeEncodeError;

impl core::fmt::Display for RnsBlackholeEncodeError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("could not encode MessagePack")
    }
}

impl std::error::Error for RnsBlackholeEncodeError {}

#[derive(Debug)]
pub enum RnsBlackholeFileError {
    Io(std::io::Error),
    Decode(RnsBlackholeDecodeError),
    Encode(RnsBlackholeEncodeError),
}

impl core::fmt::Display for RnsBlackholeFileError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Decode(error) => write!(formatter, "invalid RNS blackhole file: {error}"),
            Self::Encode(error) => {
                write!(formatter, "could not encode RNS blackhole file: {error}")
            }
        }
    }
}

impl std::error::Error for RnsBlackholeFileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Decode(error) => Some(error),
            Self::Encode(error) => Some(error),
        }
    }
}

impl From<std::io::Error> for RnsBlackholeFileError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<RnsBlackholeDecodeError> for RnsBlackholeFileError {
    fn from(error: RnsBlackholeDecodeError) -> Self {
        Self::Decode(error)
    }
}

#[derive(Debug, Clone)]
pub struct RnsLocalBlackholeFile {
    blackhole_dir: PathBuf,
}

impl RnsLocalBlackholeFile {
    pub fn new(blackhole_dir: impl Into<PathBuf>) -> Self {
        Self {
            blackhole_dir: blackhole_dir.into(),
        }
    }

    pub fn path(&self) -> PathBuf {
        self.blackhole_dir.join("local")
    }

    pub fn load(
        &self,
        local_source: IdentityHash,
        now: InstantMillis,
    ) -> Result<Vec<BlackholedIdentity<String>>, RnsBlackholeFileError> {
        let bytes = match fs::read(self.path()) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        decode_source(&bytes, local_source, now).map_err(Into::into)
    }

    pub fn store<'a>(
        &self,
        local_source: IdentityHash,
        entries: impl IntoIterator<Item = BlackholedIdentity<&'a str>>,
    ) -> Result<(), RnsBlackholeFileError> {
        let bytes = encode_local(local_source, entries).map_err(RnsBlackholeFileError::Encode)?;
        self.ensure_dir()?;
        let final_path = self.path();
        let staging_path = self.blackhole_dir.join("local.tmp");
        let result = stage(&staging_path, &bytes).and_then(|()| {
            replace_file(&staging_path, &final_path).map_err(RnsBlackholeFileError::from)
        });
        if result.is_err() {
            let _ = fs::remove_file(staging_path);
        }
        result
    }

    fn ensure_dir(&self) -> Result<(), RnsBlackholeFileError> {
        if !self.blackhole_dir.exists() {
            fs::create_dir_all(&self.blackhole_dir)?;
            #[cfg(unix)]
            let _ = fs::set_permissions(&self.blackhole_dir, fs::Permissions::from_mode(0o700));
        }
        Ok(())
    }
}

pub fn decode_source(
    bytes: &[u8],
    source: IdentityHash,
    now: InstantMillis,
) -> Result<Vec<BlackholedIdentity<String>>, RnsBlackholeDecodeError> {
    let mut cursor = Cursor::new(bytes);
    let value = rmpv::decode::read_value_with_max_depth(&mut cursor, VALUE_MAX_DEPTH)
        .map_err(|_| RnsBlackholeDecodeError::MessagePack)?;
    if cursor.position() != bytes.len() as u64 {
        return Err(RnsBlackholeDecodeError::TrailingData);
    }
    let Value::Map(map) = value else {
        return Err(RnsBlackholeDecodeError::ExpectedMap);
    };

    let mut rows = Vec::with_capacity(map.len());
    let mut positions = HashMap::with_capacity(map.len());
    for (identity, entry) in &map {
        let Some(bytes) = identity_bytes(identity)? else {
            continue;
        };
        match positions.get(&bytes).copied() {
            Some(index) => rows[index] = (bytes, entry),
            None => {
                positions.insert(bytes, rows.len());
                rows.push((bytes, entry));
            }
        }
    }

    let mut decoded = Vec::with_capacity(rows.len());
    for (identity, entry) in rows {
        let Value::Map(fields) = entry else {
            return Err(RnsBlackholeDecodeError::ExpectedEntryMap);
        };
        let Some(expiry) = decode_persisted_expiry(field(fields, "until"), now)? else {
            continue;
        };
        let reason = decode_reason(field(fields, "reason"))?;
        decoded.push(BlackholedIdentity {
            identity: IdentityHash::new(identity),
            source,
            expiry,
            reason,
        });
    }
    Ok(decoded)
}

pub fn encode_local<'a>(
    local_source: IdentityHash,
    entries: impl IntoIterator<Item = BlackholedIdentity<&'a str>>,
) -> Result<Vec<u8>, RnsBlackholeEncodeError> {
    let mut map = Vec::new();
    for entry in entries {
        if entry.source != local_source {
            continue;
        }
        let expiry = match entry.expiry {
            BlackholeExpiry::Indefinite => Value::Nil,
            BlackholeExpiry::At(at) => Value::F64(at.0 as f64 / 1_000.0),
        };
        let reason = entry.reason.map_or(Value::Nil, Value::from);
        map.push((
            Value::Binary(entry.identity.as_bytes().to_vec()),
            Value::Map(vec![
                (
                    Value::from("source"),
                    Value::Binary(local_source.as_bytes().to_vec()),
                ),
                (Value::from("until"), expiry),
                (Value::from("reason"), reason),
            ]),
        ));
    }
    let mut bytes = Vec::new();
    rmpv::encode::write_value(&mut bytes, &Value::Map(map)).map_err(|_| RnsBlackholeEncodeError)?;
    Ok(bytes)
}

fn identity_bytes(value: &Value) -> Result<Option<[u8; 16]>, RnsBlackholeDecodeError> {
    let Value::Binary(bytes) = value else {
        return Err(RnsBlackholeDecodeError::ExpectedIdentityHash);
    };
    Ok(bytes.as_slice().try_into().ok())
}

fn field<'a>(fields: &'a [(Value, Value)], name: &str) -> Option<&'a Value> {
    fields
        .iter()
        .rev()
        .find_map(|(key, value)| (key.as_str() == Some(name)).then_some(value))
}

fn decode_persisted_expiry(
    value: Option<&Value>,
    now: InstantMillis,
) -> Result<Option<BlackholeExpiry>, RnsBlackholeDecodeError> {
    let Some(value) = value else {
        return Ok(Some(BlackholeExpiry::Indefinite));
    };
    match value {
        Value::Nil => Ok(Some(BlackholeExpiry::Indefinite)),
        Value::Integer(integer) => {
            let Some(seconds) = integer.as_u64() else {
                if integer.as_i64().is_some() {
                    return Ok(None);
                }
                return Err(RnsBlackholeDecodeError::InvalidUntil);
            };
            let deadline = seconds.saturating_mul(1_000);
            Ok((now.0 < deadline).then_some(BlackholeExpiry::At(InstantMillis(deadline))))
        }
        Value::F32(_) | Value::F64(_) => {
            let Some(seconds) = value.as_f64() else {
                return Err(RnsBlackholeDecodeError::InvalidUntil);
            };
            if seconds == f64::INFINITY {
                return Ok(Some(BlackholeExpiry::At(InstantMillis(u64::MAX))));
            }
            let millis = seconds * 1_000.0;
            if !millis.is_finite() || millis <= now.0 as f64 {
                return Ok(None);
            }
            let deadline = if millis >= u64::MAX as f64 {
                u64::MAX
            } else {
                millis.floor() as u64
            };
            Ok(Some(BlackholeExpiry::At(InstantMillis(deadline))))
        }
        _ => Err(RnsBlackholeDecodeError::InvalidUntil),
    }
}

fn decode_reason(value: Option<&Value>) -> Result<Option<String>, RnsBlackholeDecodeError> {
    match value {
        None | Some(Value::Nil) => Ok(None),
        Some(Value::String(reason)) => reason
            .as_str()
            .map(|reason| Some(reason.to_owned()))
            .ok_or(RnsBlackholeDecodeError::InvalidReason),
        Some(_) => Err(RnsBlackholeDecodeError::InvalidReason),
    }
}

fn stage(path: &Path, bytes: &[u8]) -> Result<(), RnsBlackholeFileError> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(unix)]
fn replace_file(staging: &Path, final_path: &Path) -> std::io::Result<()> {
    fs::rename(staging, final_path)
}

#[cfg(not(unix))]
fn replace_file(staging: &Path, final_path: &Path) -> std::io::Result<()> {
    match fs::remove_file(final_path) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fs::rename(staging, final_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RNS_138_FIXTURE: &[u8] = b"\x82\xc4\x10\x11\x11\x11\x11\x11\x11\x11\x11\x11\x11\x11\x11\x11\x11\x11\x11\x83\xa6source\xc4\x10\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xa5until\xcb\x41\xd9\x54\xfc\x40\x08\x00\x00\xa6reason\xa8operator\xc4\x10\x22\x22\x22\x22\x22\x22\x22\x22\x22\x22\x22\x22\x22\x22\x22\x22\x83\xa6source\xc4\x10\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xa5until\xc0\xa6reason\xc0";

    fn source() -> IdentityHash {
        IdentityHash::new([0xaa; 16])
    }

    fn fixture_entries() -> Vec<BlackholedIdentity<&'static str>> {
        vec![
            BlackholedIdentity {
                identity: IdentityHash::new([0x11; 16]),
                source: source(),
                expiry: BlackholeExpiry::At(InstantMillis(1_700_000_000_125)),
                reason: Some("operator"),
            },
            BlackholedIdentity {
                identity: IdentityHash::new([0x22; 16]),
                source: source(),
                expiry: BlackholeExpiry::Indefinite,
                reason: None,
            },
        ]
    }

    #[test]
    fn decodes_the_rns_138_umsgpack_file_and_applies_reload_expiry() {
        let decoded = decode_source(RNS_138_FIXTURE, source(), InstantMillis(1_700_000_000_124));
        assert_eq!(
            decoded,
            Ok(fixture_entries()
                .into_iter()
                .map(|entry| BlackholedIdentity {
                    identity: entry.identity,
                    source: entry.source,
                    expiry: entry.expiry,
                    reason: entry.reason.map(String::from),
                })
                .collect())
        );

        let at_equality =
            decode_source(RNS_138_FIXTURE, source(), InstantMillis(1_700_000_000_125));
        assert_eq!(
            at_equality,
            Ok(vec![BlackholedIdentity {
                identity: IdentityHash::new([0x22; 16]),
                source: source(),
                expiry: BlackholeExpiry::Indefinite,
                reason: None,
            }])
        );
    }

    #[test]
    fn encodes_exactly_what_rns_138_umsgpack_emits() {
        assert!(
            encode_local(source(), fixture_entries()).is_ok_and(|bytes| bytes == RNS_138_FIXTURE)
        );
    }

    #[test]
    fn local_encoding_excludes_entries_owned_by_a_remote_source() {
        let mut entries = fixture_entries();
        entries.push(BlackholedIdentity {
            identity: IdentityHash::new([0x33; 16]),
            source: IdentityHash::new([0xbb; 16]),
            expiry: BlackholeExpiry::Indefinite,
            reason: None,
        });
        assert!(encode_local(source(), entries).is_ok_and(|bytes| bytes == RNS_138_FIXTURE));
    }

    #[test]
    fn file_store_replaces_through_local_tmp_and_missing_load_is_empty() {
        let dir = std::env::temp_dir().join(format!(
            "prns-rns-blackhole-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let file = RnsLocalBlackholeFile::new(&dir);
        assert!(file
            .load(source(), InstantMillis(0))
            .is_ok_and(|rows| rows.is_empty()));

        assert!(file.store(source(), fixture_entries()).is_ok());
        assert!(fs::read(file.path()).is_ok_and(|bytes| bytes == RNS_138_FIXTURE));
        assert!(!dir.join("local.tmp").exists());

        assert!(file.store(source(), Vec::new()).is_ok());
        assert!(fs::read(file.path()).is_ok_and(|bytes| bytes == [0x80]));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn malformed_shapes_fail_as_typed_decode_errors() {
        assert_eq!(
            decode_source(&[0x90], source(), InstantMillis(0)),
            Err(RnsBlackholeDecodeError::ExpectedMap)
        );
        assert_eq!(
            decode_source(&[0x80, 0x00], source(), InstantMillis(0)),
            Err(RnsBlackholeDecodeError::TrailingData)
        );
    }

    #[test]
    fn persisted_numeric_forms_follow_rns_reload_semantics() {
        let integer = source_map(Value::from(2));
        assert_eq!(
            decode_source(&integer, source(), InstantMillis(1_999)),
            Ok(vec![BlackholedIdentity {
                identity: IdentityHash::new([0x44; 16]),
                source: source(),
                expiry: BlackholeExpiry::At(InstantMillis(2_000)),
                reason: None,
            }])
        );
        assert!(decode_source(&integer, source(), InstantMillis(2_000))
            .is_ok_and(|entries| entries.is_empty()));

        let fractional = source_map(Value::F64(2.0005));
        assert_eq!(
            decode_source(&fractional, source(), InstantMillis(2_000)),
            Ok(vec![BlackholedIdentity {
                identity: IdentityHash::new([0x44; 16]),
                source: source(),
                expiry: BlackholeExpiry::At(InstantMillis(2_000)),
                reason: None,
            }])
        );

        for expired in [Value::from(-1), Value::from(0), Value::F64(f64::NAN)] {
            assert!(
                decode_source(&source_map(expired), source(), InstantMillis(0))
                    .is_ok_and(|entries| entries.is_empty())
            );
        }
    }

    #[test]
    fn source_identity_comes_from_the_source_file_and_short_hashes_are_skipped() {
        let replacement_source = IdentityHash::new([0xcc; 16]);
        let decoded = decode_source(RNS_138_FIXTURE, replacement_source, InstantMillis(0));
        assert!(decoded.is_ok_and(|entries| entries
            .iter()
            .all(|entry| entry.source == replacement_source)));

        let value = Value::Map(vec![
            (
                Value::Binary(vec![0x55; 15]),
                Value::Map(vec![(Value::from("until"), Value::Nil)]),
            ),
            (
                Value::Binary(vec![0x44; 16]),
                Value::Map(vec![(Value::from("until"), Value::Nil)]),
            ),
        ]);
        assert!(
            decode_source(&encode_value(value), source(), InstantMillis(0))
                .is_ok_and(|entries| entries.len() == 1)
        );
    }

    fn source_map(until: Value) -> Vec<u8> {
        encode_value(Value::Map(vec![(
            Value::Binary(vec![0x44; 16]),
            Value::Map(vec![(Value::from("until"), until)]),
        )]))
    }

    fn encode_value(value: Value) -> Vec<u8> {
        let mut bytes = Vec::new();
        assert!(rmpv::encode::write_value(&mut bytes, &value).is_ok());
        bytes
    }
}
