use alloc::string::String;
use alloc::vec::Vec;

use crate::{ApplicationEventKind, DiagnosticEventKind, EventField, HOST_SCHEMA_VERSION};

const EVENT_BATCH_MAGIC: u32 = 0x454e_5250;
const EVENT_BATCH_FORMAT_VERSION: u16 = 1;
const EVENT_BATCH_HEADER_BYTES: usize = 16;
const EVENT_RECORD_HEADER_BYTES: usize = 8;
const EVENT_FIELD_HEADER_BYTES: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventProjectionKind(u16);

impl EventProjectionKind {
    pub fn try_new(value: u16) -> Result<Self, EventProjectionConstructionError> {
        if value == 0 {
            return Err(EventProjectionConstructionError::ZeroKind);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn value(self) -> u16 {
        self.0
    }
}

impl From<ApplicationEventKind> for EventProjectionKind {
    fn from(value: ApplicationEventKind) -> Self {
        Self(value as u16)
    }
}

impl From<DiagnosticEventKind> for EventProjectionKind {
    fn from(value: DiagnosticEventKind) -> Self {
        Self(value as u16)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventProjectionFieldId(u16);

impl EventProjectionFieldId {
    pub fn try_new(value: u16) -> Result<Self, EventProjectionConstructionError> {
        if value == 0 {
            return Err(EventProjectionConstructionError::ZeroFieldId);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn value(self) -> u16 {
        self.0
    }
}

impl From<EventField> for EventProjectionFieldId {
    fn from(value: EventField) -> Self {
        Self(value as u16)
    }
}

#[repr(u16)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventProjectionExtensionField {
    CommandId = 32_768,
}

impl From<EventProjectionExtensionField> for EventProjectionFieldId {
    fn from(value: EventProjectionExtensionField) -> Self {
        Self(value as u16)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventProjectionValue {
    Bytes(Vec<u8>),
    Text(String),
    U64(u64),
    U128(u128),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventProjectionField {
    id: EventProjectionFieldId,
    value: EventProjectionValue,
}

impl EventProjectionField {
    #[must_use]
    pub fn bytes(id: EventProjectionFieldId, value: Vec<u8>) -> Self {
        Self {
            id,
            value: EventProjectionValue::Bytes(value),
        }
    }

    #[must_use]
    pub fn text(id: EventProjectionFieldId, value: String) -> Self {
        Self {
            id,
            value: EventProjectionValue::Text(value),
        }
    }

    #[must_use]
    pub fn u64(id: EventProjectionFieldId, value: u64) -> Self {
        Self {
            id,
            value: EventProjectionValue::U64(value),
        }
    }

    #[must_use]
    pub fn u128(id: EventProjectionFieldId, value: u128) -> Self {
        Self {
            id,
            value: EventProjectionValue::U128(value),
        }
    }

    #[must_use]
    pub const fn id(&self) -> EventProjectionFieldId {
        self.id
    }

    #[must_use]
    pub const fn value(&self) -> &EventProjectionValue {
        &self.value
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventProjection {
    kind: EventProjectionKind,
    fields: Vec<EventProjectionField>,
}

impl EventProjection {
    #[must_use]
    pub fn new(kind: EventProjectionKind) -> Self {
        Self {
            kind,
            fields: Vec::new(),
        }
    }

    pub fn try_new(
        kind: EventProjectionKind,
        fields: Vec<EventProjectionField>,
    ) -> Result<Self, EventProjectionConstructionError> {
        if fields.len() > usize::from(u16::MAX) {
            return Err(EventProjectionConstructionError::TooManyFields);
        }
        for (index, field) in fields.iter().enumerate() {
            if fields[..index]
                .iter()
                .any(|existing| existing.id == field.id)
            {
                return Err(EventProjectionConstructionError::DuplicateField(field.id));
            }
        }
        Ok(Self { kind, fields })
    }

    pub fn set(&mut self, field: EventProjectionField) {
        if let Some(existing) = self
            .fields
            .iter_mut()
            .find(|existing| existing.id == field.id)
        {
            *existing = field;
            return;
        }
        self.fields.push(field);
    }

    #[must_use]
    pub const fn kind(&self) -> EventProjectionKind {
        self.kind
    }

    #[must_use]
    pub fn fields(&self) -> &[EventProjectionField] {
        &self.fields
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventProjectionConstructionError {
    ZeroKind,
    ZeroFieldId,
    TooManyFields,
    DuplicateField(EventProjectionFieldId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventBatchProjectionError {
    TooManyRecords,
    RecordTooLarge(EventProjectionKind),
    ValueTooLarge(EventProjectionFieldId),
    BatchTooLarge,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EventBatchProjection {
    records: Vec<EventProjection>,
}

impl EventBatchProjection {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn from_records(records: Vec<EventProjection>) -> Self {
        Self { records }
    }

    pub fn push(&mut self, record: EventProjection) {
        self.records.push(record);
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn encode(&self) -> Result<Vec<u8>, EventBatchProjectionError> {
        let record_count = u32::try_from(self.records.len())
            .map_err(|_| EventBatchProjectionError::TooManyRecords)?;
        let mut encoded = Vec::with_capacity(EVENT_BATCH_HEADER_BYTES);
        push_u32(&mut encoded, EVENT_BATCH_MAGIC);
        push_u16(&mut encoded, EVENT_BATCH_FORMAT_VERSION);
        push_u16(&mut encoded, 0);
        push_u32(&mut encoded, HOST_SCHEMA_VERSION);
        push_u32(&mut encoded, record_count);
        for record in &self.records {
            encode_record(&mut encoded, record)?;
        }
        Ok(encoded)
    }
}

fn encode_record(
    encoded: &mut Vec<u8>,
    record: &EventProjection,
) -> Result<(), EventBatchProjectionError> {
    let record_start = encoded.len();
    encoded.resize(
        record_start
            .checked_add(EVENT_RECORD_HEADER_BYTES)
            .ok_or(EventBatchProjectionError::BatchTooLarge)?,
        0,
    );
    for field in record.fields() {
        encode_field(encoded, field)?;
    }
    let body_bytes = encoded
        .len()
        .checked_sub(record_start + EVENT_RECORD_HEADER_BYTES)
        .ok_or(EventBatchProjectionError::BatchTooLarge)?;
    let body_bytes = u32::try_from(body_bytes)
        .map_err(|_| EventBatchProjectionError::RecordTooLarge(record.kind()))?;
    let field_count = u16::try_from(record.fields().len())
        .map_err(|_| EventBatchProjectionError::RecordTooLarge(record.kind()))?;
    encoded[record_start..record_start + 4].copy_from_slice(&body_bytes.to_le_bytes());
    encoded[record_start + 4..record_start + 6]
        .copy_from_slice(&record.kind().value().to_le_bytes());
    encoded[record_start + 6..record_start + 8].copy_from_slice(&field_count.to_le_bytes());
    Ok(())
}

fn encode_field(
    encoded: &mut Vec<u8>,
    field: &EventProjectionField,
) -> Result<(), EventBatchProjectionError> {
    let (wire_type, value_bytes): (u8, &[u8]) = match field.value() {
        EventProjectionValue::Bytes(value) => (1, value),
        EventProjectionValue::Text(value) => (2, value.as_bytes()),
        EventProjectionValue::U64(value) => (3, &value.to_le_bytes()),
        EventProjectionValue::U128(value) => (4, &value.to_le_bytes()),
    };
    let value_bytes_len = u32::try_from(value_bytes.len())
        .map_err(|_| EventBatchProjectionError::ValueTooLarge(field.id()))?;
    encoded
        .len()
        .checked_add(EVENT_FIELD_HEADER_BYTES)
        .and_then(|length| length.checked_add(value_bytes.len()))
        .ok_or(EventBatchProjectionError::BatchTooLarge)?;
    push_u16(encoded, field.id().value());
    encoded.push(wire_type);
    encoded.push(0);
    push_u32(encoded, value_bytes_len);
    encoded.extend_from_slice(value_bytes);
    Ok(())
}

fn push_u16(encoded: &mut Vec<u8>, value: u16) {
    encoded.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(encoded: &mut Vec<u8>, value: u32) {
    encoded.extend_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;

    #[test]
    fn encodes_stable_little_endian_batch() {
        let record = EventProjection::try_new(
            DiagnosticEventKind::LinkEstablished.into(),
            vec![
                EventProjectionField::bytes(EventField::LinkId.into(), vec![1, 2]),
                EventProjectionField::u64(EventField::RttMillis.into(), 9),
            ],
        )
        .unwrap();
        let encoded = EventBatchProjection::from_records(vec![record])
            .encode()
            .unwrap();
        assert_eq!(
            encoded,
            vec![
                80, 82, 78, 69, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 26, 0, 0, 0, 201, 0, 2, 0, 4,
                0, 1, 0, 2, 0, 0, 0, 1, 2, 8, 0, 3, 0, 8, 0, 0, 0, 9, 0, 0, 0, 0, 0, 0, 0,
            ]
        );
    }

    #[test]
    fn refuses_duplicate_fields() {
        let id = EventProjectionFieldId::from(EventField::Data);
        let outcome = EventProjection::try_new(
            ApplicationEventKind::Response.into(),
            vec![
                EventProjectionField::bytes(id, vec![1]),
                EventProjectionField::bytes(id, vec![2]),
            ],
        );
        assert_eq!(
            outcome,
            Err(EventProjectionConstructionError::DuplicateField(id))
        );
    }

    #[test]
    fn encodes_the_command_correlation_extension_stably() {
        let record = EventProjection::try_new(
            ApplicationEventKind::Response.into(),
            vec![EventProjectionField::u64(
                EventProjectionExtensionField::CommandId.into(),
                7,
            )],
        )
        .unwrap();
        let encoded = EventBatchProjection::from_records(vec![record])
            .encode()
            .unwrap();

        assert_eq!(
            encoded,
            vec![
                80, 82, 78, 69, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 16, 0, 0, 0, 102, 0, 1, 0, 0,
                128, 3, 0, 8, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0,
            ]
        );
    }
}
