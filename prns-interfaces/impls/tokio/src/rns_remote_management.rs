use std::io::Cursor;
use std::time::Duration;
use std::vec::Vec;

use prns_core::identity::IdentityHash;
use prns_core::wire::DestinationHash;
use prns_runtime::node_introspection::{
    logical_interface_inventory, AnnounceRateSnapshot, InterfaceInventoryEntry, RouteSnapshot,
};
use rmpv::Value;

use crate::shared_instance::rpc_compat::reply::{
    encode_msgpack, interface_stats_value, path_table_value, rate_table_value,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteStatusRequest {
    InterfaceStats,
    InterfaceStatsAndLinkCount,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestinationSelection {
    All,
    Exact(DestinationHash),
    NoMatch,
}

impl DestinationSelection {
    fn includes(self, destination: DestinationHash) -> bool {
        match self {
            Self::All => true,
            Self::Exact(selected) => selected == destination,
            Self::NoMatch => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HopSelection {
    All,
    AtMost(u64),
    NoMatch,
}

impl HopSelection {
    fn includes(self, hops: u8) -> bool {
        match self {
            Self::All => true,
            Self::AtMost(maximum) => u64::from(hops) <= maximum,
            Self::NoMatch => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemotePathRequest {
    Table {
        destination: DestinationSelection,
        hops: HopSelection,
    },
    Rates {
        destination: DestinationSelection,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteRequestDecodeError {
    InvalidMessagePack,
    InvalidShape,
    UnsupportedCommand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteResponseEncodeError;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RemoteTransportStatus {
    pub transport_identity: IdentityHash,
    pub network_identity: Option<IdentityHash>,
    pub uptime: Duration,
}

pub fn decode_status_request(
    bytes: &[u8],
) -> Result<RemoteStatusRequest, RemoteRequestDecodeError> {
    let value = decode(bytes)?;
    let Value::Array(values) = value else {
        return Err(RemoteRequestDecodeError::InvalidShape);
    };
    let Some(include_link_count) = values.first() else {
        return Err(RemoteRequestDecodeError::InvalidShape);
    };
    if equals_python_true(include_link_count) {
        Ok(RemoteStatusRequest::InterfaceStatsAndLinkCount)
    } else {
        Ok(RemoteStatusRequest::InterfaceStats)
    }
}

pub fn decode_path_request(bytes: &[u8]) -> Result<RemotePathRequest, RemoteRequestDecodeError> {
    let value = decode(bytes)?;
    let Value::Array(values) = value else {
        return Err(RemoteRequestDecodeError::InvalidShape);
    };
    let command = values
        .first()
        .and_then(Value::as_str)
        .ok_or(RemoteRequestDecodeError::InvalidShape)?;
    let destination = values
        .get(1)
        .map_or(DestinationSelection::All, destination_selection);
    match command {
        "table" => Ok(RemotePathRequest::Table {
            destination,
            hops: values.get(2).map_or(Ok(HopSelection::All), hop_selection)?,
        }),
        "rates" => Ok(RemotePathRequest::Rates { destination }),
        _ => Err(RemoteRequestDecodeError::UnsupportedCommand),
    }
}

pub fn encode_status_response(
    request: RemoteStatusRequest,
    inventory: Vec<InterfaceInventoryEntry>,
    link_count: u32,
    transport: Option<RemoteTransportStatus>,
) -> Result<Vec<u8>, RemoteResponseEncodeError> {
    let mut stats = interface_stats_value(&logical_interface_inventory(inventory));
    if let (Value::Map(fields), Some(transport)) = (&mut stats, transport) {
        fields.push((
            "transport_id".into(),
            Value::Binary(transport.transport_identity.as_bytes().to_vec()),
        ));
        fields.push((
            "network_id".into(),
            transport.network_identity.map_or(Value::Nil, |identity| {
                Value::Binary(identity.as_bytes().to_vec())
            }),
        ));
        fields.push((
            "transport_uptime".into(),
            Value::F64(transport.uptime.as_secs_f64()),
        ));
        fields.push(("probe_responder".into(), Value::Nil));
    }
    let mut response = vec![stats];
    if request == RemoteStatusRequest::InterfaceStatsAndLinkCount {
        response.push(Value::from(u64::from(link_count)));
    }
    encode_msgpack(Value::Array(response)).map_err(|_| RemoteResponseEncodeError)
}

pub fn encode_path_table_response(
    request: RemotePathRequest,
    entries: Vec<RouteSnapshot>,
) -> Result<Vec<u8>, RemoteResponseEncodeError> {
    let RemotePathRequest::Table { destination, hops } = request else {
        return Err(RemoteResponseEncodeError);
    };
    let entries = entries
        .into_iter()
        .filter(|entry| destination.includes(entry.destination) && hops.includes(entry.hops))
        .collect();
    encode_msgpack(path_table_value(entries)).map_err(|_| RemoteResponseEncodeError)
}

pub fn encode_rate_table_response(
    request: RemotePathRequest,
    entries: Vec<AnnounceRateSnapshot>,
) -> Result<Vec<u8>, RemoteResponseEncodeError> {
    let RemotePathRequest::Rates { destination } = request else {
        return Err(RemoteResponseEncodeError);
    };
    let entries = entries
        .into_iter()
        .filter(|entry| destination.includes(entry.destination))
        .collect();
    encode_msgpack(rate_table_value(entries)).map_err(|_| RemoteResponseEncodeError)
}

fn decode(bytes: &[u8]) -> Result<Value, RemoteRequestDecodeError> {
    let mut cursor = Cursor::new(bytes);
    let value = rmpv::decode::read_value(&mut cursor)
        .map_err(|_| RemoteRequestDecodeError::InvalidMessagePack)?;
    if usize::try_from(cursor.position()).ok() != Some(bytes.len()) {
        return Err(RemoteRequestDecodeError::InvalidMessagePack);
    }
    Ok(value)
}

fn destination_selection(value: &Value) -> DestinationSelection {
    match value {
        Value::Nil => DestinationSelection::All,
        Value::Binary(bytes) if bytes.len() == 16 => {
            let mut destination = [0u8; 16];
            destination.copy_from_slice(bytes);
            DestinationSelection::Exact(DestinationHash::new(destination))
        }
        _ => DestinationSelection::NoMatch,
    }
}

fn hop_selection(value: &Value) -> Result<HopSelection, RemoteRequestDecodeError> {
    match value {
        Value::Nil => Ok(HopSelection::All),
        Value::Integer(value) => match value.as_i64() {
            Some(value) if value < 0 => Ok(HopSelection::NoMatch),
            Some(value) => Ok(HopSelection::AtMost(value as u64)),
            None => value
                .as_u64()
                .map(HopSelection::AtMost)
                .ok_or(RemoteRequestDecodeError::InvalidShape),
        },
        _ => Err(RemoteRequestDecodeError::InvalidShape),
    }
}

fn equals_python_true(value: &Value) -> bool {
    match value {
        Value::Boolean(value) => *value,
        Value::Integer(value) => value.as_i64() == Some(1) || value.as_u64() == Some(1),
        Value::F32(value) => *value == 1.0,
        Value::F64(value) => *value == 1.0,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_core::engine::InstantMillis;
    use prns_core::interfaces::{InterfaceId, InterfaceKind};
    use prns_core::routing::types::NextHop;

    #[test]
    fn status_request_matches_python_truth_equality() {
        assert_eq!(
            decode_status_request(&[0x91, 0xc3]),
            Ok(RemoteStatusRequest::InterfaceStatsAndLinkCount)
        );
        assert_eq!(
            decode_status_request(&[0x91, 0x01]),
            Ok(RemoteStatusRequest::InterfaceStatsAndLinkCount)
        );
        assert_eq!(
            decode_status_request(&[0x91, 0xc2]),
            Ok(RemoteStatusRequest::InterfaceStats)
        );
        assert_eq!(
            decode_status_request(&[0x90]),
            Err(RemoteRequestDecodeError::InvalidShape)
        );
    }

    #[test]
    fn path_request_decodes_stock_table_and_rate_shapes() {
        let destination = [0x42; 16];
        let table = bytes_from_hex("93a57461626c65c4104242424242424242424242424242424203");
        assert_eq!(
            decode_path_request(&table),
            Ok(RemotePathRequest::Table {
                destination: DestinationSelection::Exact(DestinationHash::new(destination)),
                hops: HopSelection::AtMost(3),
            })
        );
        let rates = bytes_from_hex("92a57261746573c0");
        assert_eq!(
            decode_path_request(&rates),
            Ok(RemotePathRequest::Rates {
                destination: DestinationSelection::All,
            })
        );
    }

    #[test]
    fn table_response_filters_before_using_the_shared_stock_projection() {
        let selected = DestinationHash::new([0x42; 16]);
        let entries = vec![
            route(selected, 2),
            route(DestinationHash::new([0x43; 16]), 1),
            route(selected, 4),
        ];
        let encoded = encode_path_table_response(
            RemotePathRequest::Table {
                destination: DestinationSelection::Exact(selected),
                hops: HopSelection::AtMost(3),
            },
            entries,
        )
        .unwrap();
        let decoded = decode(&encoded).unwrap();
        let Value::Array(rows) = decoded else {
            panic!("path response is an array");
        };
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn status_response_has_the_reference_outer_shape_and_transport_fields() {
        let encoded = encode_status_response(
            RemoteStatusRequest::InterfaceStatsAndLinkCount,
            Vec::new(),
            2,
            Some(RemoteTransportStatus {
                transport_identity: IdentityHash::new([0x11; 16]),
                network_identity: Some(IdentityHash::new([0x22; 16])),
                uptime: Duration::from_millis(1_500),
            }),
        )
        .unwrap();
        assert_eq!(
            encoded,
            bytes_from_hex(
                "928aaa696e746572666163657390a372786200a374786200a372787300a374787300a3727373c0ac7472616e73706f72745f6964c41011111111111111111111111111111111aa6e6574776f726b5f6964c41022222222222222222222222222222222b07472616e73706f72745f757074696d65cb3ff8000000000000af70726f62655f726573706f6e646572c002",
            )
        );
        let Value::Array(response) = decode(&encoded).unwrap() else {
            panic!("status response is an array");
        };
        assert_eq!(response.len(), 2);
        assert_eq!(response[1], Value::from(2));
        let Value::Map(stats) = &response[0] else {
            panic!("status body is a map");
        };
        assert!(stats
            .iter()
            .any(|(key, value)| key.as_str() == Some("transport_uptime")
                && value == &Value::F64(1.5)));
    }

    fn route(destination: DestinationHash, hops: u8) -> RouteSnapshot {
        RouteSnapshot {
            destination,
            hops,
            via: NextHop::Direct,
            learned_at: InstantMillis(1_000),
            last_relayed_at: InstantMillis(1_500),
            expires_at: InstantMillis(2_000),
            interface: InterfaceId::from_channel_tag(InterfaceKind::TcpClient, b"remote"),
        }
    }

    fn bytes_from_hex(value: &str) -> Vec<u8> {
        value
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }
}
