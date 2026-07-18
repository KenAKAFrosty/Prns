use alloc::string::String;
use alloc::vec::Vec;

use rmp::Marker;

use crate::identity::IdentityHash;
use crate::routing::BlackholeExpiry;
use crate::units::InstantMillis;

use super::super::message_pack::{MessagePackInteger, MessagePackReader};
use super::super::wire_names::{blackhole, common};
use super::RnsBlackholeEntry;

const MAXIMUM_DEPTH: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RnsBlackholeDecodeError {
    MessagePack,
    TrailingData,
    ExpectedMap,
    ExpectedIdentityHash,
    ExpectedEntryMap,
    InvalidSource,
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
            Self::InvalidSource => "invalid source identity",
            Self::InvalidUntil => "invalid until value",
            Self::InvalidReason => "invalid reason value",
        })
    }
}

#[cfg(feature = "std")]
impl std::error::Error for RnsBlackholeDecodeError {}

enum SourcePolicy {
    SourceFile(IdentityHash),
    Published,
}

enum ParsedEntry {
    ExpectedMap,
    Fields(EntryFields),
}

struct EntryFields {
    source: SourceField,
    until: UntilField,
    reason: ReasonField,
}

enum SourceField {
    Missing,
    Identity(IdentityHash),
    Invalid,
}

enum UntilField {
    Missing,
    Indefinite,
    Integer(MessagePackInteger),
    Float(f64),
    Invalid,
}

enum ReasonField {
    Missing,
    None,
    Text(String),
    Invalid,
}

pub(super) fn decode_source_file(
    bytes: &[u8],
    source: IdentityHash,
    now: InstantMillis,
) -> Result<Vec<RnsBlackholeEntry>, RnsBlackholeDecodeError> {
    decode(bytes, SourcePolicy::SourceFile(source), now)
}

pub(super) fn decode_published_table(
    bytes: &[u8],
    now: InstantMillis,
) -> Result<Vec<RnsBlackholeEntry>, RnsBlackholeDecodeError> {
    decode(bytes, SourcePolicy::Published, now)
}

fn decode(
    bytes: &[u8],
    source_policy: SourcePolicy,
    now: InstantMillis,
) -> Result<Vec<RnsBlackholeEntry>, RnsBlackholeDecodeError> {
    let mut reader = MessagePackReader::new(bytes);
    let root = reader.marker().map_err(message_pack)?;
    let Some(length) = reader.map_length(root).map_err(message_pack)? else {
        reader
            .skip_value(root, 0, MAXIMUM_DEPTH)
            .map_err(message_pack)?;
        return if reader.is_finished() {
            Err(RnsBlackholeDecodeError::ExpectedMap)
        } else {
            Err(RnsBlackholeDecodeError::TrailingData)
        };
    };

    let mut rows = Vec::new();
    let mut invalid_key = false;
    for _ in 0..length {
        let identity = decode_identity_key(&mut reader)?;
        match identity {
            IdentityKey::Exact(identity) => {
                let entry = decode_entry(&mut reader, &source_policy)?;
                if let Some(position) = rows.iter().position(|(stored, _)| stored == &identity) {
                    rows[position] = (identity, entry);
                } else {
                    rows.push((identity, entry));
                }
            }
            IdentityKey::WrongLength => skip_next(&mut reader, 1)?,
            IdentityKey::InvalidType => {
                invalid_key = true;
                skip_next(&mut reader, 1)?;
            }
        }
    }
    if !reader.is_finished() {
        return Err(RnsBlackholeDecodeError::TrailingData);
    }
    if invalid_key {
        return Err(RnsBlackholeDecodeError::ExpectedIdentityHash);
    }

    rows.into_iter()
        .filter_map(
            |(identity, entry)| match finish_entry(identity, entry, &source_policy, now) {
                Ok(Some(entry)) => Some(Ok(entry)),
                Ok(None) => None,
                Err(error) => Some(Err(error)),
            },
        )
        .collect()
}

enum IdentityKey {
    Exact([u8; 16]),
    WrongLength,
    InvalidType,
}

fn decode_identity_key(
    reader: &mut MessagePackReader<'_>,
) -> Result<IdentityKey, RnsBlackholeDecodeError> {
    let marker = reader.marker().map_err(message_pack)?;
    if !MessagePackReader::is_binary(marker) {
        reader
            .skip_value(marker, 1, MAXIMUM_DEPTH)
            .map_err(message_pack)?;
        return Ok(IdentityKey::InvalidType);
    }
    let bytes = reader
        .binary(marker)
        .map_err(message_pack)?
        .ok_or(RnsBlackholeDecodeError::MessagePack)?;
    Ok(<[u8; 16]>::try_from(bytes).map_or(IdentityKey::WrongLength, IdentityKey::Exact))
}

fn decode_entry(
    reader: &mut MessagePackReader<'_>,
    source_policy: &SourcePolicy,
) -> Result<ParsedEntry, RnsBlackholeDecodeError> {
    let marker = reader.marker().map_err(message_pack)?;
    let Some(length) = reader.map_length(marker).map_err(message_pack)? else {
        reader
            .skip_value(marker, 1, MAXIMUM_DEPTH)
            .map_err(message_pack)?;
        return Ok(ParsedEntry::ExpectedMap);
    };
    let mut fields = EntryFields {
        source: SourceField::Missing,
        until: UntilField::Missing,
        reason: ReasonField::Missing,
    };
    for _ in 0..length {
        let key_marker = reader.marker().map_err(message_pack)?;
        if !MessagePackReader::is_string(key_marker) {
            reader
                .skip_value(key_marker, 2, MAXIMUM_DEPTH)
                .map_err(message_pack)?;
            skip_next(reader, 2)?;
            continue;
        }
        let key = reader.string(key_marker).map_err(message_pack)?;
        match key {
            Some(blackhole::SOURCE) => {
                fields.source = decode_source(reader, source_policy)?;
            }
            Some(common::UNTIL) => fields.until = decode_until(reader)?,
            Some(common::REASON) => fields.reason = decode_reason(reader)?,
            Some(_) | None => skip_next(reader, 2)?,
        }
    }
    Ok(ParsedEntry::Fields(fields))
}

fn decode_source(
    reader: &mut MessagePackReader<'_>,
    source_policy: &SourcePolicy,
) -> Result<SourceField, RnsBlackholeDecodeError> {
    if matches!(source_policy, SourcePolicy::SourceFile(_)) {
        skip_next(reader, 2)?;
        return Ok(SourceField::Missing);
    }
    let marker = reader.marker().map_err(message_pack)?;
    if !MessagePackReader::is_binary(marker) {
        reader
            .skip_value(marker, 2, MAXIMUM_DEPTH)
            .map_err(message_pack)?;
        return Ok(SourceField::Invalid);
    }
    let bytes = reader
        .binary(marker)
        .map_err(message_pack)?
        .ok_or(RnsBlackholeDecodeError::MessagePack)?;
    Ok(
        <[u8; 16]>::try_from(bytes).map_or(SourceField::Invalid, |bytes| {
            SourceField::Identity(IdentityHash::new(bytes))
        }),
    )
}

fn decode_until(reader: &mut MessagePackReader<'_>) -> Result<UntilField, RnsBlackholeDecodeError> {
    let marker = reader.marker().map_err(message_pack)?;
    if marker == Marker::Null {
        return Ok(UntilField::Indefinite);
    }
    if MessagePackReader::is_integer(marker) {
        let integer = reader.integer(marker).map_err(message_pack)?;
        return Ok(integer.map_or(UntilField::Invalid, UntilField::Integer));
    }
    if matches!(marker, Marker::F32 | Marker::F64) {
        let value = reader.float(marker).map_err(message_pack)?;
        return Ok(value.map_or(UntilField::Invalid, UntilField::Float));
    }
    reader
        .skip_value(marker, 2, MAXIMUM_DEPTH)
        .map_err(message_pack)?;
    Ok(UntilField::Invalid)
}

fn decode_reason(
    reader: &mut MessagePackReader<'_>,
) -> Result<ReasonField, RnsBlackholeDecodeError> {
    let marker = reader.marker().map_err(message_pack)?;
    if marker == Marker::Null {
        return Ok(ReasonField::None);
    }
    if MessagePackReader::is_string(marker) {
        return Ok(reader
            .string(marker)
            .map_err(message_pack)?
            .map_or(ReasonField::Invalid, |value| {
                ReasonField::Text(String::from(value))
            }));
    }
    reader
        .skip_value(marker, 2, MAXIMUM_DEPTH)
        .map_err(message_pack)?;
    Ok(ReasonField::Invalid)
}

fn finish_entry(
    identity: [u8; 16],
    entry: ParsedEntry,
    source_policy: &SourcePolicy,
    now: InstantMillis,
) -> Result<Option<RnsBlackholeEntry>, RnsBlackholeDecodeError> {
    let ParsedEntry::Fields(fields) = entry else {
        return Err(RnsBlackholeDecodeError::ExpectedEntryMap);
    };
    let Some(expiry) = finish_expiry(fields.until, now)? else {
        return Ok(None);
    };
    let source = match source_policy {
        SourcePolicy::SourceFile(source) => *source,
        SourcePolicy::Published => match fields.source {
            SourceField::Identity(source) => source,
            SourceField::Missing | SourceField::Invalid => {
                return Err(RnsBlackholeDecodeError::InvalidSource);
            }
        },
    };
    let reason = match fields.reason {
        ReasonField::Missing | ReasonField::None => None,
        ReasonField::Text(reason) => Some(reason),
        ReasonField::Invalid => return Err(RnsBlackholeDecodeError::InvalidReason),
    };
    Ok(Some(RnsBlackholeEntry {
        identity: IdentityHash::new(identity),
        source,
        expiry,
        reason,
    }))
}

fn finish_expiry(
    until: UntilField,
    now: InstantMillis,
) -> Result<Option<BlackholeExpiry>, RnsBlackholeDecodeError> {
    match until {
        UntilField::Missing | UntilField::Indefinite => Ok(Some(BlackholeExpiry::Indefinite)),
        UntilField::Integer(MessagePackInteger::Negative(_)) => Ok(None),
        UntilField::Integer(MessagePackInteger::Nonnegative(seconds)) => {
            let deadline = seconds.saturating_mul(1_000);
            Ok((now.0 < deadline).then_some(BlackholeExpiry::At(InstantMillis(deadline))))
        }
        UntilField::Float(seconds) if seconds == f64::INFINITY => {
            Ok(Some(BlackholeExpiry::At(InstantMillis(u64::MAX))))
        }
        UntilField::Float(seconds) => {
            let millis = seconds * 1_000.0;
            if !millis.is_finite() || millis <= now.0 as f64 {
                return Ok(None);
            }
            let deadline = if millis >= u64::MAX as f64 {
                u64::MAX
            } else {
                millis as u64
            };
            Ok(Some(BlackholeExpiry::At(InstantMillis(deadline))))
        }
        UntilField::Invalid => Err(RnsBlackholeDecodeError::InvalidUntil),
    }
}

fn skip_next(
    reader: &mut MessagePackReader<'_>,
    depth: usize,
) -> Result<(), RnsBlackholeDecodeError> {
    let marker = reader.marker().map_err(message_pack)?;
    reader
        .skip_value(marker, depth, MAXIMUM_DEPTH)
        .map_err(message_pack)
}

fn message_pack(_: super::super::message_pack::MessagePackDecodeError) -> RnsBlackholeDecodeError {
    RnsBlackholeDecodeError::MessagePack
}
