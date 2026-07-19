use core::str;

use rmp::decode::{read_marker, Bytes, RmpRead};
use rmp::Marker;

use super::MessagePackDecodeError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MessagePackInteger {
    Negative(i64),
    Nonnegative(u64),
}

pub(crate) struct MessagePackReader<'a> {
    bytes: Bytes<'a>,
}

impl<'a> MessagePackReader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes: Bytes::new(bytes),
        }
    }

    pub(crate) fn marker(&mut self) -> Result<Marker, MessagePackDecodeError> {
        read_marker(&mut self.bytes).map_err(|_| MessagePackDecodeError)
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.bytes.remaining_slice().is_empty()
    }

    pub(crate) fn array_length(
        &mut self,
        marker: Marker,
    ) -> Result<Option<usize>, MessagePackDecodeError> {
        match marker {
            Marker::FixArray(length) => Ok(Some(usize::from(length))),
            Marker::Array16 => Ok(Some(usize::from(self.u16()?))),
            Marker::Array32 => self.length32().map(Some),
            _ => Ok(None),
        }
    }

    pub(crate) fn map_length(
        &mut self,
        marker: Marker,
    ) -> Result<Option<usize>, MessagePackDecodeError> {
        match marker {
            Marker::FixMap(length) => Ok(Some(usize::from(length))),
            Marker::Map16 => Ok(Some(usize::from(self.u16()?))),
            Marker::Map32 => self.length32().map(Some),
            _ => Ok(None),
        }
    }

    pub(crate) const fn is_string(marker: Marker) -> bool {
        matches!(
            marker,
            Marker::FixStr(_) | Marker::Str8 | Marker::Str16 | Marker::Str32
        )
    }

    pub(crate) fn string(
        &mut self,
        marker: Marker,
    ) -> Result<Option<&'a str>, MessagePackDecodeError> {
        let length = match marker {
            Marker::FixStr(length) => usize::from(length),
            Marker::Str8 => usize::from(self.u8()?),
            Marker::Str16 => usize::from(self.u16()?),
            Marker::Str32 => self.length32()?,
            _ => return Ok(None),
        };
        Ok(str::from_utf8(self.bytes(length)?).ok())
    }

    pub(crate) const fn is_binary(marker: Marker) -> bool {
        matches!(marker, Marker::Bin8 | Marker::Bin16 | Marker::Bin32)
    }

    pub(crate) fn binary(
        &mut self,
        marker: Marker,
    ) -> Result<Option<&'a [u8]>, MessagePackDecodeError> {
        let length = match marker {
            Marker::Bin8 => usize::from(self.u8()?),
            Marker::Bin16 => usize::from(self.u16()?),
            Marker::Bin32 => self.length32()?,
            _ => return Ok(None),
        };
        self.bytes(length).map(Some)
    }

    pub(crate) const fn is_integer(marker: Marker) -> bool {
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

    pub(crate) fn integer(
        &mut self,
        marker: Marker,
    ) -> Result<Option<MessagePackInteger>, MessagePackDecodeError> {
        let integer = match marker {
            Marker::FixPos(value) => MessagePackInteger::Nonnegative(u64::from(value)),
            Marker::FixNeg(value) => MessagePackInteger::Negative(i64::from(value)),
            Marker::U8 => MessagePackInteger::Nonnegative(u64::from(self.u8()?)),
            Marker::U16 => MessagePackInteger::Nonnegative(u64::from(self.u16()?)),
            Marker::U32 => MessagePackInteger::Nonnegative(u64::from(self.u32()?)),
            Marker::U64 => MessagePackInteger::Nonnegative(self.u64()?),
            Marker::I8 => signed(i64::from(self.u8()? as i8)),
            Marker::I16 => signed(i64::from(self.u16()? as i16)),
            Marker::I32 => signed(i64::from(self.u32()? as i32)),
            Marker::I64 => signed(self.u64()? as i64),
            _ => return Ok(None),
        };
        Ok(Some(integer))
    }

    pub(crate) fn float(&mut self, marker: Marker) -> Result<Option<f64>, MessagePackDecodeError> {
        match marker {
            Marker::F32 => Ok(Some(f64::from(f32::from_bits(self.u32()?)))),
            Marker::F64 => Ok(Some(f64::from_bits(self.u64()?))),
            _ => Ok(None),
        }
    }

    #[cfg(feature = "shared-instance-rpc")]
    pub(crate) fn skip_value(
        &mut self,
        marker: Marker,
        depth: usize,
        maximum_depth: usize,
    ) -> Result<(), MessagePackDecodeError> {
        if depth > maximum_depth {
            return Err(MessagePackDecodeError);
        }
        match marker {
            Marker::False | Marker::True | Marker::Null | Marker::FixPos(_) | Marker::FixNeg(_) => {
            }
            Marker::U8 | Marker::I8 => self.skip(1)?,
            Marker::U16 | Marker::I16 => self.skip(2)?,
            Marker::U32 | Marker::I32 | Marker::F32 => self.skip(4)?,
            Marker::U64 | Marker::I64 | Marker::F64 => self.skip(8)?,
            Marker::FixStr(length) => self.skip(usize::from(length))?,
            Marker::Str8 | Marker::Bin8 => {
                let length = usize::from(self.u8()?);
                self.skip(length)?;
            }
            Marker::Str16 | Marker::Bin16 => {
                let length = usize::from(self.u16()?);
                self.skip(length)?;
            }
            Marker::Str32 | Marker::Bin32 => {
                let length = self.length32()?;
                self.skip(length)?;
            }
            Marker::FixArray(length) => {
                self.skip_sequence(usize::from(length), depth, maximum_depth)?
            }
            Marker::Array16 => {
                let length = usize::from(self.u16()?);
                self.skip_sequence(length, depth, maximum_depth)?;
            }
            Marker::Array32 => {
                let length = self.length32()?;
                self.skip_sequence(length, depth, maximum_depth)?;
            }
            Marker::FixMap(length) => {
                self.skip_sequence(usize::from(length) * 2, depth, maximum_depth)?
            }
            Marker::Map16 => {
                let length = usize::from(self.u16()?)
                    .checked_mul(2)
                    .ok_or(MessagePackDecodeError)?;
                self.skip_sequence(length, depth, maximum_depth)?;
            }
            Marker::Map32 => {
                let length = self
                    .length32()?
                    .checked_mul(2)
                    .ok_or(MessagePackDecodeError)?;
                self.skip_sequence(length, depth, maximum_depth)?;
            }
            Marker::FixExt1 => self.skip(2)?,
            Marker::FixExt2 => self.skip(3)?,
            Marker::FixExt4 => self.skip(5)?,
            Marker::FixExt8 => self.skip(9)?,
            Marker::FixExt16 => self.skip(17)?,
            Marker::Ext8 => {
                let length = usize::from(self.u8()?)
                    .checked_add(1)
                    .ok_or(MessagePackDecodeError)?;
                self.skip(length)?;
            }
            Marker::Ext16 => {
                let length = usize::from(self.u16()?)
                    .checked_add(1)
                    .ok_or(MessagePackDecodeError)?;
                self.skip(length)?;
            }
            Marker::Ext32 => {
                let length = self
                    .length32()?
                    .checked_add(1)
                    .ok_or(MessagePackDecodeError)?;
                self.skip(length)?;
            }
            Marker::Reserved => return Err(MessagePackDecodeError),
        }
        Ok(())
    }

    #[cfg(feature = "shared-instance-rpc")]
    fn skip_sequence(
        &mut self,
        length: usize,
        depth: usize,
        maximum_depth: usize,
    ) -> Result<(), MessagePackDecodeError> {
        for _ in 0..length {
            let marker = self.marker()?;
            self.skip_value(marker, depth + 1, maximum_depth)?;
        }
        Ok(())
    }

    fn length32(&mut self) -> Result<usize, MessagePackDecodeError> {
        usize::try_from(self.u32()?).map_err(|_| MessagePackDecodeError)
    }

    fn bytes(&mut self, length: usize) -> Result<&'a [u8], MessagePackDecodeError> {
        let remaining = self.bytes.remaining_slice();
        let (value, after) = remaining
            .split_at_checked(length)
            .ok_or(MessagePackDecodeError)?;
        self.bytes = Bytes::new(after);
        Ok(value)
    }

    #[cfg(feature = "shared-instance-rpc")]
    fn skip(&mut self, length: usize) -> Result<(), MessagePackDecodeError> {
        self.bytes(length).map(|_| ())
    }

    fn u8(&mut self) -> Result<u8, MessagePackDecodeError> {
        self.bytes.read_u8().map_err(|_| MessagePackDecodeError)
    }

    fn u16(&mut self) -> Result<u16, MessagePackDecodeError> {
        let bytes: [u8; 2] = self
            .bytes(2)?
            .try_into()
            .map_err(|_| MessagePackDecodeError)?;
        Ok(u16::from_be_bytes(bytes))
    }

    fn u32(&mut self) -> Result<u32, MessagePackDecodeError> {
        let bytes: [u8; 4] = self
            .bytes(4)?
            .try_into()
            .map_err(|_| MessagePackDecodeError)?;
        Ok(u32::from_be_bytes(bytes))
    }

    fn u64(&mut self) -> Result<u64, MessagePackDecodeError> {
        let bytes: [u8; 8] = self
            .bytes(8)?
            .try_into()
            .map_err(|_| MessagePackDecodeError)?;
        Ok(u64::from_be_bytes(bytes))
    }
}

const fn signed(value: i64) -> MessagePackInteger {
    if value < 0 {
        MessagePackInteger::Negative(value)
    } else {
        MessagePackInteger::Nonnegative(value as u64)
    }
}
