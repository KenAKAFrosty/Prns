use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use rmp::decode::{read_marker, Bytes, RmpRead};
use rmp::Marker;

use super::super::RnsInteger;

#[derive(Debug, Clone, PartialEq)]
pub enum RnsRpcScalarReply {
    Null,
    Boolean(bool),
    Integer(RnsInteger),
    Float(f64),
    Binary(Vec<u8>),
    String(String),
}

impl RnsRpcScalarReply {
    pub fn decode_message_pack(bytes: &[u8]) -> Result<Self, RnsRpcScalarReplyDecodeError> {
        let mut reader = Bytes::new(bytes);
        let marker = read_marker(&mut reader)
            .map_err(|_| RnsRpcScalarReplyDecodeError::InvalidMessagePack)?;
        let reply = decode_value(&mut reader, marker)?;
        if !reader.remaining_slice().is_empty() {
            return Err(RnsRpcScalarReplyDecodeError::TrailingData);
        }
        Ok(reply)
    }

    pub const fn nonnegative_integer(&self) -> Option<u64> {
        match self {
            Self::Integer(value) => value.nonnegative_value(),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RnsRpcScalarReplyDecodeError {
    InvalidMessagePack,
    UnsupportedShape,
    InvalidUtf8,
    AllocationFailed { bytes: usize },
    TrailingData,
}

impl fmt::Display for RnsRpcScalarReplyDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMessagePack => formatter.write_str("invalid MessagePack RPC reply"),
            Self::UnsupportedShape => formatter.write_str("RPC reply is not a scalar value"),
            Self::InvalidUtf8 => formatter.write_str("RPC string reply is not valid UTF-8"),
            Self::AllocationFailed { bytes } => {
                write!(formatter, "could not allocate {bytes} bytes for RPC reply")
            }
            Self::TrailingData => formatter.write_str("RPC reply has trailing MessagePack data"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for RnsRpcScalarReplyDecodeError {}

fn decode_value(
    reader: &mut Bytes<'_>,
    marker: Marker,
) -> Result<RnsRpcScalarReply, RnsRpcScalarReplyDecodeError> {
    match marker {
        Marker::Null => Ok(RnsRpcScalarReply::Null),
        Marker::False => Ok(RnsRpcScalarReply::Boolean(false)),
        Marker::True => Ok(RnsRpcScalarReply::Boolean(true)),
        Marker::FixPos(value) => Ok(integer_u64(u64::from(value))),
        Marker::FixNeg(value) => Ok(integer_i64(i64::from(value))),
        Marker::U8 => read_u8(reader).map(|value| integer_u64(u64::from(value))),
        Marker::U16 => read_u16(reader).map(|value| integer_u64(u64::from(value))),
        Marker::U32 => read_u32(reader).map(|value| integer_u64(u64::from(value))),
        Marker::U64 => read_u64(reader).map(integer_u64),
        Marker::I8 => read_u8(reader).map(|value| integer_i64(i64::from(value as i8))),
        Marker::I16 => read_u16(reader).map(|value| integer_i64(i64::from(value as i16))),
        Marker::I32 => read_u32(reader).map(|value| integer_i64(i64::from(value as i32))),
        Marker::I64 => read_u64(reader).map(|value| integer_i64(value as i64)),
        Marker::F32 => {
            read_u32(reader).map(|value| RnsRpcScalarReply::Float(f64::from(f32::from_bits(value))))
        }
        Marker::F64 => {
            read_u64(reader).map(|value| RnsRpcScalarReply::Float(f64::from_bits(value)))
        }
        Marker::FixStr(length) => decode_string(reader, usize::from(length)),
        Marker::Str8 => {
            let length = usize::from(read_u8(reader)?);
            decode_string(reader, length)
        }
        Marker::Str16 => {
            let length = usize::from(read_u16(reader)?);
            decode_string(reader, length)
        }
        Marker::Str32 => {
            let length = length_u32(reader)?;
            decode_string(reader, length)
        }
        Marker::Bin8 => {
            let length = usize::from(read_u8(reader)?);
            read_bytes(reader, length).map(RnsRpcScalarReply::Binary)
        }
        Marker::Bin16 => {
            let length = usize::from(read_u16(reader)?);
            read_bytes(reader, length).map(RnsRpcScalarReply::Binary)
        }
        Marker::Bin32 => {
            let length = length_u32(reader)?;
            read_bytes(reader, length).map(RnsRpcScalarReply::Binary)
        }
        _ => Err(RnsRpcScalarReplyDecodeError::UnsupportedShape),
    }
}

fn integer_i64(value: i64) -> RnsRpcScalarReply {
    RnsRpcScalarReply::Integer(RnsInteger::from_i64(value))
}

fn integer_u64(value: u64) -> RnsRpcScalarReply {
    RnsRpcScalarReply::Integer(RnsInteger::from_u64(value))
}

fn decode_string(
    reader: &mut Bytes<'_>,
    length: usize,
) -> Result<RnsRpcScalarReply, RnsRpcScalarReplyDecodeError> {
    let bytes = read_bytes(reader, length)?;
    String::from_utf8(bytes)
        .map(RnsRpcScalarReply::String)
        .map_err(|_| RnsRpcScalarReplyDecodeError::InvalidUtf8)
}

fn read_bytes(
    reader: &mut Bytes<'_>,
    length: usize,
) -> Result<Vec<u8>, RnsRpcScalarReplyDecodeError> {
    if reader.remaining_slice().len() < length {
        return Err(RnsRpcScalarReplyDecodeError::InvalidMessagePack);
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| RnsRpcScalarReplyDecodeError::AllocationFailed { bytes: length })?;
    bytes.resize(length, 0);
    reader
        .read_exact_buf(&mut bytes)
        .map_err(|_| RnsRpcScalarReplyDecodeError::InvalidMessagePack)?;
    Ok(bytes)
}

fn length_u32(reader: &mut Bytes<'_>) -> Result<usize, RnsRpcScalarReplyDecodeError> {
    usize::try_from(read_u32(reader)?).map_err(|_| RnsRpcScalarReplyDecodeError::InvalidMessagePack)
}

fn read_u8(reader: &mut Bytes<'_>) -> Result<u8, RnsRpcScalarReplyDecodeError> {
    reader
        .read_u8()
        .map_err(|_| RnsRpcScalarReplyDecodeError::InvalidMessagePack)
}

fn read_u16(reader: &mut Bytes<'_>) -> Result<u16, RnsRpcScalarReplyDecodeError> {
    let mut bytes = [0u8; 2];
    reader
        .read_exact_buf(&mut bytes)
        .map_err(|_| RnsRpcScalarReplyDecodeError::InvalidMessagePack)?;
    Ok(u16::from_be_bytes(bytes))
}

fn read_u32(reader: &mut Bytes<'_>) -> Result<u32, RnsRpcScalarReplyDecodeError> {
    let mut bytes = [0u8; 4];
    reader
        .read_exact_buf(&mut bytes)
        .map_err(|_| RnsRpcScalarReplyDecodeError::InvalidMessagePack)?;
    Ok(u32::from_be_bytes(bytes))
}

fn read_u64(reader: &mut Bytes<'_>) -> Result<u64, RnsRpcScalarReplyDecodeError> {
    let mut bytes = [0u8; 8];
    reader
        .read_exact_buf(&mut bytes)
        .map_err(|_| RnsRpcScalarReplyDecodeError::InvalidMessagePack)?;
    Ok(u64::from_be_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;

    #[test]
    fn decodes_every_supported_scalar_shape() {
        assert_eq!(
            RnsRpcScalarReply::decode_message_pack(&[0xc0]),
            Ok(RnsRpcScalarReply::Null)
        );
        assert_eq!(
            RnsRpcScalarReply::decode_message_pack(&[0xc3]),
            Ok(RnsRpcScalarReply::Boolean(true))
        );
        assert_eq!(
            RnsRpcScalarReply::decode_message_pack(&[0xff]),
            Ok(integer_i64(-1))
        );
        assert_eq!(
            RnsRpcScalarReply::decode_message_pack(&[0xcc, 0xff]),
            Ok(integer_u64(255))
        );
        assert_eq!(
            RnsRpcScalarReply::decode_message_pack(&[0xa2, b'o', b'k']),
            Ok(RnsRpcScalarReply::String(String::from("ok")))
        );
        assert_eq!(
            RnsRpcScalarReply::decode_message_pack(&[0xc4, 0x02, 0x01, 0x02]),
            Ok(RnsRpcScalarReply::Binary(vec![1, 2]))
        );
    }

    #[test]
    fn rejects_collections_and_trailing_data() {
        assert_eq!(
            RnsRpcScalarReply::decode_message_pack(&[0x90]),
            Err(RnsRpcScalarReplyDecodeError::UnsupportedShape)
        );
        assert_eq!(
            RnsRpcScalarReply::decode_message_pack(&[0x01, 0x02]),
            Err(RnsRpcScalarReplyDecodeError::TrailingData)
        );
    }
}
