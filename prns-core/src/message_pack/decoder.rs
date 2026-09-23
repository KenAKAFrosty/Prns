use core::str;

use super::MessagePackDecodeError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Marker {
    FixPos(u8),
    FixMap(u8),
    FixArray(u8),
    FixStr(u8),
    Null,
    Reserved,
    False,
    True,
    Bin8,
    Bin16,
    Bin32,
    Ext8,
    Ext16,
    Ext32,
    F32,
    F64,
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    FixExt1,
    FixExt2,
    FixExt4,
    FixExt8,
    FixExt16,
    Str8,
    Str16,
    Str32,
    Array16,
    Array32,
    Map16,
    Map32,
    FixNeg(i8),
}

impl Marker {
    const fn from_u8(value: u8) -> Self {
        match value {
            0x00..=0x7f => Self::FixPos(value),
            0x80..=0x8f => Self::FixMap(value & 0x0f),
            0x90..=0x9f => Self::FixArray(value & 0x0f),
            0xa0..=0xbf => Self::FixStr(value & 0x1f),
            0xc0 => Self::Null,
            0xc1 => Self::Reserved,
            0xc2 => Self::False,
            0xc3 => Self::True,
            0xc4 => Self::Bin8,
            0xc5 => Self::Bin16,
            0xc6 => Self::Bin32,
            0xc7 => Self::Ext8,
            0xc8 => Self::Ext16,
            0xc9 => Self::Ext32,
            0xca => Self::F32,
            0xcb => Self::F64,
            0xcc => Self::U8,
            0xcd => Self::U16,
            0xce => Self::U32,
            0xcf => Self::U64,
            0xd0 => Self::I8,
            0xd1 => Self::I16,
            0xd2 => Self::I32,
            0xd3 => Self::I64,
            0xd4 => Self::FixExt1,
            0xd5 => Self::FixExt2,
            0xd6 => Self::FixExt4,
            0xd7 => Self::FixExt8,
            0xd8 => Self::FixExt16,
            0xd9 => Self::Str8,
            0xda => Self::Str16,
            0xdb => Self::Str32,
            0xdc => Self::Array16,
            0xdd => Self::Array32,
            0xde => Self::Map16,
            0xdf => Self::Map32,
            0xe0..=0xff => Self::FixNeg(value as i8),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MessagePackInteger {
    Negative(i64),
    Nonnegative(u64),
}

pub(crate) struct MessagePackReader<'a> {
    bytes: &'a [u8],
}

impl<'a> MessagePackReader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    pub(crate) fn marker(&mut self) -> Result<Marker, MessagePackDecodeError> {
        self.u8().map(Marker::from_u8)
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.bytes.is_empty()
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

    #[cfg(any(
        feature = "rnx",
        feature = "shared-instance-rpc",
        feature = "signed-artifact"
    ))]
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

    #[cfg(any(
        feature = "rnx",
        feature = "shared-instance-rpc",
        feature = "signed-artifact"
    ))]
    pub(crate) fn float(&mut self, marker: Marker) -> Result<Option<f64>, MessagePackDecodeError> {
        match marker {
            Marker::F32 => Ok(Some(f64::from(f32::from_bits(self.u32()?)))),
            Marker::F64 => Ok(Some(f64::from_bits(self.u64()?))),
            _ => Ok(None),
        }
    }

    #[cfg(feature = "rns-management-wire")]
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

    #[cfg(feature = "rns-management-wire")]
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
        let (value, after) = self
            .bytes
            .split_at_checked(length)
            .ok_or(MessagePackDecodeError)?;
        self.bytes = after;
        Ok(value)
    }

    #[cfg(feature = "rns-management-wire")]
    fn skip(&mut self, length: usize) -> Result<(), MessagePackDecodeError> {
        self.bytes(length).map(|_| ())
    }

    fn u8(&mut self) -> Result<u8, MessagePackDecodeError> {
        let (&value, after) = self.bytes.split_first().ok_or(MessagePackDecodeError)?;
        self.bytes = after;
        Ok(value)
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
