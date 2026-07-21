use alloc::vec::Vec;
use core::fmt;

use rmp::Marker;

use crate::units::InstantMillis;
use crate::wire::DestinationHash;

use super::message_pack::{MessagePackInteger, MessagePackReader};
use super::wire_names::{common, rate};
use super::{rns_timestamp, MessagePackEncoder, RnsManagementEncodeError};

const MAXIMUM_DEPTH: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RnsAnnounceRateField {
    Hash,
    Last,
    Violations,
    BlockedUntil,
    Timestamps,
}

impl fmt::Display for RnsAnnounceRateField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Hash => common::HASH,
            Self::Last => rate::LAST,
            Self::Violations => rate::VIOLATIONS,
            Self::BlockedUntil => rate::BLOCKED_UNTIL,
            Self::Timestamps => rate::TIMESTAMPS,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RnsAnnounceRateEntry {
    destination: DestinationHash,
    last_allowed_announce_at_seconds: f64,
    blocked_until_seconds: f64,
    rate_violations: u64,
    observed_at_seconds: Vec<f64>,
}

impl RnsAnnounceRateEntry {
    pub fn new(
        destination: DestinationHash,
        last_allowed_announce_at: InstantMillis,
        blocked_until: InstantMillis,
        rate_violations: u16,
        observed_at: Vec<InstantMillis>,
    ) -> Self {
        Self {
            destination,
            last_allowed_announce_at_seconds: rns_timestamp(last_allowed_announce_at),
            blocked_until_seconds: if blocked_until.0 == 0 {
                0.0
            } else {
                rns_timestamp(blocked_until)
            },
            rate_violations: u64::from(rate_violations),
            observed_at_seconds: observed_at.into_iter().map(rns_timestamp).collect(),
        }
    }

    pub const fn destination(&self) -> DestinationHash {
        self.destination
    }

    pub const fn last_allowed_announce_at_seconds(&self) -> f64 {
        self.last_allowed_announce_at_seconds
    }

    pub const fn blocked_until_seconds(&self) -> f64 {
        self.blocked_until_seconds
    }

    pub const fn rate_violations(&self) -> u64 {
        self.rate_violations
    }

    pub fn observed_at_seconds(&self) -> &[f64] {
        &self.observed_at_seconds
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RnsAnnounceRateTable {
    entries: Vec<RnsAnnounceRateEntry>,
}

impl RnsAnnounceRateTable {
    pub fn new(entries: Vec<RnsAnnounceRateEntry>) -> Self {
        Self { entries }
    }

    pub fn entries(&self) -> &[RnsAnnounceRateEntry] {
        &self.entries
    }

    pub fn into_entries(self) -> Vec<RnsAnnounceRateEntry> {
        self.entries
    }

    pub fn decode_message_pack(bytes: &[u8]) -> Result<Self, RnsAnnounceRateTableDecodeError> {
        decode(bytes).map(Self::new)
    }

    pub fn encode_message_pack(&self) -> Result<Vec<u8>, RnsManagementEncodeError> {
        let mut encoder = MessagePackEncoder::new();
        self.encode_into(&mut encoder)?;
        Ok(encoder.finish())
    }

    pub(crate) fn encode_into(
        &self,
        encoder: &mut MessagePackEncoder,
    ) -> Result<(), RnsManagementEncodeError> {
        encoder.array(self.entries.len())?;
        for entry in &self.entries {
            encoder.map(5)?;
            encoder.field(common::HASH)?;
            encoder.binary(entry.destination.as_bytes())?;
            encoder.field(rate::LAST)?;
            encoder.float(entry.last_allowed_announce_at_seconds);
            encoder.field(rate::VIOLATIONS)?;
            encoder.unsigned(entry.rate_violations);
            encoder.field(rate::BLOCKED_UNTIL)?;
            if entry.blocked_until_seconds == 0.0 {
                encoder.unsigned(0);
            } else {
                encoder.float(entry.blocked_until_seconds);
            }
            encoder.field(rate::TIMESTAMPS)?;
            encoder.array(entry.observed_at_seconds.len())?;
            for timestamp in &entry.observed_at_seconds {
                encoder.float(*timestamp);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RnsAnnounceRateTableDecodeError {
    InvalidMessagePack,
    ExpectedArray,
    ExpectedEntryMap {
        index: usize,
    },
    InvalidMapKey {
        index: usize,
    },
    MissingField {
        index: usize,
        field: RnsAnnounceRateField,
    },
    DuplicateField {
        index: usize,
        field: RnsAnnounceRateField,
    },
    InvalidFieldType {
        index: usize,
        field: RnsAnnounceRateField,
    },
    InvalidHashLength {
        index: usize,
        actual: usize,
    },
    ExpectedTimestampsArray {
        index: usize,
    },
    AllocationFailed {
        entries: usize,
    },
    TimestampAllocationFailed {
        index: usize,
        entries: usize,
    },
    TrailingData,
}

impl fmt::Display for RnsAnnounceRateTableDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMessagePack => {
                formatter.write_str("invalid MessagePack announce-rate reply")
            }
            Self::ExpectedArray => formatter.write_str("announce-rate reply must be an array"),
            Self::ExpectedEntryMap { index } => {
                write!(formatter, "announce-rate entry {index} must be a map")
            }
            Self::InvalidMapKey { index } => write!(
                formatter,
                "announce-rate entry {index} contains a non-string field name"
            ),
            Self::MissingField { index, field } => {
                write!(formatter, "announce-rate entry {index} is missing {field}")
            }
            Self::DuplicateField { index, field } => {
                write!(formatter, "announce-rate entry {index} repeats {field}")
            }
            Self::InvalidFieldType { index, field } => write!(
                formatter,
                "announce-rate entry {index} has the wrong type at {field}"
            ),
            Self::InvalidHashLength { index, actual } => write!(
                formatter,
                "announce-rate entry {index} has {actual} hash bytes, expected 16"
            ),
            Self::ExpectedTimestampsArray { index } => write!(
                formatter,
                "announce-rate entry {index} timestamps must be an array"
            ),
            Self::AllocationFailed { entries } => write!(
                formatter,
                "announce-rate reply declares {entries} entries, but storage could not be allocated"
            ),
            Self::TimestampAllocationFailed { index, entries } => write!(
                formatter,
                "announce-rate entry {index} declares {entries} timestamps, but storage could not be allocated"
            ),
            Self::TrailingData => {
                formatter.write_str("announce-rate reply has trailing MessagePack data")
            }
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for RnsAnnounceRateTableDecodeError {}

#[derive(Default)]
struct EntryBuilder {
    destination: Option<DestinationHash>,
    last_allowed_announce_at_seconds: Option<f64>,
    blocked_until_seconds: Option<f64>,
    rate_violations: Option<u64>,
    observed_at_seconds: Option<Vec<f64>>,
}

impl EntryBuilder {
    fn finish(self, index: usize) -> Result<RnsAnnounceRateEntry, RnsAnnounceRateTableDecodeError> {
        Ok(RnsAnnounceRateEntry {
            destination: required(self.destination, index, RnsAnnounceRateField::Hash)?,
            last_allowed_announce_at_seconds: required(
                self.last_allowed_announce_at_seconds,
                index,
                RnsAnnounceRateField::Last,
            )?,
            blocked_until_seconds: required(
                self.blocked_until_seconds,
                index,
                RnsAnnounceRateField::BlockedUntil,
            )?,
            rate_violations: required(
                self.rate_violations,
                index,
                RnsAnnounceRateField::Violations,
            )?,
            observed_at_seconds: required(
                self.observed_at_seconds,
                index,
                RnsAnnounceRateField::Timestamps,
            )?,
        })
    }
}

fn decode(bytes: &[u8]) -> Result<Vec<RnsAnnounceRateEntry>, RnsAnnounceRateTableDecodeError> {
    let mut reader = MessagePackReader::new(bytes);
    let marker = reader.marker().map_err(message_pack)?;
    let length = reader
        .array_length(marker)
        .map_err(message_pack)?
        .ok_or(RnsAnnounceRateTableDecodeError::ExpectedArray)?;
    let mut entries = Vec::new();
    entries
        .try_reserve(length)
        .map_err(|_| RnsAnnounceRateTableDecodeError::AllocationFailed { entries: length })?;
    for index in 0..length {
        entries.push(decode_entry(&mut reader, index)?);
    }
    if !reader.is_finished() {
        return Err(RnsAnnounceRateTableDecodeError::TrailingData);
    }
    Ok(entries)
}

fn decode_entry(
    reader: &mut MessagePackReader<'_>,
    index: usize,
) -> Result<RnsAnnounceRateEntry, RnsAnnounceRateTableDecodeError> {
    let marker = reader.marker().map_err(message_pack)?;
    let length = reader
        .map_length(marker)
        .map_err(message_pack)?
        .ok_or(RnsAnnounceRateTableDecodeError::ExpectedEntryMap { index })?;
    let mut builder = EntryBuilder::default();
    for _ in 0..length {
        let key_marker = reader.marker().map_err(message_pack)?;
        let key = reader
            .string(key_marker)
            .map_err(message_pack)?
            .ok_or(RnsAnnounceRateTableDecodeError::InvalidMapKey { index })?;
        let value_marker = reader.marker().map_err(message_pack)?;
        match key {
            common::HASH => set(
                &mut builder.destination,
                decode_hash(reader, value_marker, index)?.map(DestinationHash::new),
                index,
                RnsAnnounceRateField::Hash,
            )?,
            rate::LAST => set(
                &mut builder.last_allowed_announce_at_seconds,
                decode_number(reader, value_marker)?,
                index,
                RnsAnnounceRateField::Last,
            )?,
            rate::VIOLATIONS => set(
                &mut builder.rate_violations,
                decode_nonnegative(reader, value_marker)?,
                index,
                RnsAnnounceRateField::Violations,
            )?,
            rate::BLOCKED_UNTIL => set(
                &mut builder.blocked_until_seconds,
                decode_number(reader, value_marker)?,
                index,
                RnsAnnounceRateField::BlockedUntil,
            )?,
            rate::TIMESTAMPS => set(
                &mut builder.observed_at_seconds,
                Some(decode_timestamps(reader, value_marker, index)?),
                index,
                RnsAnnounceRateField::Timestamps,
            )?,
            _ => reader
                .skip_value(value_marker, 1, MAXIMUM_DEPTH)
                .map_err(message_pack)?,
        }
    }
    builder.finish(index)
}

fn decode_hash(
    reader: &mut MessagePackReader<'_>,
    marker: Marker,
    index: usize,
) -> Result<Option<[u8; 16]>, RnsAnnounceRateTableDecodeError> {
    let Some(bytes) = reader.binary(marker).map_err(message_pack)? else {
        return Ok(None);
    };
    bytes
        .try_into()
        .map(Some)
        .map_err(|_| RnsAnnounceRateTableDecodeError::InvalidHashLength {
            index,
            actual: bytes.len(),
        })
}

fn decode_timestamps(
    reader: &mut MessagePackReader<'_>,
    marker: Marker,
    index: usize,
) -> Result<Vec<f64>, RnsAnnounceRateTableDecodeError> {
    let length = reader
        .array_length(marker)
        .map_err(message_pack)?
        .ok_or(RnsAnnounceRateTableDecodeError::ExpectedTimestampsArray { index })?;
    let mut timestamps = Vec::new();
    timestamps.try_reserve(length).map_err(|_| {
        RnsAnnounceRateTableDecodeError::TimestampAllocationFailed {
            index,
            entries: length,
        }
    })?;
    for _ in 0..length {
        let marker = reader.marker().map_err(message_pack)?;
        let Some(timestamp) = decode_number(reader, marker)? else {
            return Err(RnsAnnounceRateTableDecodeError::InvalidFieldType {
                index,
                field: RnsAnnounceRateField::Timestamps,
            });
        };
        timestamps.push(timestamp);
    }
    Ok(timestamps)
}

fn decode_nonnegative(
    reader: &mut MessagePackReader<'_>,
    marker: Marker,
) -> Result<Option<u64>, RnsAnnounceRateTableDecodeError> {
    Ok(match reader.integer(marker).map_err(message_pack)? {
        Some(MessagePackInteger::Nonnegative(value)) => Some(value),
        Some(MessagePackInteger::Negative(_)) | None => None,
    })
}

fn decode_number(
    reader: &mut MessagePackReader<'_>,
    marker: Marker,
) -> Result<Option<f64>, RnsAnnounceRateTableDecodeError> {
    Ok(match reader.integer(marker).map_err(message_pack)? {
        Some(MessagePackInteger::Negative(value)) => Some(value as f64),
        Some(MessagePackInteger::Nonnegative(value)) => Some(value as f64),
        None => reader.float(marker).map_err(message_pack)?,
    })
}

fn set<T>(
    slot: &mut Option<T>,
    value: Option<T>,
    index: usize,
    field: RnsAnnounceRateField,
) -> Result<(), RnsAnnounceRateTableDecodeError> {
    if slot.is_some() {
        return Err(RnsAnnounceRateTableDecodeError::DuplicateField { index, field });
    }
    let Some(value) = value else {
        return Err(RnsAnnounceRateTableDecodeError::InvalidFieldType { index, field });
    };
    *slot = Some(value);
    Ok(())
}

fn required<T>(
    value: Option<T>,
    index: usize,
    field: RnsAnnounceRateField,
) -> Result<T, RnsAnnounceRateTableDecodeError> {
    value.ok_or(RnsAnnounceRateTableDecodeError::MissingField { index, field })
}

fn message_pack(_: super::message_pack::MessagePackDecodeError) -> RnsAnnounceRateTableDecodeError {
    RnsAnnounceRateTableDecodeError::InvalidMessagePack
}

#[cfg(test)]
mod tests {
    use super::*;

    const RNS_140_RATE_TABLE: &str = "9185a468617368c41033333333333333333333333333333333a46c617374cb41d954fc40100000af726174655f76696f6c6174696f6e7303ad626c6f636b65645f756e74696ccb41d954fc72300000aa74696d657374616d707392cb41d954fb46000000cb41d954fc40100000";

    #[test]
    fn decodes_and_reencodes_the_rns_138_rate_table_fixture() {
        let bytes = bytes_from_hex(RNS_140_RATE_TABLE);
        let table = RnsAnnounceRateTable::decode_message_pack(&bytes).unwrap();
        assert_eq!(table.entries().len(), 1);
        let entry = &table.entries()[0];
        assert_eq!(entry.destination(), DestinationHash::new([0x33; 16]));
        assert_eq!(entry.last_allowed_announce_at_seconds(), 1_700_000_000.25);
        assert_eq!(entry.rate_violations(), 3);
        assert_eq!(entry.blocked_until_seconds(), 1_700_000_200.75);
        assert_eq!(
            entry.observed_at_seconds(),
            &[1_699_999_000.0, 1_700_000_000.25]
        );
        assert_eq!(table.encode_message_pack(), Ok(bytes));
    }

    #[test]
    fn malformed_rate_tables_fail_with_typed_errors() {
        assert_eq!(
            RnsAnnounceRateTable::decode_message_pack(&[0x80]),
            Err(RnsAnnounceRateTableDecodeError::ExpectedArray)
        );
        assert_eq!(
            RnsAnnounceRateTable::decode_message_pack(&[0x90, 0x00]),
            Err(RnsAnnounceRateTableDecodeError::TrailingData)
        );
    }

    fn bytes_from_hex(value: &str) -> Vec<u8> {
        value
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(core::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }
}
