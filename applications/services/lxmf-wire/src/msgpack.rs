use core::{fmt, str};

use crate::{LimitKind, WireError, WireLimits, ABSOLUTE_MAX_NESTING_DEPTH};

/// Whether Python u-msgpack decoding and default re-encoding is proven to
/// preserve one value's exact bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Canonicality {
    /// Exact bytes are a proven fixed point.
    Canonical,
    /// Exact bytes are known to be re-encoded differently.
    NonCanonical,
}

impl Canonicality {
    fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::NonCanonical, _) | (_, Self::NonCanonical) => Self::NonCanonical,
            (Self::Canonical, Self::Canonical) => Self::Canonical,
        }
    }
}

/// Structural kind of one MessagePack value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessagePackKind {
    /// Nil.
    Nil,
    /// Boolean.
    Boolean,
    /// Signed or unsigned integer.
    Integer,
    /// IEEE-754 binary32.
    Float32,
    /// IEEE-754 binary64.
    Float64,
    /// UTF-8 string.
    String,
    /// Binary bytes.
    Binary,
    /// Array.
    Array,
    /// Map.
    Map,
    /// Application extension.
    Extension,
}

/// Borrowed, structurally validated MessagePack value.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct MessagePackValue<'a> {
    raw: &'a [u8],
    kind: MessagePackKind,
    canonicality: Canonicality,
}

impl<'a> MessagePackValue<'a> {
    pub(crate) const fn from_scanned(
        raw: &'a [u8],
        kind: MessagePackKind,
        canonicality: Canonicality,
    ) -> Self {
        Self {
            raw,
            kind,
            canonicality,
        }
    }

    /// Complete encoded value bytes.
    pub const fn raw(self) -> &'a [u8] {
        self.raw
    }

    /// Structural value kind.
    pub const fn kind(self) -> MessagePackKind {
        self.kind
    }

    /// Whether this value is a proven fixed point under Python u-msgpack's
    /// default decoder and encoder.
    ///
    /// Map-key forms whose Python equality cannot be proven and timestamp
    /// extension type `-1` are rejected with typed errors before this view is
    /// constructed.
    pub const fn is_python_canonical(self) -> bool {
        matches!(self.canonicality, Canonicality::Canonical)
    }

    /// Detailed fixed-point classification.
    pub const fn canonicality(self) -> Canonicality {
        self.canonicality
    }
}

impl fmt::Debug for MessagePackValue<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MessagePackValue")
            .field("kind", &self.kind)
            .field("encoded_len", &self.raw.len())
            .field("canonicality", &self.canonicality)
            .finish()
    }
}

/// Validate exactly one arbitrary MessagePack value without allocating.
pub fn validate_messagepack_value(
    raw: &[u8],
    limits: WireLimits,
) -> Result<MessagePackValue<'_>, WireError> {
    if raw.len() > limits.max_wire_bytes {
        return Err(WireError::LimitExceeded {
            kind: LimitKind::WireBytes,
            actual: raw.len(),
            maximum: limits.max_wire_bytes,
            offset: 0,
        });
    }
    let mut scanner = Scanner::new(raw, limits);
    let scanned = scanner.scan_value(0, 1)?;
    if scanned.end != raw.len() {
        return Err(WireError::TrailingBytes {
            offset: scanned.end,
            trailing: raw.len() - scanned.end,
        });
    }
    Ok(MessagePackValue::from_scanned(
        raw,
        scanned.kind,
        scanned.canonicality,
    ))
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Scanned {
    pub(crate) end: usize,
    pub(crate) kind: MessagePackKind,
    pub(crate) canonicality: Canonicality,
}

pub(crate) struct Scanner<'a> {
    bytes: &'a [u8],
    limits: WireLimits,
    total_values: usize,
    scan_steps: usize,
}

impl<'a> Scanner<'a> {
    pub(crate) const fn new(bytes: &'a [u8], limits: WireLimits) -> Self {
        Self {
            bytes,
            limits,
            total_values: 0,
            scan_steps: 0,
        }
    }

    pub(crate) fn note_container(
        &mut self,
        offset: usize,
        depth: usize,
        count: usize,
    ) -> Result<(), WireError> {
        self.bump_scan_step(offset)?;
        self.check_depth(offset, depth)?;
        self.bump_value(offset)?;
        self.check_container(offset, count)
    }

    pub(crate) fn scan_value(&mut self, offset: usize, depth: usize) -> Result<Scanned, WireError> {
        self.scan_value_mode(offset, depth, true)
    }

    fn scan_value_mode(
        &mut self,
        offset: usize,
        depth: usize,
        count_value: bool,
    ) -> Result<Scanned, WireError> {
        self.bump_scan_step(offset)?;
        self.check_depth(offset, depth)?;
        if count_value {
            self.bump_value(offset)?;
        }
        let marker = self.byte(offset, offset)?;
        match marker {
            0x00..=0x7f | 0xe0..=0xff => Ok(Scanned {
                end: offset + 1,
                kind: MessagePackKind::Integer,
                canonicality: Canonicality::Canonical,
            }),
            0x80..=0x8f => {
                let count = usize::from(marker & 0x0f);
                self.scan_map(offset, offset + 1, count, depth, true, count_value)
            }
            0x90..=0x9f => {
                let count = usize::from(marker & 0x0f);
                self.scan_array(offset, offset + 1, count, depth, true, count_value)
            }
            0xa0..=0xbf => {
                let length = usize::from(marker & 0x1f);
                self.scan_string(offset, offset + 1, length, true)
            }
            0xc0 => self.scalar(offset, 1, MessagePackKind::Nil, true),
            0xc1 => Err(WireError::ReservedMarker { offset }),
            0xc2 | 0xc3 => self.scalar(offset, 1, MessagePackKind::Boolean, true),
            0xc4 => {
                let length = usize::from(self.byte(offset + 1, offset)?);
                self.scan_blob(
                    offset,
                    offset + 2,
                    length,
                    MessagePackKind::Binary,
                    length < 256,
                )
            }
            0xc5 => {
                let length = usize::from(self.u16(offset + 1, offset)?);
                self.scan_blob(
                    offset,
                    offset + 3,
                    length,
                    MessagePackKind::Binary,
                    length >= 256,
                )
            }
            0xc6 => {
                let length = self.usize_u32(offset + 1, offset)?;
                self.scan_blob(
                    offset,
                    offset + 5,
                    length,
                    MessagePackKind::Binary,
                    length >= 65_536,
                )
            }
            0xc7 => {
                let length = usize::from(self.byte(offset + 1, offset)?);
                let canonical = length < 256 && !matches!(length, 1 | 2 | 4 | 8 | 16);
                self.scan_extension(offset, offset + 2, length, canonical)
            }
            0xc8 => {
                let length = usize::from(self.u16(offset + 1, offset)?);
                self.scan_extension(offset, offset + 3, length, length >= 256)
            }
            0xc9 => {
                let length = self.usize_u32(offset + 1, offset)?;
                self.scan_extension(offset, offset + 5, length, length >= 65_536)
            }
            0xca => self.scalar(offset, 5, MessagePackKind::Float32, false),
            0xcb => self.scalar(offset, 9, MessagePackKind::Float64, true),
            0xcc => {
                let value = self.byte(offset + 1, offset)?;
                self.scalar(offset, 2, MessagePackKind::Integer, value >= 128)
            }
            0xcd => {
                let value = self.u16(offset + 1, offset)?;
                self.scalar(offset, 3, MessagePackKind::Integer, value >= 256)
            }
            0xce => {
                let value = self.u32(offset + 1, offset)?;
                self.scalar(offset, 5, MessagePackKind::Integer, value >= 65_536)
            }
            0xcf => {
                let value = self.u64(offset + 1, offset)?;
                self.scalar(offset, 9, MessagePackKind::Integer, value >= 4_294_967_296)
            }
            0xd0 => {
                let value = self.byte(offset + 1, offset)? as i8;
                self.scalar(offset, 2, MessagePackKind::Integer, value < -32)
            }
            0xd1 => {
                let value = self.u16(offset + 1, offset)? as i16;
                self.scalar(offset, 3, MessagePackKind::Integer, value < -128)
            }
            0xd2 => {
                let value = self.u32(offset + 1, offset)? as i32;
                self.scalar(offset, 5, MessagePackKind::Integer, value < -32_768)
            }
            0xd3 => {
                let value = self.u64(offset + 1, offset)? as i64;
                self.scalar(offset, 9, MessagePackKind::Integer, value < -2_147_483_648)
            }
            0xd4 => self.scan_extension(offset, offset + 1, 1, true),
            0xd5 => self.scan_extension(offset, offset + 1, 2, true),
            0xd6 => self.scan_extension(offset, offset + 1, 4, true),
            0xd7 => self.scan_extension(offset, offset + 1, 8, true),
            0xd8 => self.scan_extension(offset, offset + 1, 16, true),
            0xd9 => {
                let length = usize::from(self.byte(offset + 1, offset)?);
                self.scan_string(offset, offset + 2, length, (32..256).contains(&length))
            }
            0xda => {
                let length = usize::from(self.u16(offset + 1, offset)?);
                self.scan_string(offset, offset + 3, length, length >= 256)
            }
            0xdb => {
                let length = self.usize_u32(offset + 1, offset)?;
                self.scan_string(offset, offset + 5, length, length >= 65_536)
            }
            0xdc => {
                let count = usize::from(self.u16(offset + 1, offset)?);
                self.scan_array(offset, offset + 3, count, depth, count >= 16, count_value)
            }
            0xdd => {
                let count = self.usize_u32(offset + 1, offset)?;
                self.scan_array(
                    offset,
                    offset + 5,
                    count,
                    depth,
                    count >= 65_536,
                    count_value,
                )
            }
            0xde => {
                let count = usize::from(self.u16(offset + 1, offset)?);
                self.scan_map(offset, offset + 3, count, depth, count >= 16, count_value)
            }
            0xdf => {
                let count = self.usize_u32(offset + 1, offset)?;
                self.scan_map(
                    offset,
                    offset + 5,
                    count,
                    depth,
                    count >= 65_536,
                    count_value,
                )
            }
        }
    }

    fn scalar(
        &self,
        offset: usize,
        length: usize,
        kind: MessagePackKind,
        python_canonical: bool,
    ) -> Result<Scanned, WireError> {
        let end = self.take_end(offset, length, offset)?;
        Ok(Scanned {
            end,
            kind,
            canonicality: if python_canonical {
                Canonicality::Canonical
            } else {
                Canonicality::NonCanonical
            },
        })
    }

    fn scan_blob(
        &self,
        marker_offset: usize,
        payload_offset: usize,
        length: usize,
        kind: MessagePackKind,
        python_canonical: bool,
    ) -> Result<Scanned, WireError> {
        self.check_value_bytes(marker_offset, length)?;
        let end = self.take_end(payload_offset, length, marker_offset)?;
        Ok(Scanned {
            end,
            kind,
            canonicality: if python_canonical {
                Canonicality::Canonical
            } else {
                Canonicality::NonCanonical
            },
        })
    }

    fn scan_string(
        &self,
        marker_offset: usize,
        payload_offset: usize,
        length: usize,
        python_canonical: bool,
    ) -> Result<Scanned, WireError> {
        let scanned = self.scan_blob(
            marker_offset,
            payload_offset,
            length,
            MessagePackKind::String,
            python_canonical,
        )?;
        if str::from_utf8(&self.bytes[payload_offset..scanned.end]).is_err() {
            return Err(WireError::InvalidUtf8 {
                offset: marker_offset,
            });
        }
        Ok(scanned)
    }

    fn scan_extension(
        &self,
        marker_offset: usize,
        type_offset: usize,
        length: usize,
        marker_canonical: bool,
    ) -> Result<Scanned, WireError> {
        self.check_value_bytes(marker_offset, length)?;
        let payload_offset = type_offset
            .checked_add(1)
            .ok_or(WireError::LengthOverflow {
                offset: marker_offset,
            })?;
        let end = self.take_end(payload_offset, length, marker_offset)?;
        let extension_type = self.byte(type_offset, marker_offset)? as i8;
        if extension_type == -1 {
            return Err(WireError::UnsupportedTimestampExtension {
                offset: marker_offset,
            });
        }
        Ok(Scanned {
            end,
            kind: MessagePackKind::Extension,
            canonicality: if marker_canonical {
                Canonicality::Canonical
            } else {
                Canonicality::NonCanonical
            },
        })
    }

    fn scan_array(
        &mut self,
        marker_offset: usize,
        mut cursor: usize,
        count: usize,
        depth: usize,
        marker_canonical: bool,
        count_values: bool,
    ) -> Result<Scanned, WireError> {
        self.check_container(marker_offset, count)?;
        let mut canonicality = if marker_canonical {
            Canonicality::Canonical
        } else {
            Canonicality::NonCanonical
        };
        for _ in 0..count {
            let child = self.scan_value_mode(cursor, depth + 1, count_values)?;
            cursor = child.end;
            canonicality = canonicality.merge(child.canonicality);
        }
        Ok(Scanned {
            end: cursor,
            kind: MessagePackKind::Array,
            canonicality,
        })
    }

    fn scan_map(
        &mut self,
        marker_offset: usize,
        mut cursor: usize,
        count: usize,
        depth: usize,
        marker_canonical: bool,
        count_values: bool,
    ) -> Result<Scanned, WireError> {
        self.check_container(marker_offset, count)?;
        let entries_offset = cursor;
        let mut canonicality = if marker_canonical {
            Canonicality::Canonical
        } else {
            Canonicality::NonCanonical
        };
        for entry_index in 0..count {
            let key_start = cursor;
            let key = self.scan_value_mode(cursor, depth + 1, count_values)?;
            cursor = key.end;
            canonicality = canonicality.merge(key.canonicality);

            if key_class(&self.bytes[key_start..key.end]).is_none() {
                return Err(WireError::UnsupportedMapKey { offset: key_start });
            } else if self.has_prior_equal_key(
                entries_offset,
                entry_index,
                key_start,
                key.end,
                depth + 1,
            )? {
                return Err(WireError::DuplicateMapKey { offset: key_start });
            }

            let value = self.scan_value_mode(cursor, depth + 1, count_values)?;
            cursor = value.end;
            canonicality = canonicality.merge(value.canonicality);
        }
        Ok(Scanned {
            end: cursor,
            kind: MessagePackKind::Map,
            canonicality,
        })
    }

    fn has_prior_equal_key(
        &mut self,
        entries_offset: usize,
        prior_count: usize,
        current_start: usize,
        current_end: usize,
        depth: usize,
    ) -> Result<bool, WireError> {
        let mut cursor = entries_offset;
        for _ in 0..prior_count {
            let prior_start = cursor;
            let prior = self.scan_value_mode(cursor, depth, false)?;
            cursor = prior.end;
            let equal = {
                let prior_class = key_class(&self.bytes[prior_start..prior.end]);
                let current_class = key_class(&self.bytes[current_start..current_end]);
                matches!((prior_class, current_class), (Some(left), Some(right)) if left.python_eq(right))
            };
            if equal {
                return Ok(true);
            }
            cursor = self.scan_value_mode(cursor, depth, false)?.end;
        }
        Ok(false)
    }

    fn check_depth(&self, offset: usize, depth: usize) -> Result<(), WireError> {
        let effective_maximum = self
            .limits
            .max_nesting_depth
            .min(ABSOLUTE_MAX_NESTING_DEPTH);
        if depth > effective_maximum {
            Err(WireError::LimitExceeded {
                kind: LimitKind::NestingDepth,
                actual: depth,
                maximum: effective_maximum,
                offset,
            })
        } else {
            Ok(())
        }
    }

    fn check_container(&self, offset: usize, count: usize) -> Result<(), WireError> {
        if count > self.limits.max_container_items {
            Err(WireError::LimitExceeded {
                kind: LimitKind::ContainerItems,
                actual: count,
                maximum: self.limits.max_container_items,
                offset,
            })
        } else {
            Ok(())
        }
    }

    fn check_value_bytes(&self, offset: usize, length: usize) -> Result<(), WireError> {
        if length > self.limits.max_value_bytes {
            Err(WireError::LimitExceeded {
                kind: LimitKind::ValueBytes,
                actual: length,
                maximum: self.limits.max_value_bytes,
                offset,
            })
        } else {
            Ok(())
        }
    }

    fn bump_value(&mut self, offset: usize) -> Result<(), WireError> {
        self.total_values = self
            .total_values
            .checked_add(1)
            .ok_or(WireError::LengthOverflow { offset })?;
        if self.total_values > self.limits.max_total_values {
            return Err(WireError::LimitExceeded {
                kind: LimitKind::TotalValues,
                actual: self.total_values,
                maximum: self.limits.max_total_values,
                offset,
            });
        }
        Ok(())
    }

    fn bump_scan_step(&mut self, offset: usize) -> Result<(), WireError> {
        self.scan_steps = self
            .scan_steps
            .checked_add(1)
            .ok_or(WireError::LengthOverflow { offset })?;
        if self.scan_steps > self.limits.max_scan_steps {
            return Err(WireError::LimitExceeded {
                kind: LimitKind::ScanSteps,
                actual: self.scan_steps,
                maximum: self.limits.max_scan_steps,
                offset,
            });
        }
        Ok(())
    }

    fn byte(&self, offset: usize, marker_offset: usize) -> Result<u8, WireError> {
        self.bytes.get(offset).copied().ok_or(WireError::Truncated {
            offset: marker_offset,
            needed: offset.saturating_sub(marker_offset) + 1,
            available: self.bytes.len().saturating_sub(marker_offset),
        })
    }

    fn u16(&self, offset: usize, marker_offset: usize) -> Result<u16, WireError> {
        Ok(u16::from_be_bytes([
            self.byte(offset, marker_offset)?,
            self.byte(offset + 1, marker_offset)?,
        ]))
    }

    fn u32(&self, offset: usize, marker_offset: usize) -> Result<u32, WireError> {
        Ok(u32::from_be_bytes([
            self.byte(offset, marker_offset)?,
            self.byte(offset + 1, marker_offset)?,
            self.byte(offset + 2, marker_offset)?,
            self.byte(offset + 3, marker_offset)?,
        ]))
    }

    fn u64(&self, offset: usize, marker_offset: usize) -> Result<u64, WireError> {
        Ok(u64::from_be_bytes([
            self.byte(offset, marker_offset)?,
            self.byte(offset + 1, marker_offset)?,
            self.byte(offset + 2, marker_offset)?,
            self.byte(offset + 3, marker_offset)?,
            self.byte(offset + 4, marker_offset)?,
            self.byte(offset + 5, marker_offset)?,
            self.byte(offset + 6, marker_offset)?,
            self.byte(offset + 7, marker_offset)?,
        ]))
    }

    fn usize_u32(&self, offset: usize, marker_offset: usize) -> Result<usize, WireError> {
        usize::try_from(self.u32(offset, marker_offset)?).map_err(|_| WireError::LengthOverflow {
            offset: marker_offset,
        })
    }

    fn take_end(
        &self,
        offset: usize,
        length: usize,
        marker_offset: usize,
    ) -> Result<usize, WireError> {
        let end = offset
            .checked_add(length)
            .ok_or(WireError::LengthOverflow {
                offset: marker_offset,
            })?;
        if end > self.bytes.len() {
            return Err(WireError::Truncated {
                offset: marker_offset,
                needed: end.saturating_sub(marker_offset),
                available: self.bytes.len().saturating_sub(marker_offset),
            });
        }
        Ok(end)
    }
}

#[derive(Clone, Copy)]
enum KeyClass<'a> {
    Nil,
    Boolean(bool),
    Integer(i128),
    String(&'a [u8]),
    Binary(&'a [u8]),
    Extension(i8, &'a [u8]),
}

impl KeyClass<'_> {
    fn python_eq(self, other: Self) -> bool {
        match (self, other) {
            (Self::Nil, Self::Nil) => true,
            (Self::Boolean(left), Self::Boolean(right)) => left == right,
            (Self::Integer(left), Self::Integer(right)) => left == right,
            (Self::Boolean(left), Self::Integer(right))
            | (Self::Integer(right), Self::Boolean(left)) => i128::from(left) == right,
            (Self::String(left), Self::String(right))
            | (Self::Binary(left), Self::Binary(right)) => left == right,
            (Self::Extension(left_type, left), Self::Extension(right_type, right)) => {
                left_type == right_type && left == right
            }
            _ => false,
        }
    }
}

fn key_class(raw: &[u8]) -> Option<KeyClass<'_>> {
    let marker = *raw.first()?;
    match marker {
        0x00..=0x7f => Some(KeyClass::Integer(i128::from(marker))),
        0xe0..=0xff => Some(KeyClass::Integer(i128::from(marker as i8))),
        0xc0 => Some(KeyClass::Nil),
        0xc2 => Some(KeyClass::Boolean(false)),
        0xc3 => Some(KeyClass::Boolean(true)),
        0xcc => Some(KeyClass::Integer(i128::from(*raw.get(1)?))),
        0xcd => Some(KeyClass::Integer(i128::from(u16::from_be_bytes(
            raw.get(1..3)?.try_into().ok()?,
        )))),
        0xce => Some(KeyClass::Integer(i128::from(u32::from_be_bytes(
            raw.get(1..5)?.try_into().ok()?,
        )))),
        0xcf => Some(KeyClass::Integer(i128::from(u64::from_be_bytes(
            raw.get(1..9)?.try_into().ok()?,
        )))),
        0xd0 => Some(KeyClass::Integer(i128::from(*raw.get(1)? as i8))),
        0xd1 => Some(KeyClass::Integer(i128::from(i16::from_be_bytes(
            raw.get(1..3)?.try_into().ok()?,
        )))),
        0xd2 => Some(KeyClass::Integer(i128::from(i32::from_be_bytes(
            raw.get(1..5)?.try_into().ok()?,
        )))),
        0xd3 => Some(KeyClass::Integer(i128::from(i64::from_be_bytes(
            raw.get(1..9)?.try_into().ok()?,
        )))),
        0xa0..=0xbf => Some(KeyClass::String(raw.get(1..)?)),
        0xd9 => Some(KeyClass::String(raw.get(2..)?)),
        0xda => Some(KeyClass::String(raw.get(3..)?)),
        0xdb => Some(KeyClass::String(raw.get(5..)?)),
        0xc4 => Some(KeyClass::Binary(raw.get(2..)?)),
        0xc5 => Some(KeyClass::Binary(raw.get(3..)?)),
        0xc6 => Some(KeyClass::Binary(raw.get(5..)?)),
        0xd4..=0xd8 => extension_key(raw, 1),
        0xc7 => extension_key(raw, 2),
        0xc8 => extension_key(raw, 3),
        0xc9 => extension_key(raw, 5),
        _ => None,
    }
}

fn extension_key(raw: &[u8], type_offset: usize) -> Option<KeyClass<'_>> {
    let extension_type = *raw.get(type_offset)? as i8;
    if extension_type == -1 {
        return None;
    }
    Some(KeyClass::Extension(
        extension_type,
        raw.get(type_offset + 1..)?,
    ))
}
