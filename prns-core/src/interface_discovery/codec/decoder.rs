use alloc::string::String;

use rmp::Marker;

use super::{DiscoveryDecodeError, DiscoveryField};
use crate::wire::TransportId;

const VALUE_MAX_DEPTH: usize = 8;

pub(super) struct Decoder<'a> {
    remaining: &'a [u8],
}

impl<'a> Decoder<'a> {
    pub(super) const fn new(bytes: &'a [u8]) -> Self {
        Self { remaining: bytes }
    }

    pub(super) const fn is_empty(&self) -> bool {
        self.remaining.is_empty()
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], DiscoveryDecodeError> {
        let (value, remaining) = self
            .remaining
            .split_at_checked(length)
            .ok_or(DiscoveryDecodeError::MessagePack)?;
        self.remaining = remaining;
        Ok(value)
    }

    fn byte(&mut self) -> Result<u8, DiscoveryDecodeError> {
        Ok(self.take(1)?[0])
    }

    fn marker(&mut self) -> Result<Marker, DiscoveryDecodeError> {
        Ok(Marker::from_u8(self.byte()?))
    }

    fn raw_u16(&mut self) -> Result<u16, DiscoveryDecodeError> {
        Ok(u16::from_be_bytes(
            self.take(2)?
                .try_into()
                .map_err(|_| DiscoveryDecodeError::MessagePack)?,
        ))
    }

    fn raw_u32(&mut self) -> Result<u32, DiscoveryDecodeError> {
        Ok(u32::from_be_bytes(
            self.take(4)?
                .try_into()
                .map_err(|_| DiscoveryDecodeError::MessagePack)?,
        ))
    }

    fn raw_u64(&mut self) -> Result<u64, DiscoveryDecodeError> {
        Ok(u64::from_be_bytes(
            self.take(8)?
                .try_into()
                .map_err(|_| DiscoveryDecodeError::MessagePack)?,
        ))
    }

    fn raw_i16(&mut self) -> Result<i16, DiscoveryDecodeError> {
        Ok(i16::from_be_bytes(
            self.take(2)?
                .try_into()
                .map_err(|_| DiscoveryDecodeError::MessagePack)?,
        ))
    }

    fn raw_i32(&mut self) -> Result<i32, DiscoveryDecodeError> {
        Ok(i32::from_be_bytes(
            self.take(4)?
                .try_into()
                .map_err(|_| DiscoveryDecodeError::MessagePack)?,
        ))
    }

    fn raw_i64(&mut self) -> Result<i64, DiscoveryDecodeError> {
        Ok(i64::from_be_bytes(
            self.take(8)?
                .try_into()
                .map_err(|_| DiscoveryDecodeError::MessagePack)?,
        ))
    }

    pub(super) fn map_len(&mut self) -> Result<usize, DiscoveryDecodeError> {
        match self.marker()? {
            Marker::FixMap(length) => Ok(usize::from(length)),
            Marker::Map16 => Ok(usize::from(self.raw_u16()?)),
            Marker::Map32 => {
                usize::try_from(self.raw_u32()?).map_err(|_| DiscoveryDecodeError::MessagePack)
            }
            _ => Err(DiscoveryDecodeError::ExpectedMap),
        }
    }

    pub(super) fn map_key(&mut self) -> Result<Option<u64>, DiscoveryDecodeError> {
        let marker = self.marker()?;
        if let Some(value) = self.unsigned_after_marker(marker)? {
            Ok(Some(value))
        } else if is_integer_marker(marker) {
            Ok(None)
        } else {
            self.skip_after_marker(marker, 0)?;
            Ok(None)
        }
    }

    pub(super) fn unsigned(&mut self, field: DiscoveryField) -> Result<u64, DiscoveryDecodeError> {
        let marker = self.marker()?;
        self.unsigned_after_marker(marker)?
            .ok_or(DiscoveryDecodeError::InvalidField(field))
    }

    fn unsigned_after_marker(
        &mut self,
        marker: Marker,
    ) -> Result<Option<u64>, DiscoveryDecodeError> {
        let value = match marker {
            Marker::FixPos(value) => Some(u64::from(value)),
            Marker::U8 => Some(u64::from(self.byte()?)),
            Marker::U16 => Some(u64::from(self.raw_u16()?)),
            Marker::U32 => Some(u64::from(self.raw_u32()?)),
            Marker::U64 => Some(self.raw_u64()?),
            Marker::FixNeg(_) => None,
            Marker::I8 => u64::try_from(self.byte()? as i8).ok(),
            Marker::I16 => u64::try_from(self.raw_i16()?).ok(),
            Marker::I32 => u64::try_from(self.raw_i32()?).ok(),
            Marker::I64 => u64::try_from(self.raw_i64()?).ok(),
            _ => return Ok(None),
        };
        Ok(value)
    }

    pub(super) fn boolean(&mut self, field: DiscoveryField) -> Result<bool, DiscoveryDecodeError> {
        match self.marker()? {
            Marker::True => Ok(true),
            Marker::False => Ok(false),
            _ => Err(DiscoveryDecodeError::InvalidField(field)),
        }
    }

    pub(super) fn string(&mut self, field: DiscoveryField) -> Result<String, DiscoveryDecodeError> {
        let marker = self.marker()?;
        self.string_after_marker(marker, field)
    }

    fn string_after_marker(
        &mut self,
        marker: Marker,
        field: DiscoveryField,
    ) -> Result<String, DiscoveryDecodeError> {
        let length = match marker {
            Marker::FixStr(length) => usize::from(length),
            Marker::Str8 => usize::from(self.byte()?),
            Marker::Str16 => usize::from(self.raw_u16()?),
            Marker::Str32 => {
                usize::try_from(self.raw_u32()?).map_err(|_| DiscoveryDecodeError::MessagePack)?
            }
            _ => return Err(DiscoveryDecodeError::InvalidField(field)),
        };
        let value = core::str::from_utf8(self.take(length)?)
            .map_err(|_| DiscoveryDecodeError::InvalidField(field))?;
        Ok(String::from(value))
    }

    pub(super) fn optional_string(
        &mut self,
        field: DiscoveryField,
    ) -> Result<Option<String>, DiscoveryDecodeError> {
        let marker = self.marker()?;
        if marker == Marker::Null {
            Ok(None)
        } else {
            self.string_after_marker(marker, field).map(Some)
        }
    }

    pub(super) fn optional_float(
        &mut self,
        field: DiscoveryField,
    ) -> Result<Option<f64>, DiscoveryDecodeError> {
        match self.marker()? {
            Marker::Null => Ok(None),
            Marker::F32 => Ok(Some(f64::from(f32::from_bits(self.raw_u32()?)))),
            Marker::F64 => Ok(Some(f64::from_bits(self.raw_u64()?))),
            _ => Err(DiscoveryDecodeError::InvalidField(field)),
        }
    }

    pub(super) fn transport_id(&mut self) -> Result<TransportId, DiscoveryDecodeError> {
        let marker = self.marker()?;
        let length = match marker {
            Marker::Bin8 => usize::from(self.byte()?),
            Marker::Bin16 => usize::from(self.raw_u16()?),
            Marker::Bin32 => {
                usize::try_from(self.raw_u32()?).map_err(|_| DiscoveryDecodeError::MessagePack)?
            }
            _ => {
                return Err(DiscoveryDecodeError::InvalidField(
                    DiscoveryField::TransportId,
                ));
            }
        };
        TransportId::from_slice(self.take(length)?).ok_or(DiscoveryDecodeError::InvalidField(
            DiscoveryField::TransportId,
        ))
    }

    pub(super) fn skip_value(&mut self, depth: usize) -> Result<(), DiscoveryDecodeError> {
        let marker = self.marker()?;
        self.skip_after_marker(marker, depth)
    }

    fn skip_after_marker(
        &mut self,
        marker: Marker,
        depth: usize,
    ) -> Result<(), DiscoveryDecodeError> {
        match marker {
            Marker::FixPos(_) | Marker::FixNeg(_) | Marker::Null | Marker::False | Marker::True => {
                Ok(())
            }
            Marker::Reserved => Err(DiscoveryDecodeError::MessagePack),
            Marker::U8 | Marker::I8 => self.take(1).map(|_| ()),
            Marker::U16 | Marker::I16 => self.take(2).map(|_| ()),
            Marker::U32 | Marker::I32 | Marker::F32 => self.take(4).map(|_| ()),
            Marker::U64 | Marker::I64 | Marker::F64 => self.take(8).map(|_| ()),
            Marker::FixStr(length) => self.take(usize::from(length)).map(|_| ()),
            Marker::Str8 | Marker::Bin8 => {
                let length = usize::from(self.byte()?);
                self.take(length).map(|_| ())
            }
            Marker::Str16 | Marker::Bin16 => {
                let length = usize::from(self.raw_u16()?);
                self.take(length).map(|_| ())
            }
            Marker::Str32 | Marker::Bin32 => {
                let length = usize::try_from(self.raw_u32()?)
                    .map_err(|_| DiscoveryDecodeError::MessagePack)?;
                self.take(length).map(|_| ())
            }
            Marker::FixArray(length) => self.skip_sequence(usize::from(length), depth),
            Marker::Array16 => {
                let length = usize::from(self.raw_u16()?);
                self.skip_sequence(length, depth)
            }
            Marker::Array32 => {
                let length = usize::try_from(self.raw_u32()?)
                    .map_err(|_| DiscoveryDecodeError::MessagePack)?;
                self.skip_sequence(length, depth)
            }
            Marker::FixMap(length) => self.skip_map(usize::from(length), depth),
            Marker::Map16 => {
                let length = usize::from(self.raw_u16()?);
                self.skip_map(length, depth)
            }
            Marker::Map32 => {
                let length = usize::try_from(self.raw_u32()?)
                    .map_err(|_| DiscoveryDecodeError::MessagePack)?;
                self.skip_map(length, depth)
            }
            Marker::FixExt1 => self.skip_ext(1),
            Marker::FixExt2 => self.skip_ext(2),
            Marker::FixExt4 => self.skip_ext(4),
            Marker::FixExt8 => self.skip_ext(8),
            Marker::FixExt16 => self.skip_ext(16),
            Marker::Ext8 => {
                let length = usize::from(self.byte()?);
                self.skip_ext(length)
            }
            Marker::Ext16 => {
                let length = usize::from(self.raw_u16()?);
                self.skip_ext(length)
            }
            Marker::Ext32 => {
                let length = usize::try_from(self.raw_u32()?)
                    .map_err(|_| DiscoveryDecodeError::MessagePack)?;
                self.skip_ext(length)
            }
        }
    }

    fn skip_sequence(&mut self, length: usize, depth: usize) -> Result<(), DiscoveryDecodeError> {
        if depth >= VALUE_MAX_DEPTH {
            return Err(DiscoveryDecodeError::MessagePack);
        }
        for _ in 0..length {
            self.skip_value(depth + 1)?;
        }
        Ok(())
    }

    fn skip_map(&mut self, length: usize, depth: usize) -> Result<(), DiscoveryDecodeError> {
        if depth >= VALUE_MAX_DEPTH {
            return Err(DiscoveryDecodeError::MessagePack);
        }
        for _ in 0..length {
            self.skip_value(depth + 1)?;
            self.skip_value(depth + 1)?;
        }
        Ok(())
    }

    fn skip_ext(&mut self, length: usize) -> Result<(), DiscoveryDecodeError> {
        let total = length
            .checked_add(1)
            .ok_or(DiscoveryDecodeError::MessagePack)?;
        self.take(total).map(|_| ())
    }
}

const fn is_integer_marker(marker: Marker) -> bool {
    matches!(
        marker,
        Marker::FixPos(_)
            | Marker::FixNeg(_)
            | Marker::U8
            | Marker::U16
            | Marker::U32
            | Marker::U64
            | Marker::I8
            | Marker::I16
            | Marker::I32
            | Marker::I64
    )
}
