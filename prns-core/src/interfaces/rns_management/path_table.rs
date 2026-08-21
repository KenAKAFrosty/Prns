use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

use rmp::Marker;

use crate::engine::RouteSnapshot;
use crate::units::InstantMillis;
use crate::wire::{DestinationHash, TransportId};

use super::message_pack::{MessagePackInteger, MessagePackReader};
use super::wire_names::{common, path};
use super::{
    interface_name, next_hop_bytes, rns_timestamp, MessagePackEncoder, RnsManagementEncodeError,
};

const MAXIMUM_DEPTH: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RnsPathTableField {
    Hash,
    Via,
    Hops,
    Timestamp,
    Expires,
    Interface,
}

impl fmt::Display for RnsPathTableField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Hash => common::HASH,
            Self::Via => path::VIA,
            Self::Hops => path::HOPS,
            Self::Timestamp => path::TIMESTAMP,
            Self::Expires => path::EXPIRES,
            Self::Interface => path::INTERFACE,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RnsPathTableEntry {
    destination: DestinationHash,
    via: TransportId,
    hops: u64,
    learned_at_seconds: f64,
    expires_at_seconds: f64,
    interface: String,
}

impl RnsPathTableEntry {
    pub const fn destination(&self) -> DestinationHash {
        self.destination
    }

    pub const fn via(&self) -> TransportId {
        self.via
    }

    pub const fn hops(&self) -> u64 {
        self.hops
    }

    pub const fn learned_at_seconds(&self) -> f64 {
        self.learned_at_seconds
    }

    pub const fn expires_at_seconds(&self) -> f64 {
        self.expires_at_seconds
    }

    pub fn interface(&self) -> &str {
        &self.interface
    }
}

impl From<RouteSnapshot> for RnsPathTableEntry {
    fn from(entry: RouteSnapshot) -> Self {
        Self {
            destination: entry.destination,
            via: TransportId::new(next_hop_bytes(&entry)),
            hops: u64::from(entry.hops),
            learned_at_seconds: rns_timestamp(InstantMillis(
                entry.learned_at.0.max(entry.last_route_activity_at.0),
            )),
            expires_at_seconds: rns_timestamp(entry.expires_at),
            interface: interface_name(entry.interface),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RnsPathTable {
    entries: Vec<RnsPathTableEntry>,
}

impl RnsPathTable {
    pub fn new(entries: Vec<RouteSnapshot>) -> Self {
        Self {
            entries: entries.into_iter().map(RnsPathTableEntry::from).collect(),
        }
    }

    pub fn from_entries(entries: Vec<RnsPathTableEntry>) -> Self {
        Self { entries }
    }

    pub fn entries(&self) -> &[RnsPathTableEntry] {
        &self.entries
    }

    pub fn into_entries(self) -> Vec<RnsPathTableEntry> {
        self.entries
    }

    pub fn decode_message_pack(bytes: &[u8]) -> Result<Self, RnsPathTableDecodeError> {
        decode(bytes).map(Self::from_entries)
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
            encoder.map(6)?;
            encoder.field(common::HASH)?;
            encoder.binary(entry.destination.as_bytes())?;
            encoder.field(path::TIMESTAMP)?;
            encoder.float(entry.learned_at_seconds);
            encoder.field(path::VIA)?;
            encoder.binary(entry.via.as_bytes())?;
            encoder.unsigned_field(path::HOPS, entry.hops)?;
            encoder.field(path::EXPIRES)?;
            encoder.float(entry.expires_at_seconds);
            encoder.string_field(path::INTERFACE, &entry.interface)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RnsPathTableDecodeError {
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
        field: RnsPathTableField,
    },
    DuplicateField {
        index: usize,
        field: RnsPathTableField,
    },
    InvalidFieldType {
        index: usize,
        field: RnsPathTableField,
    },
    InvalidHashLength {
        index: usize,
        field: RnsPathTableField,
        actual: usize,
    },
    AllocationFailed {
        entries: usize,
    },
    TrailingData,
}

impl fmt::Display for RnsPathTableDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMessagePack => formatter.write_str("invalid MessagePack path-table reply"),
            Self::ExpectedArray => formatter.write_str("path-table reply must be an array"),
            Self::ExpectedEntryMap { index } => {
                write!(formatter, "path-table entry {index} must be a map")
            }
            Self::InvalidMapKey { index } => {
                write!(
                    formatter,
                    "path-table entry {index} contains a non-string field name"
                )
            }
            Self::MissingField { index, field } => {
                write!(formatter, "path-table entry {index} is missing {field}")
            }
            Self::DuplicateField { index, field } => {
                write!(formatter, "path-table entry {index} repeats {field}")
            }
            Self::InvalidFieldType { index, field } => {
                write!(
                    formatter,
                    "path-table entry {index} has the wrong type at {field}"
                )
            }
            Self::InvalidHashLength {
                index,
                field,
                actual,
            } => write!(
                formatter,
                "path-table entry {index} has {actual} bytes at {field}, expected 16"
            ),
            Self::AllocationFailed { entries } => write!(
                formatter,
                "path-table reply declares {entries} entries, but storage could not be allocated"
            ),
            Self::TrailingData => {
                formatter.write_str("path-table reply has trailing MessagePack data")
            }
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for RnsPathTableDecodeError {}

#[derive(Default)]
struct EntryBuilder {
    destination: Option<DestinationHash>,
    via: Option<TransportId>,
    hops: Option<u64>,
    learned_at_seconds: Option<f64>,
    expires_at_seconds: Option<f64>,
    interface: Option<String>,
}

impl EntryBuilder {
    fn finish(self, index: usize) -> Result<RnsPathTableEntry, RnsPathTableDecodeError> {
        Ok(RnsPathTableEntry {
            destination: required(self.destination, index, RnsPathTableField::Hash)?,
            via: required(self.via, index, RnsPathTableField::Via)?,
            hops: required(self.hops, index, RnsPathTableField::Hops)?,
            learned_at_seconds: required(
                self.learned_at_seconds,
                index,
                RnsPathTableField::Timestamp,
            )?,
            expires_at_seconds: required(
                self.expires_at_seconds,
                index,
                RnsPathTableField::Expires,
            )?,
            interface: required(self.interface, index, RnsPathTableField::Interface)?,
        })
    }
}

fn decode(bytes: &[u8]) -> Result<Vec<RnsPathTableEntry>, RnsPathTableDecodeError> {
    let mut reader = MessagePackReader::new(bytes);
    let marker = reader.marker().map_err(message_pack)?;
    let length = reader
        .array_length(marker)
        .map_err(message_pack)?
        .ok_or(RnsPathTableDecodeError::ExpectedArray)?;
    let mut entries = Vec::new();
    entries
        .try_reserve(length)
        .map_err(|_| RnsPathTableDecodeError::AllocationFailed { entries: length })?;
    for index in 0..length {
        entries.push(decode_entry(&mut reader, index)?);
    }
    if !reader.is_finished() {
        return Err(RnsPathTableDecodeError::TrailingData);
    }
    Ok(entries)
}

fn decode_entry(
    reader: &mut MessagePackReader<'_>,
    index: usize,
) -> Result<RnsPathTableEntry, RnsPathTableDecodeError> {
    let marker = reader.marker().map_err(message_pack)?;
    let length = reader
        .map_length(marker)
        .map_err(message_pack)?
        .ok_or(RnsPathTableDecodeError::ExpectedEntryMap { index })?;
    let mut builder = EntryBuilder::default();
    for _ in 0..length {
        let key_marker = reader.marker().map_err(message_pack)?;
        let key = reader
            .string(key_marker)
            .map_err(message_pack)?
            .ok_or(RnsPathTableDecodeError::InvalidMapKey { index })?;
        let value_marker = reader.marker().map_err(message_pack)?;
        match key {
            common::HASH => set(
                &mut builder.destination,
                decode_hash(reader, value_marker, index, RnsPathTableField::Hash)?
                    .map(DestinationHash::new),
                index,
                RnsPathTableField::Hash,
            )?,
            path::VIA => set(
                &mut builder.via,
                decode_hash(reader, value_marker, index, RnsPathTableField::Via)?
                    .map(TransportId::new),
                index,
                RnsPathTableField::Via,
            )?,
            path::HOPS => set(
                &mut builder.hops,
                decode_nonnegative(reader, value_marker)?,
                index,
                RnsPathTableField::Hops,
            )?,
            path::TIMESTAMP => set(
                &mut builder.learned_at_seconds,
                decode_number(reader, value_marker)?,
                index,
                RnsPathTableField::Timestamp,
            )?,
            path::EXPIRES => set(
                &mut builder.expires_at_seconds,
                decode_number(reader, value_marker)?,
                index,
                RnsPathTableField::Expires,
            )?,
            path::INTERFACE => set(
                &mut builder.interface,
                reader
                    .string(value_marker)
                    .map_err(message_pack)?
                    .map(ToString::to_string),
                index,
                RnsPathTableField::Interface,
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
    field: RnsPathTableField,
) -> Result<Option<[u8; 16]>, RnsPathTableDecodeError> {
    let Some(bytes) = reader.binary(marker).map_err(message_pack)? else {
        return Ok(None);
    };
    bytes
        .try_into()
        .map(Some)
        .map_err(|_| RnsPathTableDecodeError::InvalidHashLength {
            index,
            field,
            actual: bytes.len(),
        })
}

fn decode_nonnegative(
    reader: &mut MessagePackReader<'_>,
    marker: Marker,
) -> Result<Option<u64>, RnsPathTableDecodeError> {
    Ok(match reader.integer(marker).map_err(message_pack)? {
        Some(MessagePackInteger::Nonnegative(value)) => Some(value),
        Some(MessagePackInteger::Negative(_)) | None => None,
    })
}

fn decode_number(
    reader: &mut MessagePackReader<'_>,
    marker: Marker,
) -> Result<Option<f64>, RnsPathTableDecodeError> {
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
    field: RnsPathTableField,
) -> Result<(), RnsPathTableDecodeError> {
    if slot.is_some() {
        return Err(RnsPathTableDecodeError::DuplicateField { index, field });
    }
    let Some(value) = value else {
        return Err(RnsPathTableDecodeError::InvalidFieldType { index, field });
    };
    *slot = Some(value);
    Ok(())
}

fn required<T>(
    value: Option<T>,
    index: usize,
    field: RnsPathTableField,
) -> Result<T, RnsPathTableDecodeError> {
    value.ok_or(RnsPathTableDecodeError::MissingField { index, field })
}

fn message_pack(_: super::message_pack::MessagePackDecodeError) -> RnsPathTableDecodeError {
    RnsPathTableDecodeError::InvalidMessagePack
}

#[cfg(test)]
mod tests {
    use super::*;

    const RNS_1_4_2_PATH_TABLE: &str = "9186a468617368c41011111111111111111111111111111111a974696d657374616d70cb41d954fc40080000a3766961c41022222222222222222222222222222222a4686f707302a765787069726573cb41d954fc59200000a9696e74657266616365ba544350436c69656e74496e746572666163655b6f7261636c655d";

    #[test]
    fn decodes_and_reencodes_the_rns_1_4_2_path_table_fixture() {
        let bytes = bytes_from_hex(RNS_1_4_2_PATH_TABLE);
        let table = RnsPathTable::decode_message_pack(&bytes).unwrap();
        assert_eq!(table.entries().len(), 1);
        let entry = &table.entries()[0];
        assert_eq!(entry.destination(), DestinationHash::new([0x11; 16]));
        assert_eq!(entry.via(), TransportId::new([0x22; 16]));
        assert_eq!(entry.hops(), 2);
        assert_eq!(entry.learned_at_seconds(), 1_700_000_000.125);
        assert_eq!(entry.expires_at_seconds(), 1_700_000_100.5);
        assert_eq!(entry.interface(), "TCPClientInterface[oracle]");
        assert_eq!(table.encode_message_pack(), Ok(bytes));
    }

    #[test]
    fn malformed_path_tables_fail_with_typed_errors() {
        assert_eq!(
            RnsPathTable::decode_message_pack(&[0x80]),
            Err(RnsPathTableDecodeError::ExpectedArray)
        );
        assert_eq!(
            RnsPathTable::decode_message_pack(&[0x90, 0x00]),
            Err(RnsPathTableDecodeError::TrailingData)
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
