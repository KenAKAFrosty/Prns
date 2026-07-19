use alloc::string::String;
use alloc::vec::Vec;

use rmp::Marker;

use super::{
    MessagePackDecodeError, MessagePackEncodeError, MessagePackEncoder, MessagePackInteger,
    MessagePackReader,
};

#[derive(Debug, Clone, PartialEq)]
pub enum MessagePackValue {
    Nil,
    Boolean(bool),
    Signed(i64),
    Unsigned(u64),
    Float(f64),
    String(String),
    Binary(Vec<u8>),
    Array(Vec<Self>),
    Map(Vec<(Self, Self)>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessagePackDecodeLimits {
    pub maximum_depth: usize,
    pub maximum_values: usize,
    pub maximum_container_length: usize,
    pub maximum_blob_length: usize,
}

impl Default for MessagePackDecodeLimits {
    fn default() -> Self {
        Self {
            maximum_depth: 16,
            maximum_values: 4096,
            maximum_container_length: 4096,
            maximum_blob_length: 16 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessagePackOwnedError {
    Decode(MessagePackDecodeError),
    Encode(MessagePackEncodeError),
    LimitExceeded,
    UnsupportedMarker,
    TrailingData,
}

pub fn decode_owned(
    bytes: &[u8],
    limits: MessagePackDecodeLimits,
) -> Result<MessagePackValue, MessagePackOwnedError> {
    let mut reader = MessagePackReader::new(bytes);
    let mut values = 0;
    let value = decode_value(&mut reader, 0, &mut values, limits)?;
    if !reader.is_finished() {
        return Err(MessagePackOwnedError::TrailingData);
    }
    Ok(value)
}

pub fn encode_owned(value: &MessagePackValue) -> Result<Vec<u8>, MessagePackOwnedError> {
    let mut encoder = MessagePackEncoder::new();
    encode_value(&mut encoder, value)?;
    Ok(encoder.finish())
}

fn decode_value(
    reader: &mut MessagePackReader<'_>,
    depth: usize,
    values: &mut usize,
    limits: MessagePackDecodeLimits,
) -> Result<MessagePackValue, MessagePackOwnedError> {
    if depth > limits.maximum_depth || *values >= limits.maximum_values {
        return Err(MessagePackOwnedError::LimitExceeded);
    }
    *values += 1;
    let marker = reader.marker().map_err(MessagePackOwnedError::Decode)?;
    match marker {
        Marker::Null => Ok(MessagePackValue::Nil),
        Marker::False => Ok(MessagePackValue::Boolean(false)),
        Marker::True => Ok(MessagePackValue::Boolean(true)),
        marker if MessagePackReader::is_integer(marker) => match reader
            .integer(marker)
            .map_err(MessagePackOwnedError::Decode)?
            .ok_or(MessagePackOwnedError::UnsupportedMarker)?
        {
            MessagePackInteger::Negative(value) => Ok(MessagePackValue::Signed(value)),
            MessagePackInteger::Nonnegative(value) => Ok(MessagePackValue::Unsigned(value)),
        },
        marker if matches!(marker, Marker::F32 | Marker::F64) => reader
            .float(marker)
            .map_err(MessagePackOwnedError::Decode)?
            .map(MessagePackValue::Float)
            .ok_or(MessagePackOwnedError::UnsupportedMarker),
        marker if MessagePackReader::is_string(marker) => {
            let value = reader
                .string(marker)
                .map_err(MessagePackOwnedError::Decode)?
                .ok_or(MessagePackOwnedError::UnsupportedMarker)?;
            if value.len() > limits.maximum_blob_length {
                return Err(MessagePackOwnedError::LimitExceeded);
            }
            Ok(MessagePackValue::String(String::from(value)))
        }
        marker if MessagePackReader::is_binary(marker) => {
            let value = reader
                .binary(marker)
                .map_err(MessagePackOwnedError::Decode)?
                .ok_or(MessagePackOwnedError::UnsupportedMarker)?;
            if value.len() > limits.maximum_blob_length {
                return Err(MessagePackOwnedError::LimitExceeded);
            }
            Ok(MessagePackValue::Binary(value.to_vec()))
        }
        marker
            if matches!(
                marker,
                Marker::FixArray(_) | Marker::Array16 | Marker::Array32
            ) =>
        {
            let length = reader
                .array_length(marker)
                .map_err(MessagePackOwnedError::Decode)?
                .ok_or(MessagePackOwnedError::UnsupportedMarker)?;
            if length > limits.maximum_container_length {
                return Err(MessagePackOwnedError::LimitExceeded);
            }
            let mut array = Vec::with_capacity(length);
            for _ in 0..length {
                array.push(decode_value(reader, depth + 1, values, limits)?);
            }
            Ok(MessagePackValue::Array(array))
        }
        marker if matches!(marker, Marker::FixMap(_) | Marker::Map16 | Marker::Map32) => {
            let length = reader
                .map_length(marker)
                .map_err(MessagePackOwnedError::Decode)?
                .ok_or(MessagePackOwnedError::UnsupportedMarker)?;
            if length > limits.maximum_container_length {
                return Err(MessagePackOwnedError::LimitExceeded);
            }
            let mut map = Vec::with_capacity(length);
            for _ in 0..length {
                let key = decode_value(reader, depth + 1, values, limits)?;
                let value = decode_value(reader, depth + 1, values, limits)?;
                map.push((key, value));
            }
            Ok(MessagePackValue::Map(map))
        }
        _ => Err(MessagePackOwnedError::UnsupportedMarker),
    }
}

fn encode_value(
    encoder: &mut MessagePackEncoder,
    value: &MessagePackValue,
) -> Result<(), MessagePackOwnedError> {
    match value {
        MessagePackValue::Nil => encoder.nil(),
        MessagePackValue::Boolean(value) => encoder.boolean(*value),
        MessagePackValue::Signed(value) => encoder.signed(*value),
        MessagePackValue::Unsigned(value) => encoder.unsigned(*value),
        MessagePackValue::Float(value) => encoder.float(*value),
        MessagePackValue::String(value) => encoder
            .string(value)
            .map_err(MessagePackOwnedError::Encode)?,
        MessagePackValue::Binary(value) => encoder
            .binary(value)
            .map_err(MessagePackOwnedError::Encode)?,
        MessagePackValue::Array(values) => {
            encoder
                .array(values.len())
                .map_err(MessagePackOwnedError::Encode)?;
            for value in values {
                encode_value(encoder, value)?;
            }
        }
        MessagePackValue::Map(entries) => {
            encoder
                .map(entries.len())
                .map_err(MessagePackOwnedError::Encode)?;
            for (key, value) in entries {
                encode_value(encoder, key)?;
                encode_value(encoder, value)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_values_round_trip_nested_maps_without_reordering() {
        let value = MessagePackValue::Map(vec![
            (
                MessagePackValue::String(String::from("name")),
                MessagePackValue::String(String::from("Prns")),
            ),
            (
                MessagePackValue::String(String::from("values")),
                MessagePackValue::Array(vec![
                    MessagePackValue::Unsigned(3),
                    MessagePackValue::Boolean(true),
                    MessagePackValue::Binary(vec![1, 2, 3]),
                ]),
            ),
        ]);
        let encoded = encode_owned(&value).unwrap();
        assert_eq!(
            decode_owned(&encoded, MessagePackDecodeLimits::default()),
            Ok(value)
        );
    }

    #[test]
    fn owned_decode_enforces_depth_value_container_and_blob_limits() {
        let nested = encode_owned(&MessagePackValue::Array(vec![MessagePackValue::Array(
            vec![MessagePackValue::Unsigned(1)],
        )]))
        .unwrap();
        let limits = MessagePackDecodeLimits {
            maximum_depth: 1,
            ..MessagePackDecodeLimits::default()
        };
        assert_eq!(
            decode_owned(&nested, limits),
            Err(MessagePackOwnedError::LimitExceeded)
        );

        let values = encode_owned(&MessagePackValue::Array(vec![
            MessagePackValue::Unsigned(1),
            MessagePackValue::Unsigned(2),
        ]))
        .unwrap();
        let limits = MessagePackDecodeLimits {
            maximum_values: 2,
            ..MessagePackDecodeLimits::default()
        };
        assert_eq!(
            decode_owned(&values, limits),
            Err(MessagePackOwnedError::LimitExceeded)
        );

        let container = encode_owned(&MessagePackValue::Array(vec![
            MessagePackValue::Nil,
            MessagePackValue::Nil,
        ]))
        .unwrap();
        let limits = MessagePackDecodeLimits {
            maximum_container_length: 1,
            ..MessagePackDecodeLimits::default()
        };
        assert_eq!(
            decode_owned(&container, limits),
            Err(MessagePackOwnedError::LimitExceeded)
        );

        let blob = encode_owned(&MessagePackValue::Binary(vec![1, 2])).unwrap();
        let limits = MessagePackDecodeLimits {
            maximum_blob_length: 1,
            ..MessagePackDecodeLimits::default()
        };
        assert_eq!(
            decode_owned(&blob, limits),
            Err(MessagePackOwnedError::LimitExceeded)
        );
    }
}
