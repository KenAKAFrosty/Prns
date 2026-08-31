use super::*;
use crate::engine::RouteSnapshot;
use crate::identity::IdentityHash;
use crate::interfaces::IfacSize;
use crate::interfaces::{
    ConnectionState, InterfaceId, InterfaceKind, InterfaceSnapshot, Membership, TransferRates,
};
use crate::routing::NextHop;
use crate::routing::{BlackholeExpiry, BlackholedIdentity};
use crate::units::InstantMillis;
use crate::wire::DestinationHash;
use core::time::Duration;

use super::wire_names::interface;

fn decode(bytes: &[u8]) -> rmpv::Value {
    match rmpv::decode::read_value(&mut std::io::Cursor::new(bytes)) {
        Ok(value) => value,
        Err(error) => panic!("encoded management reply must be MessagePack: {error}"),
    }
}

#[test]
fn remote_status_matches_the_rns_1_4_2_outer_shape() {
    let status = RnsInterfaceStats::new(Vec::new()).with_transport(RnsTransportStatus::new(
        IdentityHash::new([0x11; 16]),
        Some(IdentityHash::new([0x22; 16])),
        Duration::from_millis(1_500),
    ));
    let expected = bytes_from_hex(
        "928aaa696e746572666163657390a372786200a374786200a372787300a374787300a3727373c0ac7472616e73706f72745f6964c41011111111111111111111111111111111aa6e6574776f726b5f6964c41022222222222222222222222222222222b07472616e73706f72745f757074696d65cb3ff8000000000000af70726f62655f726573706f6e646572c002",
    );

    assert_eq!(status.encode_remote_response(Some(2)), Ok(expected));
}

#[test]
fn interface_stats_preserve_live_counters_and_access_code_fields() {
    let id = InterfaceId::from_channel_tag(InterfaceKind::TcpClient, b"stats");
    let stats = RnsInterfaceStats::new(vec![RnsInterfaceStatsEntry::new(
        Some(String::from("Public TCP")),
        InterfaceSnapshot {
            id,
            mode: crate::interfaces::InterfaceMode::Boundary,
            gravity: crate::interfaces::InterfaceGravity::new(-12),
            connection: ConnectionState::Degraded,
            failure_reason: None,
            rx_bytes: 10,
            tx_bytes: 20,
            transfer_rates: Some(TransferRates {
                rx_bps: 30,
                tx_bps: 40,
            }),
            destinations: 0,
            links: 0,
            transported_links: 0,
            membership: Membership::Independent,
        },
        Some(RnsInterfaceAccessCode::new(
            [0x33; 64],
            IfacSize::WIDE,
            Some(String::from("ops")),
        )),
    )]);
    let Ok(encoded) = stats.encode_message_pack() else {
        panic!("interface stats must encode");
    };
    let rmpv::Value::Map(fields) = decode(&encoded) else {
        panic!("interface stats must be a map");
    };

    assert!(fields.iter().any(|(key, value)| {
        key.as_str() == Some(interface::RECEIVE_BYTES) && value.as_u64() == Some(10)
    }));
    let interfaces = fields
        .iter()
        .find_map(|(key, value)| (key.as_str() == Some(interface::INTERFACES)).then_some(value))
        .and_then(rmpv::Value::as_array);
    assert!(interfaces.is_some_and(|entries| entries.len() == 1));
    let entry = interfaces
        .and_then(|entries| entries.first())
        .and_then(rmpv::Value::as_map);
    assert!(entry.is_some_and(|fields| {
        fields
            .iter()
            .any(|(key, value)| key.as_str() == Some(interface::MODE) && value.as_i64() == Some(5))
            && fields.iter().any(|(key, value)| {
                key.as_str() == Some(interface::GRAVITY) && value.as_i64() == Some(-12)
            })
    }));
}

#[test]
fn interface_stats_encode_nested_fleet_peers_with_rssi() {
    let supervisor = InterfaceId::from_channel_tag(InterfaceKind::BluetoothAuto, b"ble");
    let peer = InterfaceId::from_channel_tag(InterfaceKind::BluetoothPeer, b"peer");
    let stats = RnsInterfaceStats::new(vec![RnsInterfaceStatsEntry::new(
        Some(String::from("Bluetooth Auto")),
        InterfaceSnapshot {
            id: supervisor,
            mode: crate::interfaces::InterfaceMode::Full,
            gravity: crate::interfaces::InterfaceGravity::ZERO,
            connection: ConnectionState::Connected,
            failure_reason: None,
            rx_bytes: 100,
            tx_bytes: 50,
            transfer_rates: Some(TransferRates {
                rx_bps: 10,
                tx_bps: 5,
            }),
            destinations: 0,
            links: 0,
            transported_links: 0,
            membership: Membership::Independent,
        },
        None,
    )
    .with_fleet_peers(vec![RnsInterfaceStatsEntry::new(
        Some(String::from("ab12… @ AA:BB:CC:DD:EE:FF")),
        InterfaceSnapshot {
            id: peer,
            mode: crate::interfaces::InterfaceMode::Full,
            gravity: crate::interfaces::InterfaceGravity::ZERO,
            connection: ConnectionState::Connected,
            failure_reason: None,
            rx_bytes: 40,
            tx_bytes: 20,
            transfer_rates: Some(TransferRates {
                rx_bps: 4,
                tx_bps: 2,
            }),
            destinations: 0,
            links: 0,
            transported_links: 0,
            membership: Membership::FleetMember {
                supervisor_id: supervisor,
            },
        },
        None,
    )
    .with_rssi(Some(-61))])]);
    let Ok(encoded) = stats.encode_message_pack() else {
        panic!("interface stats must encode");
    };
    let report = RnsInterfaceStatsReport::decode_message_pack(&encoded)
        .expect("encoded fleet peers must decode");
    assert_eq!(report.interfaces.len(), 1);
    assert_eq!(report.interfaces[0].fleet_peers.len(), 1);
    let nested = &report.interfaces[0].fleet_peers[0];
    assert_eq!(nested.name, "ab12… @ AA:BB:CC:DD:EE:FF");
    assert!(nested.online);
    assert_eq!(nested.receive_bytes, 40);
    assert_eq!(nested.transmit_bytes, 20);
    assert_eq!(nested.rssi, RnsOptionalField::Value(-61));
}

#[test]
fn local_interface_fallback_names_match_stock_rnstatus_categories() {
    let server = InterfaceId::from_channel_tag(InterfaceKind::LocalServer, b"server");
    let client = InterfaceId::from_channel_tag(InterfaceKind::LocalClient, b"client");
    assert!(interface_name(server).starts_with("Shared Instance["));
    assert!(interface_name(client).starts_with("LocalInterface["));
}

#[test]
fn path_rate_and_blackhole_tables_keep_stock_shapes() {
    let destination = DestinationHash::new([0x42; 16]);
    let route = RouteSnapshot {
        destination,
        hops: 2,
        via: NextHop::Direct,
        learned_at: InstantMillis(1_000),
        last_route_activity_at: InstantMillis(1_500),
        expires_at: InstantMillis(2_000),
        interface: InterfaceId::from_channel_tag(InterfaceKind::TcpClient, b"route"),
    };
    let Ok(path_bytes) = RnsPathTable::new(vec![route]).encode_message_pack() else {
        panic!("path table must encode");
    };
    let Ok(rate_bytes) = RnsAnnounceRateTable::new(vec![RnsAnnounceRateEntry::new(
        destination,
        InstantMillis(1_500),
        InstantMillis(4_250),
        4,
        vec![InstantMillis(1_000), InstantMillis(1_250)],
    )])
    .encode_message_pack() else {
        panic!("rate table must encode");
    };
    let blackholes = RnsBlackholeTable::from_entries([BlackholedIdentity {
        identity: IdentityHash::new([0x11; 16]),
        source: IdentityHash::new([0xaa; 16]),
        expiry: BlackholeExpiry::At(InstantMillis(1_700_000_000_125)),
        reason: Some("operator"),
    }]);
    let Ok(blackhole_bytes) = blackholes.encode_message_pack() else {
        panic!("blackhole table must encode");
    };

    assert!(matches!(decode(&path_bytes), rmpv::Value::Array(rows) if rows.len() == 1));
    assert!(matches!(decode(&rate_bytes), rmpv::Value::Array(rows) if rows.len() == 1));
    assert_eq!(
        blackhole_bytes,
        bytes_from_hex(
            "81c4101111111111111111111111111111111183a6736f75726365c410aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa5756e74696ccb41d954fc40080000a6726561736f6ea86f70657261746f72",
        )
    );
}

fn bytes_from_hex(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| match core::str::from_utf8(pair) {
            Ok(pair) => match u8::from_str_radix(pair, 16) {
                Ok(byte) => byte,
                Err(error) => panic!("fixture must be hexadecimal: {error}"),
            },
            Err(error) => panic!("fixture must be UTF-8: {error}"),
        })
        .collect()
}
