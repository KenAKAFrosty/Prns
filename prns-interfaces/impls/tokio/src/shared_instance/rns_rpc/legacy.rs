use prns_core::interfaces::shared_instance::rns_rpc::{RnsRpcRequest, RpcRequest};
use prns_core::message_pack::{
    decode_owned, encode_owned, MessagePackDecodeLimits, MessagePackOwnedError, MessagePackValue,
};
use serde_pickle::{HashableValue, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LegacyCodecError {
    Pickle,
    LimitExceeded,
    UnsupportedValue,
    MessagePack,
}

pub(super) fn decode_reply(bytes: &[u8]) -> Result<Vec<u8>, LegacyCodecError> {
    pickle_to_message_pack(bytes)
}

pub(super) fn decode_request(bytes: &[u8]) -> Result<RnsRpcRequest, LegacyCodecError> {
    let message_pack = pickle_to_message_pack(bytes)?;
    match RpcRequest::decode(&message_pack).map_err(|_| LegacyCodecError::UnsupportedValue)? {
        RpcRequest::Msgpack(request) => Ok(request),
        RpcRequest::Pickle(_) => Err(LegacyCodecError::UnsupportedValue),
    }
}

pub(super) fn encode_reply(bytes: &[u8]) -> Result<Vec<u8>, LegacyCodecError> {
    let value = decode_owned(bytes, MessagePackDecodeLimits::default()).map_err(map_owned_error)?;
    let value = message_pack_to_pickle(value)?;
    serde_pickle::value_to_vec(&value, serde_pickle::SerOptions::new())
        .map_err(|_| LegacyCodecError::Pickle)
}

fn pickle_to_message_pack(bytes: &[u8]) -> Result<Vec<u8>, LegacyCodecError> {
    let value = serde_pickle::value_from_slice(bytes, serde_pickle::DeOptions::new())
        .map_err(|_| LegacyCodecError::Pickle)?;
    let limits = MessagePackDecodeLimits::default();
    let mut values = 0;
    let value = transcode_value(value, 0, &mut values, limits)?;
    encode_owned(&value).map_err(|error| match error {
        MessagePackOwnedError::LimitExceeded => LegacyCodecError::LimitExceeded,
        _ => LegacyCodecError::MessagePack,
    })
}

fn transcode_value(
    value: Value,
    depth: usize,
    values: &mut usize,
    limits: MessagePackDecodeLimits,
) -> Result<MessagePackValue, LegacyCodecError> {
    if depth > limits.maximum_depth || *values >= limits.maximum_values {
        return Err(LegacyCodecError::LimitExceeded);
    }
    *values += 1;
    match value {
        Value::None => Ok(MessagePackValue::Nil),
        Value::Bool(value) => Ok(MessagePackValue::Boolean(value)),
        Value::I64(value) => Ok(MessagePackValue::Signed(value)),
        Value::Int(value) => i64::try_from(&value)
            .map(MessagePackValue::Signed)
            .or_else(|_| u64::try_from(&value).map(MessagePackValue::Unsigned))
            .map_err(|_| LegacyCodecError::UnsupportedValue),
        Value::F64(value) => Ok(MessagePackValue::Float(value)),
        Value::Bytes(value) => {
            ensure_blob_length(value.len(), limits)?;
            Ok(MessagePackValue::Binary(value))
        }
        Value::String(value) => {
            ensure_blob_length(value.len(), limits)?;
            Ok(MessagePackValue::String(value))
        }
        Value::List(entries) | Value::Tuple(entries) => {
            ensure_container_length(entries.len(), limits)?;
            entries
                .into_iter()
                .map(|entry| transcode_value(entry, depth + 1, values, limits))
                .collect::<Result<Vec<_>, _>>()
                .map(MessagePackValue::Array)
        }
        Value::Dict(entries) => {
            ensure_container_length(entries.len(), limits)?;
            entries
                .into_iter()
                .map(|(key, value)| {
                    let key = transcode_hashable(key, depth + 1, values, limits)?;
                    let value = transcode_value(value, depth + 1, values, limits)?;
                    Ok((key, value))
                })
                .collect::<Result<Vec<_>, _>>()
                .map(MessagePackValue::Map)
        }
        Value::Set(_) | Value::FrozenSet(_) => Err(LegacyCodecError::UnsupportedValue),
    }
}

fn transcode_hashable(
    value: HashableValue,
    depth: usize,
    values: &mut usize,
    limits: MessagePackDecodeLimits,
) -> Result<MessagePackValue, LegacyCodecError> {
    transcode_value(value.into_value(), depth, values, limits)
}

fn ensure_blob_length(
    length: usize,
    limits: MessagePackDecodeLimits,
) -> Result<(), LegacyCodecError> {
    if length > limits.maximum_blob_length {
        Err(LegacyCodecError::LimitExceeded)
    } else {
        Ok(())
    }
}

fn ensure_container_length(
    length: usize,
    limits: MessagePackDecodeLimits,
) -> Result<(), LegacyCodecError> {
    if length > limits.maximum_container_length {
        Err(LegacyCodecError::LimitExceeded)
    } else {
        Ok(())
    }
}

fn message_pack_to_pickle(value: MessagePackValue) -> Result<Value, LegacyCodecError> {
    match value {
        MessagePackValue::Nil => Ok(Value::None),
        MessagePackValue::Boolean(value) => Ok(Value::Bool(value)),
        MessagePackValue::Signed(value) => Ok(Value::I64(value)),
        MessagePackValue::Unsigned(value) => Ok(match i64::try_from(value) {
            Ok(value) => Value::I64(value),
            Err(_) => Value::Int(value.into()),
        }),
        MessagePackValue::Float(value) => Ok(Value::F64(value)),
        MessagePackValue::String(value) => Ok(Value::String(value)),
        MessagePackValue::Binary(value) => Ok(Value::Bytes(value)),
        MessagePackValue::Array(entries) => entries
            .into_iter()
            .map(message_pack_to_pickle)
            .collect::<Result<Vec<_>, _>>()
            .map(Value::List),
        MessagePackValue::Map(entries) => entries
            .into_iter()
            .map(|(key, value)| {
                Ok((
                    message_pack_to_hashable(key)?,
                    message_pack_to_pickle(value)?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map(Value::Dict),
    }
}

fn message_pack_to_hashable(value: MessagePackValue) -> Result<HashableValue, LegacyCodecError> {
    match value {
        MessagePackValue::Nil => Ok(HashableValue::None),
        MessagePackValue::Boolean(value) => Ok(HashableValue::Bool(value)),
        MessagePackValue::Signed(value) => Ok(HashableValue::I64(value)),
        MessagePackValue::Unsigned(value) => Ok(match i64::try_from(value) {
            Ok(value) => HashableValue::I64(value),
            Err(_) => HashableValue::Int(value.into()),
        }),
        MessagePackValue::Float(value) => Ok(HashableValue::F64(value)),
        MessagePackValue::String(value) => Ok(HashableValue::String(value)),
        MessagePackValue::Binary(value) => Ok(HashableValue::Bytes(value)),
        MessagePackValue::Array(entries) => entries
            .into_iter()
            .map(message_pack_to_hashable)
            .collect::<Result<Vec<_>, _>>()
            .map(HashableValue::Tuple),
        MessagePackValue::Map(_) => Err(LegacyCodecError::UnsupportedValue),
    }
}

fn map_owned_error(error: MessagePackOwnedError) -> LegacyCodecError {
    match error {
        MessagePackOwnedError::LimitExceeded => LegacyCodecError::LimitExceeded,
        _ => LegacyCodecError::MessagePack,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use prns_core::identity::IdentityHash;
    use prns_core::interfaces::shared_instance::rns_rpc::{
        DestinationDataOperation, PacketHashArgument, RnsInteger, RnsNumber,
    };
    use prns_core::message_pack::{decode_owned, MessagePackDecodeLimits};
    use prns_core::wire::{DestinationHash, TransportId};
    use serde_pickle::SerOptions;

    use super::*;

    #[test]
    fn nested_python_values_transcode_to_the_existing_message_pack_model() {
        let value = Value::Dict(BTreeMap::from([
            (
                HashableValue::String(String::from("interfaces")),
                Value::List(vec![Value::Dict(BTreeMap::from([(
                    HashableValue::String(String::from("name")),
                    Value::String(String::from("TCPClientInterface[test]")),
                )]))]),
            ),
            (
                HashableValue::String(String::from("transport_id")),
                Value::Bytes(vec![0x42; 16]),
            ),
        ]));
        let pickle = serde_pickle::value_to_vec(&value, SerOptions::new()).unwrap();
        let message_pack = decode_reply(&pickle).unwrap();

        assert_eq!(
            decode_owned(&message_pack, MessagePackDecodeLimits::default()),
            Ok(MessagePackValue::Map(vec![
                (
                    MessagePackValue::String(String::from("interfaces")),
                    MessagePackValue::Array(vec![MessagePackValue::Map(vec![(
                        MessagePackValue::String(String::from("name")),
                        MessagePackValue::String(String::from("TCPClientInterface[test]")),
                    )])]),
                ),
                (
                    MessagePackValue::String(String::from("transport_id")),
                    MessagePackValue::Binary(vec![0x42; 16]),
                ),
            ]))
        );
    }

    #[test]
    fn conversion_rejects_excessive_depth_and_python_only_sets() {
        let limits = MessagePackDecodeLimits {
            maximum_depth: 1,
            ..MessagePackDecodeLimits::default()
        };
        let mut values = 0;
        assert_eq!(
            transcode_value(
                Value::List(vec![Value::List(vec![Value::None])]),
                0,
                &mut values,
                limits,
            ),
            Err(LegacyCodecError::LimitExceeded)
        );

        assert_eq!(
            transcode_value(
                Value::Set(BTreeSet::new()),
                0,
                &mut 0,
                MessagePackDecodeLimits::default(),
            ),
            Err(LegacyCodecError::UnsupportedValue)
        );
    }

    #[test]
    fn message_pack_replies_round_trip_through_protocol_three_pickle() {
        let original = MessagePackValue::Map(vec![
            (
                MessagePackValue::String(String::from("interfaces")),
                MessagePackValue::Array(vec![MessagePackValue::Map(vec![(
                    MessagePackValue::String(String::from("rxb")),
                    MessagePackValue::Unsigned(u64::MAX),
                )])]),
            ),
            (
                MessagePackValue::String(String::from("transport_id")),
                MessagePackValue::Binary(vec![0x62; 16]),
            ),
        ]);
        let message_pack = encode_owned(&original).unwrap();
        let pickle = encode_reply(&message_pack).unwrap();
        assert_eq!(&pickle[..2], &[0x80, 0x03]);
        let round_trip = decode_reply(&pickle).unwrap();

        assert_eq!(
            decode_owned(&round_trip, MessagePackDecodeLimits::default()),
            Ok(original)
        );
    }

    #[test]
    fn every_legacy_request_normalizes_to_the_same_typed_request() {
        let destination = DestinationHash::new([0x11; 16]);
        let transport = TransportId::new([0x22; 16]);
        let identity = IdentityHash::new([0x33; 16]);
        let packet_hash = || PacketHashArgument::new(vec![0x44; 32]);
        let cases = vec![
            RnsRpcRequest::InterfaceStats,
            RnsRpcRequest::PathTable { max_hops: None },
            RnsRpcRequest::PathTable {
                max_hops: Some(RnsInteger::from_u64(u64::MAX)),
            },
            RnsRpcRequest::RateTable,
            RnsRpcRequest::NextHopInterface {
                destination_hash: destination,
            },
            RnsRpcRequest::NextHop {
                destination_hash: destination,
            },
            RnsRpcRequest::FirstHopTimeout {
                destination_hash: destination,
            },
            RnsRpcRequest::LinkCount,
            RnsRpcRequest::PacketRssi {
                packet_hash: packet_hash(),
            },
            RnsRpcRequest::PacketSnr {
                packet_hash: packet_hash(),
            },
            RnsRpcRequest::PacketQuality {
                packet_hash: packet_hash(),
            },
            RnsRpcRequest::BlackholedIdentities,
            RnsRpcRequest::IsBlackholed {
                identity_hash: identity,
            },
            RnsRpcRequest::DropPath {
                destination_hash: destination,
            },
            RnsRpcRequest::DropAllVia {
                transport_id: transport,
            },
            RnsRpcRequest::DropAnnounceQueues,
            RnsRpcRequest::BlackholeIdentity {
                identity_hash: identity,
                until: Some(RnsNumber::Float(123.5)),
                reason: Some(String::from("legacy operator ☃")),
            },
            RnsRpcRequest::UnblackholeIdentity {
                identity_hash: identity,
            },
            RnsRpcRequest::DestinationData {
                operation: DestinationDataOperation::Used,
                destination_hash: destination,
            },
            RnsRpcRequest::DestinationData {
                operation: DestinationDataOperation::Retain,
                destination_hash: destination,
            },
            RnsRpcRequest::DestinationData {
                operation: DestinationDataOperation::Unretain,
                destination_hash: destination,
            },
            RnsRpcRequest::RetainIdentity {
                identity_hash: identity,
            },
        ];

        for expected in cases {
            let pickle = expected.encode_pickle().unwrap();
            assert_eq!(&pickle[..2], &[0x80, 0x03]);
            assert_eq!(decode_request(&pickle), Ok(expected));
        }
    }
}
