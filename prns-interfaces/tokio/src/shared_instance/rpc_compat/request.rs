use std::io::Cursor;
use std::string::String;
use std::vec::Vec;

use prns_core::identity::IdentityHash;
use prns_core::routing::BlackholeExpiry;
use prns_core::units::InstantMillis;
use prns_core::wire::{DestinationHash, TransportId};
use rmpv::Value;

const REQUEST_MAX_DEPTH: usize = 8;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum RnsInteger {
    Negative(i64),
    Nonnegative(u64),
}

#[derive(Debug, PartialEq)]
pub(super) enum RnsNumber {
    Integer(RnsInteger),
    Float(f64),
}

impl RnsNumber {
    pub(super) fn blackhole_expiry(&self) -> BlackholeExpiry {
        match self {
            Self::Integer(RnsInteger::Negative(_)) => BlackholeExpiry::At(InstantMillis(0)),
            Self::Integer(RnsInteger::Nonnegative(0)) => BlackholeExpiry::Indefinite,
            Self::Integer(RnsInteger::Nonnegative(seconds)) => {
                BlackholeExpiry::At(InstantMillis(seconds.saturating_mul(1_000)))
            }
            Self::Float(seconds) if *seconds == 0.0 || seconds.is_nan() => {
                BlackholeExpiry::Indefinite
            }
            Self::Float(seconds) if *seconds < 0.0 => BlackholeExpiry::At(InstantMillis(0)),
            Self::Float(seconds) => {
                let millis = *seconds * 1_000.0;
                let deadline = if !millis.is_finite() || millis >= u64::MAX as f64 {
                    u64::MAX
                } else {
                    millis.floor() as u64
                };
                BlackholeExpiry::At(InstantMillis(deadline))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DestinationDataOperation {
    Used,
    Retain,
    Unretain,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct PacketHashArgument(Vec<u8>);

impl PacketHashArgument {
    pub(super) fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Debug, PartialEq)]
pub(super) enum RnsRpcRequest {
    InterfaceStats,
    PathTable {
        max_hops: Option<RnsInteger>,
    },
    RateTable,
    NextHopInterface {
        destination_hash: DestinationHash,
    },
    NextHop {
        destination_hash: DestinationHash,
    },
    FirstHopTimeout {
        destination_hash: DestinationHash,
    },
    LinkCount,
    PacketRssi {
        packet_hash: PacketHashArgument,
    },
    PacketSnr {
        packet_hash: PacketHashArgument,
    },
    PacketQuality {
        packet_hash: PacketHashArgument,
    },
    BlackholedIdentities,
    IsBlackholed {
        identity_hash: IdentityHash,
    },
    DropPath {
        destination_hash: DestinationHash,
    },
    DropAllVia {
        transport_id: TransportId,
    },
    DropAnnounceQueues,
    BlackholeIdentity {
        identity_hash: IdentityHash,
        until: Option<RnsNumber>,
        reason: Option<String>,
    },
    UnblackholeIdentity {
        identity_hash: IdentityHash,
    },
    DestinationData {
        operation: DestinationDataOperation,
        destination_hash: DestinationHash,
    },
    RetainIdentity {
        identity_hash: IdentityHash,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum DecodeError {
    MessagePack,
    TrailingData,
    ExpectedMap,
    ExpectedStringKey,
    DuplicateField(String),
    UnknownField(String),
    MissingOperation,
    ContradictoryOperation,
    MissingField(&'static str),
    UnexpectedField(&'static str),
    InvalidFieldType(&'static str),
    InvalidHashLength {
        field: &'static str,
        expected: usize,
        actual: usize,
    },
    UnknownOperation {
        selector: &'static str,
        operation: String,
    },
}

#[derive(Default)]
struct Fields {
    get: Option<Value>,
    drop: Option<Value>,
    blackhole_identity: Option<Value>,
    unblackhole_identity: Option<Value>,
    destination_data: Option<Value>,
    identity_data: Option<Value>,
    max_hops: Option<Value>,
    destination_hash: Option<Value>,
    packet_hash: Option<Value>,
    identity_hash: Option<Value>,
    until: Option<Value>,
    reason: Option<Value>,
}

impl TryFrom<Value> for Fields {
    type Error = DecodeError;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        let Value::Map(entries) = value else {
            return Err(DecodeError::ExpectedMap);
        };
        let mut fields = Self::default();
        for (key, value) in entries {
            let Some(key) = key.as_str() else {
                return Err(DecodeError::ExpectedStringKey);
            };
            let slot = match key {
                "get" => &mut fields.get,
                "drop" => &mut fields.drop,
                "blackhole_identity" => &mut fields.blackhole_identity,
                "unblackhole_identity" => &mut fields.unblackhole_identity,
                "destination_data" => &mut fields.destination_data,
                "identity_data" => &mut fields.identity_data,
                "max_hops" => &mut fields.max_hops,
                "destination_hash" => &mut fields.destination_hash,
                "packet_hash" => &mut fields.packet_hash,
                "identity_hash" => &mut fields.identity_hash,
                "until" => &mut fields.until,
                "reason" => &mut fields.reason,
                _ => return Err(DecodeError::UnknownField(key.into())),
            };
            if slot.replace(value).is_some() {
                return Err(DecodeError::DuplicateField(key.into()));
            }
        }
        Ok(fields)
    }
}

impl Fields {
    fn operation_count(&self) -> usize {
        [
            self.get.is_some(),
            self.drop.is_some(),
            self.blackhole_identity.is_some(),
            self.unblackhole_identity.is_some(),
            self.destination_data.is_some(),
            self.identity_data.is_some(),
        ]
        .into_iter()
        .filter(|present| *present)
        .count()
    }

    fn ensure_only(&self, allowed: &[&str]) -> Result<(), DecodeError> {
        for (name, present) in [
            ("get", self.get.is_some()),
            ("drop", self.drop.is_some()),
            ("blackhole_identity", self.blackhole_identity.is_some()),
            ("unblackhole_identity", self.unblackhole_identity.is_some()),
            ("destination_data", self.destination_data.is_some()),
            ("identity_data", self.identity_data.is_some()),
            ("max_hops", self.max_hops.is_some()),
            ("destination_hash", self.destination_hash.is_some()),
            ("packet_hash", self.packet_hash.is_some()),
            ("identity_hash", self.identity_hash.is_some()),
            ("until", self.until.is_some()),
            ("reason", self.reason.is_some()),
        ] {
            if present && !allowed.contains(&name) {
                return Err(DecodeError::UnexpectedField(name));
            }
        }
        Ok(())
    }
}

pub(super) fn decode(bytes: &[u8]) -> Result<RnsRpcRequest, DecodeError> {
    let mut cursor = Cursor::new(bytes);
    let value = rmpv::decode::read_value_with_max_depth(&mut cursor, REQUEST_MAX_DEPTH)
        .map_err(|_| DecodeError::MessagePack)?;
    if cursor.position() != bytes.len() as u64 {
        return Err(DecodeError::TrailingData);
    }
    decode_fields(Fields::try_from(value)?)
}

fn decode_fields(fields: Fields) -> Result<RnsRpcRequest, DecodeError> {
    match fields.operation_count() {
        0 => Err(DecodeError::MissingOperation),
        2.. => Err(DecodeError::ContradictoryOperation),
        _ if fields.get.is_some() => decode_get(fields),
        _ if fields.drop.is_some() => decode_drop(fields),
        _ if fields.blackhole_identity.is_some() => decode_blackhole(fields),
        _ if fields.unblackhole_identity.is_some() => decode_unblackhole(fields),
        _ if fields.destination_data.is_some() => decode_destination_data(fields),
        _ => decode_identity_data(fields),
    }
}

fn decode_get(mut fields: Fields) -> Result<RnsRpcRequest, DecodeError> {
    let operation = take_string(&mut fields.get, "get")?;
    match operation.as_str() {
        "interface_stats" => {
            fields.ensure_only(&["get"])?;
            Ok(RnsRpcRequest::InterfaceStats)
        }
        "path_table" => {
            fields.ensure_only(&["get", "max_hops"])?;
            let max_hops = take_optional_integer(&mut fields.max_hops, "max_hops")?;
            Ok(RnsRpcRequest::PathTable { max_hops })
        }
        "rate_table" => {
            fields.ensure_only(&["get"])?;
            Ok(RnsRpcRequest::RateTable)
        }
        "next_hop_if_name" => {
            fields.ensure_only(&["get", "destination_hash"])?;
            Ok(RnsRpcRequest::NextHopInterface {
                destination_hash: take_destination_hash(&mut fields.destination_hash)?,
            })
        }
        "next_hop" => {
            fields.ensure_only(&["get", "destination_hash"])?;
            Ok(RnsRpcRequest::NextHop {
                destination_hash: take_destination_hash(&mut fields.destination_hash)?,
            })
        }
        "first_hop_timeout" => {
            fields.ensure_only(&["get", "destination_hash"])?;
            Ok(RnsRpcRequest::FirstHopTimeout {
                destination_hash: take_destination_hash(&mut fields.destination_hash)?,
            })
        }
        "link_count" => {
            fields.ensure_only(&["get"])?;
            Ok(RnsRpcRequest::LinkCount)
        }
        "packet_rssi" => {
            fields.ensure_only(&["get", "packet_hash"])?;
            Ok(RnsRpcRequest::PacketRssi {
                packet_hash: take_packet_hash(&mut fields.packet_hash)?,
            })
        }
        "packet_snr" => {
            fields.ensure_only(&["get", "packet_hash"])?;
            Ok(RnsRpcRequest::PacketSnr {
                packet_hash: take_packet_hash(&mut fields.packet_hash)?,
            })
        }
        "packet_q" => {
            fields.ensure_only(&["get", "packet_hash"])?;
            Ok(RnsRpcRequest::PacketQuality {
                packet_hash: take_packet_hash(&mut fields.packet_hash)?,
            })
        }
        "blackholed_identities" => {
            fields.ensure_only(&["get"])?;
            Ok(RnsRpcRequest::BlackholedIdentities)
        }
        "is_blackholed" => {
            fields.ensure_only(&["get", "identity_hash"])?;
            Ok(RnsRpcRequest::IsBlackholed {
                identity_hash: take_identity_hash(&mut fields.identity_hash)?,
            })
        }
        _ => Err(DecodeError::UnknownOperation {
            selector: "get",
            operation,
        }),
    }
}

fn decode_drop(mut fields: Fields) -> Result<RnsRpcRequest, DecodeError> {
    let operation = take_string(&mut fields.drop, "drop")?;
    match operation.as_str() {
        "path" => {
            fields.ensure_only(&["drop", "destination_hash"])?;
            Ok(RnsRpcRequest::DropPath {
                destination_hash: take_destination_hash(&mut fields.destination_hash)?,
            })
        }
        "all_via" => {
            fields.ensure_only(&["drop", "destination_hash"])?;
            Ok(RnsRpcRequest::DropAllVia {
                transport_id: take_transport_id(&mut fields.destination_hash)?,
            })
        }
        "announce_queues" => {
            fields.ensure_only(&["drop"])?;
            Ok(RnsRpcRequest::DropAnnounceQueues)
        }
        _ => Err(DecodeError::UnknownOperation {
            selector: "drop",
            operation,
        }),
    }
}

fn decode_blackhole(mut fields: Fields) -> Result<RnsRpcRequest, DecodeError> {
    fields.ensure_only(&["blackhole_identity", "until", "reason"])?;
    Ok(RnsRpcRequest::BlackholeIdentity {
        identity_hash: take_identity_hash(&mut fields.blackhole_identity)?,
        until: take_optional_number(&mut fields.until, "until")?,
        reason: take_optional_string(&mut fields.reason, "reason")?,
    })
}

fn decode_unblackhole(mut fields: Fields) -> Result<RnsRpcRequest, DecodeError> {
    fields.ensure_only(&["unblackhole_identity"])?;
    Ok(RnsRpcRequest::UnblackholeIdentity {
        identity_hash: take_identity_hash(&mut fields.unblackhole_identity)?,
    })
}

fn decode_destination_data(mut fields: Fields) -> Result<RnsRpcRequest, DecodeError> {
    fields.ensure_only(&["destination_data", "destination_hash"])?;
    let operation = match take_string(&mut fields.destination_data, "destination_data")?.as_str() {
        "used" => DestinationDataOperation::Used,
        "retain" => DestinationDataOperation::Retain,
        "unretain" => DestinationDataOperation::Unretain,
        operation => {
            return Err(DecodeError::UnknownOperation {
                selector: "destination_data",
                operation: operation.into(),
            });
        }
    };
    Ok(RnsRpcRequest::DestinationData {
        operation,
        destination_hash: take_destination_hash(&mut fields.destination_hash)?,
    })
}

fn decode_identity_data(mut fields: Fields) -> Result<RnsRpcRequest, DecodeError> {
    fields.ensure_only(&["identity_data", "identity_hash"])?;
    let operation = take_string(&mut fields.identity_data, "identity_data")?;
    if operation != "retain" {
        return Err(DecodeError::UnknownOperation {
            selector: "identity_data",
            operation,
        });
    }
    Ok(RnsRpcRequest::RetainIdentity {
        identity_hash: take_identity_hash(&mut fields.identity_hash)?,
    })
}

fn take_required(slot: &mut Option<Value>, field: &'static str) -> Result<Value, DecodeError> {
    slot.take().ok_or(DecodeError::MissingField(field))
}

fn take_string(slot: &mut Option<Value>, field: &'static str) -> Result<String, DecodeError> {
    match take_required(slot, field)? {
        Value::String(value) => value.into_str().ok_or(DecodeError::InvalidFieldType(field)),
        _ => Err(DecodeError::InvalidFieldType(field)),
    }
}

fn take_optional_string(
    slot: &mut Option<Value>,
    field: &'static str,
) -> Result<Option<String>, DecodeError> {
    match take_required(slot, field)? {
        Value::Nil => Ok(None),
        Value::String(value) => value
            .into_str()
            .map(Some)
            .ok_or(DecodeError::InvalidFieldType(field)),
        _ => Err(DecodeError::InvalidFieldType(field)),
    }
}

fn take_optional_integer(
    slot: &mut Option<Value>,
    field: &'static str,
) -> Result<Option<RnsInteger>, DecodeError> {
    match take_required(slot, field)? {
        Value::Nil => Ok(None),
        Value::Integer(value) => integer(value)
            .map(Some)
            .ok_or(DecodeError::InvalidFieldType(field)),
        _ => Err(DecodeError::InvalidFieldType(field)),
    }
}

fn take_optional_number(
    slot: &mut Option<Value>,
    field: &'static str,
) -> Result<Option<RnsNumber>, DecodeError> {
    match take_required(slot, field)? {
        Value::Nil => Ok(None),
        Value::Integer(value) => integer(value)
            .map(RnsNumber::Integer)
            .map(Some)
            .ok_or(DecodeError::InvalidFieldType(field)),
        Value::F32(value) => Ok(Some(RnsNumber::Float(f64::from(value)))),
        Value::F64(value) => Ok(Some(RnsNumber::Float(value))),
        _ => Err(DecodeError::InvalidFieldType(field)),
    }
}

fn integer(value: rmpv::Integer) -> Option<RnsInteger> {
    value.as_u64().map(RnsInteger::Nonnegative).or_else(|| {
        value
            .as_i64()
            .filter(|value| *value < 0)
            .map(RnsInteger::Negative)
    })
}

fn take_destination_hash(slot: &mut Option<Value>) -> Result<DestinationHash, DecodeError> {
    take_binary::<16>(slot, "destination_hash").map(DestinationHash::new)
}

fn take_transport_id(slot: &mut Option<Value>) -> Result<TransportId, DecodeError> {
    take_binary::<16>(slot, "destination_hash").map(TransportId::new)
}

fn take_identity_hash(slot: &mut Option<Value>) -> Result<IdentityHash, DecodeError> {
    take_binary::<16>(slot, "identity_hash").map(IdentityHash::new)
}

fn take_packet_hash(slot: &mut Option<Value>) -> Result<PacketHashArgument, DecodeError> {
    let Value::Binary(bytes) = take_required(slot, "packet_hash")? else {
        return Err(DecodeError::InvalidFieldType("packet_hash"));
    };
    Ok(PacketHashArgument(bytes))
}

fn take_binary<const N: usize>(
    slot: &mut Option<Value>,
    field: &'static str,
) -> Result<[u8; N], DecodeError> {
    let Value::Binary(bytes) = take_required(slot, field)? else {
        return Err(DecodeError::InvalidFieldType(field));
    };
    let actual = bytes.len();
    bytes
        .try_into()
        .map_err(|_| DecodeError::InvalidHashLength {
            field,
            expected: N,
            actual,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(entries: Vec<(&str, Value)>) -> Vec<u8> {
        let value = Value::Map(
            entries
                .into_iter()
                .map(|(key, value)| (Value::from(key), value))
                .collect(),
        );
        let mut bytes = Vec::new();
        rmpv::encode::write_value(&mut bytes, &value).unwrap();
        bytes
    }

    fn binary<const N: usize>(byte: u8) -> Value {
        Value::Binary(vec![byte; N])
    }

    #[test]
    fn decodes_every_rns_1_3_8_operation() {
        let cases = [
            request(vec![("get", Value::from("interface_stats"))]),
            request(vec![
                ("get", Value::from("path_table")),
                ("max_hops", Value::Nil),
            ]),
            request(vec![("get", Value::from("rate_table"))]),
            request(vec![
                ("get", Value::from("next_hop_if_name")),
                ("destination_hash", binary::<16>(1)),
            ]),
            request(vec![
                ("get", Value::from("next_hop")),
                ("destination_hash", binary::<16>(1)),
            ]),
            request(vec![
                ("get", Value::from("first_hop_timeout")),
                ("destination_hash", binary::<16>(1)),
            ]),
            request(vec![("get", Value::from("link_count"))]),
            request(vec![
                ("get", Value::from("packet_rssi")),
                ("packet_hash", binary::<16>(2)),
            ]),
            request(vec![
                ("get", Value::from("packet_snr")),
                ("packet_hash", binary::<16>(2)),
            ]),
            request(vec![
                ("get", Value::from("packet_q")),
                ("packet_hash", binary::<16>(2)),
            ]),
            request(vec![("get", Value::from("blackholed_identities"))]),
            request(vec![
                ("get", Value::from("is_blackholed")),
                ("identity_hash", binary::<16>(3)),
            ]),
            request(vec![
                ("drop", Value::from("path")),
                ("destination_hash", binary::<16>(4)),
            ]),
            request(vec![
                ("drop", Value::from("all_via")),
                ("destination_hash", binary::<16>(4)),
            ]),
            request(vec![("drop", Value::from("announce_queues"))]),
            request(vec![
                ("blackhole_identity", binary::<16>(5)),
                ("until", Value::Nil),
                ("reason", Value::Nil),
            ]),
            request(vec![("unblackhole_identity", binary::<16>(5))]),
            request(vec![
                ("destination_data", Value::from("used")),
                ("destination_hash", binary::<16>(6)),
            ]),
            request(vec![
                ("destination_data", Value::from("retain")),
                ("destination_hash", binary::<16>(6)),
            ]),
            request(vec![
                ("destination_data", Value::from("unretain")),
                ("destination_hash", binary::<16>(6)),
            ]),
            request(vec![
                ("identity_data", Value::from("retain")),
                ("identity_hash", binary::<16>(7)),
            ]),
        ];

        assert_eq!(cases.len(), 21);
        for bytes in cases {
            assert!(decode(&bytes).is_ok(), "request rejected: {bytes:02x?}");
        }
    }

    #[test]
    fn preserves_numeric_and_optional_blackhole_arguments() {
        assert_eq!(
            decode(&request(vec![
                ("get", Value::from("path_table")),
                ("max_hops", Value::from(u64::MAX)),
            ])),
            Ok(RnsRpcRequest::PathTable {
                max_hops: Some(RnsInteger::Nonnegative(u64::MAX)),
            })
        );
        assert_eq!(
            decode(&request(vec![
                ("blackhole_identity", binary::<16>(5)),
                ("until", Value::F64(123.5)),
                ("reason", Value::from("operator request")),
            ])),
            Ok(RnsRpcRequest::BlackholeIdentity {
                identity_hash: IdentityHash::new([5; 16]),
                until: Some(RnsNumber::Float(123.5)),
                reason: Some("operator request".into()),
            })
        );
    }

    #[test]
    fn blackhole_deadlines_preserve_rns_138_truthiness_and_epoch_seconds() {
        assert_eq!(
            RnsNumber::Integer(RnsInteger::Negative(1)).blackhole_expiry(),
            BlackholeExpiry::At(InstantMillis(0))
        );
        assert_eq!(
            RnsNumber::Integer(RnsInteger::Nonnegative(0)).blackhole_expiry(),
            BlackholeExpiry::Indefinite
        );
        assert_eq!(
            RnsNumber::Float(f64::NAN).blackhole_expiry(),
            BlackholeExpiry::Indefinite
        );
        assert_eq!(
            RnsNumber::Float(f64::INFINITY).blackhole_expiry(),
            BlackholeExpiry::At(InstantMillis(u64::MAX))
        );
        assert_eq!(
            RnsNumber::Float(123.4567).blackhole_expiry(),
            BlackholeExpiry::At(InstantMillis(123_456))
        );
    }

    #[test]
    fn packet_hash_arguments_preserve_the_rpc_lookup_key() {
        assert_eq!(
            decode(&request(vec![
                ("get", Value::from("packet_rssi")),
                ("packet_hash", binary::<16>(2)),
            ])),
            Ok(RnsRpcRequest::PacketRssi {
                packet_hash: PacketHashArgument(vec![2; 16]),
            })
        );
        assert_eq!(
            decode(&request(vec![
                ("get", Value::from("packet_rssi")),
                ("packet_hash", binary::<32>(2)),
            ])),
            Ok(RnsRpcRequest::PacketRssi {
                packet_hash: PacketHashArgument(vec![2; 32]),
            })
        );
    }

    #[test]
    fn rejects_malformed_ambiguous_and_incomplete_requests() {
        assert_eq!(decode(&[]), Err(DecodeError::MessagePack));
        assert_eq!(decode(&[0xc0]), Err(DecodeError::ExpectedMap));

        let mut trailing = request(vec![("get", Value::from("link_count"))]);
        trailing.push(0xc0);
        assert_eq!(decode(&trailing), Err(DecodeError::TrailingData));

        assert_eq!(
            decode(&request(vec![
                ("get", Value::from("link_count")),
                ("drop", Value::from("announce_queues")),
            ])),
            Err(DecodeError::ContradictoryOperation)
        );
        assert_eq!(
            decode(&request(vec![("destination_hash", binary::<16>(1))])),
            Err(DecodeError::MissingOperation)
        );
        assert_eq!(
            decode(&request(vec![("get", Value::from("next_hop"))])),
            Err(DecodeError::MissingField("destination_hash"))
        );
        assert_eq!(
            decode(&request(vec![
                ("get", Value::from("next_hop")),
                ("destination_hash", binary::<15>(1)),
            ])),
            Err(DecodeError::InvalidHashLength {
                field: "destination_hash",
                expected: 16,
                actual: 15,
            })
        );
        assert_eq!(
            decode(&request(vec![
                ("get", Value::from("link_count")),
                ("reason", Value::from("interface_stats")),
            ])),
            Err(DecodeError::UnexpectedField("reason"))
        );
    }

    #[test]
    fn rejects_duplicate_and_unknown_fields() {
        assert_eq!(
            decode(&request(vec![
                ("get", Value::from("link_count")),
                ("get", Value::from("rate_table")),
            ])),
            Err(DecodeError::DuplicateField("get".into()))
        );
        assert_eq!(
            decode(&request(vec![("future", Value::from("link_count"))])),
            Err(DecodeError::UnknownField("future".into()))
        );
    }
}
