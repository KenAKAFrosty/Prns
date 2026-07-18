use std::string::String;
use std::vec::Vec;

use prns_core::interfaces::shared_instance::rns_rpc::{RnsInteger, RpcDialect};
use prns_core::interfaces::{ConnectionState, InterfaceId};
use prns_core::routing::types::NextHop;
use prns_runtime::node_introspection::{
    logical_interface_inventory, AnnounceRateSnapshot, InterfaceInventoryEntry, RouteSnapshot,
};
use rmpv::Value;

/// RNS's `Interface.MODE_FULL` — the default interface mode a stock client renders as "Full".
const RNS_INTERFACE_MODE_FULL: i64 = 0x01;

/// `get_next_hop` in the client's dialect: the next-hop hash to reach the destination — the transport node a route goes via, or the destination itself when it is directly reachable — as bytes, or [`None`] when no route is held (RNS `Transport.next_hop`).
pub(super) fn reply_next_hop(
    dialect: RpcDialect,
    route: Option<RouteSnapshot>,
) -> std::io::Result<Vec<u8>> {
    match route {
        Some(entry) => reply_bytes(dialect, next_hop_bytes(&entry)),
        None => reply_none(dialect),
    }
}

/// `get_next_hop_if_name` in the client's dialect: the name of the interface the route is reached over. RNS returns `str(interface)`, so an unknown route is the literal string `"None"`, never nil.
pub(super) fn reply_next_hop_if_name(
    dialect: RpcDialect,
    route: Option<RouteSnapshot>,
) -> std::io::Result<Vec<u8>> {
    let name = route.map_or_else(
        || String::from("None"),
        |entry| interface_name(entry.interface),
    );
    reply_str(dialect, &name)
}

fn next_hop_bytes(entry: &RouteSnapshot) -> Vec<u8> {
    match entry.via {
        NextHop::Via(transport) => transport.as_bytes().to_vec(),
        NextHop::Direct => entry.destination.as_bytes().to_vec(),
    }
}

/// The path table in the client's dialect. Msgpack renders each known route as RNS's path-table dict (`hash`, `via`, `hops`, `timestamp`, `expires`, `interface`); the legacy pickle dialect gets an empty list — a legacy client lists nothing rather than faulting. `timestamp`/`expires` carry the engine's learned-at clock in milliseconds.
pub(super) fn reply_path_table(
    dialect: RpcDialect,
    entries: Vec<RouteSnapshot>,
    max_hops: Option<&RnsInteger>,
) -> std::io::Result<Vec<u8>> {
    match dialect {
        RpcDialect::Pickle => Ok(b"].".to_vec()),
        RpcDialect::Msgpack => {
            let entries = entries
                .into_iter()
                .filter(|entry| within_hop_limit(entry.hops, max_hops))
                .collect::<Vec<_>>();
            encode_msgpack(path_table_value(entries))
        }
    }
}

fn within_hop_limit(hops: u8, maximum: Option<&RnsInteger>) -> bool {
    maximum.is_none_or(|maximum| {
        maximum
            .nonnegative_value()
            .is_some_and(|maximum| u64::from(hops) <= maximum)
    })
}

pub(crate) fn path_table_value(entries: Vec<RouteSnapshot>) -> Value {
    Value::Array(
        entries
            .into_iter()
            .map(|entry| {
                let via = next_hop_bytes(&entry);
                let last_active_at = prns_core::engine::InstantMillis(
                    entry.learned_at.0.max(entry.last_relayed_at.0),
                );
                Value::Map(std::vec![
                    (
                        "hash".into(),
                        Value::Binary(entry.destination.as_bytes().to_vec()),
                    ),
                    ("via".into(), Value::Binary(via)),
                    ("hops".into(), Value::from(i64::from(entry.hops))),
                    ("timestamp".into(), rns_timestamp(last_active_at)),
                    ("expires".into(), rns_timestamp(entry.expires_at)),
                    (
                        "interface".into(),
                        Value::from(interface_name(entry.interface)),
                    ),
                ])
            })
            .collect(),
    )
}

/// A human name for an interface in the path table — RNS shows `str(interface)`. The kind names the medium; a short hash prefix disambiguates instances of the same kind.
fn interface_name(id: InterfaceId) -> String {
    let mut name = match id.kind() {
        Some(kind) => std::format!("{kind:?}["),
        None => std::string::String::from("Interface["),
    };
    for byte in id.as_bytes().iter().take(4) {
        name.push_str(&std::format!("{byte:02x}"));
    }
    name.push(']');
    name
}

/// `None` in the client's dialect: pickle protocol-0 `NONE` + `STOP`, or msgpack nil.
pub(super) fn reply_none(dialect: RpcDialect) -> std::io::Result<Vec<u8>> {
    match dialect {
        RpcDialect::Pickle => Ok(b"N.".to_vec()),
        RpcDialect::Msgpack => encode_msgpack(Value::Nil),
    }
}

/// A bool in the client's dialect: pickle protocol-0 bool-as-int, or msgpack bool.
pub(super) fn reply_bool(dialect: RpcDialect, value: bool) -> std::io::Result<Vec<u8>> {
    match (dialect, value) {
        (RpcDialect::Pickle, false) => Ok(b"I00\n.".to_vec()),
        (RpcDialect::Pickle, true) => Ok(b"I01\n.".to_vec()),
        (RpcDialect::Msgpack, value) => encode_msgpack(Value::Boolean(value)),
    }
}

/// An integer in the client's dialect: pickle protocol-0 `INT` + `STOP`, or msgpack through the typed [`Value`] encoder (so any width — not just a fixint — encodes correctly).
pub(super) fn reply_int(dialect: RpcDialect, value: i64) -> std::io::Result<Vec<u8>> {
    match dialect {
        RpcDialect::Pickle => {
            let mut out = std::vec![b'I'];
            out.extend_from_slice(std::format!("{value}").as_bytes());
            out.push(b'\n');
            out.push(b'.');
            Ok(out)
        }
        RpcDialect::Msgpack => encode_msgpack(Value::from(value)),
    }
}

pub(super) fn reply_rate_table(
    dialect: RpcDialect,
    entries: Vec<AnnounceRateSnapshot>,
) -> std::io::Result<Vec<u8>> {
    match dialect {
        RpcDialect::Pickle => Ok(b"].".to_vec()),
        RpcDialect::Msgpack => encode_msgpack(rate_table_value(entries)),
    }
}

pub(crate) fn rate_table_value(entries: Vec<AnnounceRateSnapshot>) -> Value {
    Value::Array(
        entries
            .into_iter()
            .map(|entry| {
                let blocked_until = if entry.blocked_until.0 == 0 {
                    Value::from(0)
                } else {
                    rns_timestamp(entry.blocked_until)
                };
                Value::Map(std::vec![
                    (
                        "hash".into(),
                        Value::Binary(entry.destination.as_bytes().to_vec()),
                    ),
                    ("last".into(), rns_timestamp(entry.last_allowed_announce_at)),
                    (
                        "rate_violations".into(),
                        Value::from(u64::from(entry.rate_violations)),
                    ),
                    ("blocked_until".into(), blocked_until),
                    (
                        "timestamps".into(),
                        Value::Array(entry.observed_at.into_iter().map(rns_timestamp).collect(),),
                    ),
                ])
            })
            .collect(),
    )
}

fn rns_timestamp(timestamp: prns_core::engine::InstantMillis) -> Value {
    Value::F64(std::time::Duration::from_millis(timestamp.0).as_secs_f64())
}

/// An empty map in the client's dialect: the shape of RNS's blackholed identity table.
pub(super) fn reply_empty_map(dialect: RpcDialect) -> std::io::Result<Vec<u8>> {
    match dialect {
        RpcDialect::Pickle => Ok(b"}.".to_vec()),
        RpcDialect::Msgpack => encode_msgpack(Value::Map(std::vec![])),
    }
}

/// A byte string in the client's dialect: msgpack `bin` through the typed [`Value`] encoder, or pickle `SHORT_BINBYTES` (`PROTO 3`, since a `bytes` object needs protocol 3) + `STOP`. Lengths over 255 are truncated by the `as u8`; an RNS hash is 16 bytes, so this is exact for every value the shim sends.
pub(super) fn reply_bytes(dialect: RpcDialect, value: Vec<u8>) -> std::io::Result<Vec<u8>> {
    match dialect {
        RpcDialect::Msgpack => encode_msgpack(Value::Binary(value)),
        RpcDialect::Pickle => {
            let mut out = std::vec![0x80, 0x03, b'C', value.len() as u8];
            out.extend_from_slice(&value);
            out.push(b'.');
            Ok(out)
        }
    }
}

/// A string in the client's dialect: msgpack `str` through the typed [`Value`] encoder, or pickle protocol-0 `UNICODE` (a newline-terminated string) + `STOP`. Interface names are ASCII (a medium name plus a hex hash prefix), so the raw protocol-0 form needs no escaping.
pub(super) fn reply_str(dialect: RpcDialect, value: &str) -> std::io::Result<Vec<u8>> {
    match dialect {
        RpcDialect::Msgpack => encode_msgpack(Value::from(value)),
        RpcDialect::Pickle => {
            let mut out = std::vec![b'V'];
            out.extend_from_slice(value.as_bytes());
            out.push(b'\n');
            out.push(b'.');
            Ok(out)
        }
    }
}

/// The node's interface stats in the client's dialect: the shape `rnstatus` and a NomadNet TextUI index by `["interfaces"]`. Msgpack renders each interface's live counters into RNS's per-interface dict; untracked fields are simply absent, which a stock client reads as "not reported" rather than a fabricated zero. The legacy pickle dialect gets the well-formed empty map (`{"interfaces": []}`, `protocol=2`).
pub(super) fn reply_interface_stats(
    dialect: RpcDialect,
    inventory: Vec<InterfaceInventoryEntry>,
) -> std::io::Result<Vec<u8>> {
    match dialect {
        RpcDialect::Pickle => Ok(std::vec![
            0x80, 0x02, 0x7d, 0x71, 0x00, 0x58, 0x0a, 0x00, 0x00, 0x00, b'i', b'n', b't', b'e',
            b'r', b'f', b'a', b'c', b'e', b's', 0x71, 0x01, 0x5d, 0x71, 0x02, 0x73, 0x2e,
        ]),
        RpcDialect::Msgpack => encode_msgpack(interface_stats_value(&logical_interface_inventory(
            inventory,
        ))),
    }
}

pub(crate) fn interface_stats_value(inventory: &[InterfaceInventoryEntry]) -> Value {
    let mut total_rxb = 0u64;
    let mut total_txb = 0u64;
    let mut total_rxs = 0u64;
    let mut total_txs = 0u64;
    let rows = inventory
        .iter()
        .map(|entry| {
            let interface = &entry.snapshot;
            let ifac = entry.ifac.as_ref();
            let name = entry
                .name
                .clone()
                .unwrap_or_else(|| interface_name(interface.id));
            let rates = interface
                .transfer_rates
                .unwrap_or(prns_core::interfaces::TransferRates {
                    rx_bps: 0,
                    tx_bps: 0,
                });
            total_rxb += interface.rx_bytes;
            total_txb += interface.tx_bytes;
            total_rxs += u64::from(rates.rx_bps);
            total_txs += u64::from(rates.tx_bps);
            Value::Map(std::vec![
                ("name".into(), Value::from(name.clone())),
                ("short_name".into(), Value::from(name)),
                ("type".into(), Value::from(interface_type(interface.id))),
                (
                    "status".into(),
                    Value::Boolean(is_online(interface.connection))
                ),
                ("mode".into(), Value::from(RNS_INTERFACE_MODE_FULL)),
                ("clients".into(), Value::Nil),
                ("rxb".into(), Value::from(interface.rx_bytes as i64)),
                ("txb".into(), Value::from(interface.tx_bytes as i64)),
                ("rxs".into(), Value::from(i64::from(rates.rx_bps))),
                ("txs".into(), Value::from(i64::from(rates.tx_bps))),
                (
                    "ifac_signature".into(),
                    ifac.map_or(Value::Nil, |ifac| Value::Binary(ifac.signature.to_vec())),
                ),
                (
                    "ifac_size".into(),
                    ifac.map_or(Value::Nil, |ifac| Value::from(ifac.size.bytes() as i64)),
                ),
                (
                    "ifac_netname".into(),
                    ifac.and_then(|ifac| ifac.network_name.as_ref())
                        .map_or(Value::Nil, |name| Value::from(name.clone())),
                ),
            ])
        })
        .collect();
    Value::Map(std::vec![
        ("interfaces".into(), Value::Array(rows)),
        ("rxb".into(), Value::from(total_rxb as i64)),
        ("txb".into(), Value::from(total_txb as i64)),
        ("rxs".into(), Value::from(total_rxs as i64)),
        ("txs".into(), Value::from(total_txs as i64)),
        ("rss".into(), Value::Nil),
    ])
}

pub(crate) fn encode_msgpack(value: Value) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    rmpv::encode::write_value(&mut bytes, &value)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    Ok(bytes)
}

/// RNS's per-interface `status` flag is a plain bool: up or down. A `Connected` or `Degraded` interface is up (degraded carries traffic, just impaired); everything else — initializing, reconnecting, failed, disconnected, disabled — is down.
fn is_online(connection: ConnectionState) -> bool {
    matches!(
        connection,
        ConnectionState::Connected | ConnectionState::Degraded
    )
}

/// RNS's per-interface `type` is the interface class name. The kind names the medium; an interface with no kind byte is a bare `Interface`.
fn interface_type(id: InterfaceId) -> String {
    match id.kind() {
        Some(kind) => std::format!("{kind:?}"),
        None => std::string::String::from("Interface"),
    }
}
