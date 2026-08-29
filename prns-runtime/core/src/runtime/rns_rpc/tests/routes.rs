use super::*;

#[futures_test::test]
async fn a_msgpack_path_table_renders_each_route_as_a_dict() {
    let query = StubQuery {
        links: 0,
        packet_phy: None,
        rates: std::vec![],
        routes: std::vec![RouteSnapshot {
            destination: prns_core::wire::DestinationHash::new([0xab; 16]),
            hops: 3,
            via: NextHop::Direct,
            learned_at: prns_core::engine::InstantMillis(1_500),
            last_route_activity_at: prns_core::engine::InstantMillis(2_250),
            expires_at: prns_core::engine::InstantMillis(62_250),
            interface: InterfaceId::new([0x07; 8]),
        }],
        interfaces: std::vec![],
    };
    let request = msgpack_request(std::vec![
        ("get", Value::from("path_table")),
        ("max_hops", Value::Nil),
    ]);
    let reply = reply_for(&request, &query).await;
    let decoded = rmpv::decode::read_value(&mut std::io::Cursor::new(reply)).unwrap();

    assert_eq!(
        decoded,
        Value::Array(std::vec![Value::Map(std::vec![
            ("hash".into(), Value::Binary(std::vec![0xab; 16])),
            ("timestamp".into(), Value::F64(2.25)),
            ("via".into(), Value::Binary(std::vec![0xab; 16])),
            ("hops".into(), Value::from(3i64)),
            ("expires".into(), Value::F64(62.25)),
            ("interface".into(), Value::from("AutoWifi[07070707]")),
        ])])
    );
}

#[futures_test::test]
async fn a_msgpack_path_table_honors_signed_and_unbounded_max_hops() {
    let route = |hops| RouteSnapshot {
        destination: prns_core::wire::DestinationHash::new([hops; 16]),
        hops,
        via: NextHop::Direct,
        learned_at: prns_core::engine::InstantMillis(u64::from(hops)),
        last_route_activity_at: prns_core::engine::InstantMillis(0),
        expires_at: prns_core::engine::InstantMillis(u64::from(hops)),
        interface: InterfaceId::new([0x07; 8]),
    };
    let query = StubQuery {
        links: 0,
        packet_phy: None,
        rates: std::vec![],
        routes: std::vec![route(0), route(1), route(2)],
        interfaces: std::vec![],
    };
    let expected = StubQuery {
        routes: query.routes[..2].to_vec(),
        ..query.clone()
    };
    let request = |max_hops| {
        msgpack_request(std::vec![
            ("get", Value::from("path_table")),
            ("max_hops", max_hops),
        ])
    };

    assert_eq!(
        reply_for(&request(Value::from(1)), &query).await,
        reply_for(&request(Value::Nil), &expected).await
    );
    let negative_limit_reply = reply_for(&request(Value::from(-1)), &query).await;
    let decoded =
        rmpv::decode::read_value(&mut std::io::Cursor::new(negative_limit_reply)).unwrap();
    assert_eq!(decoded, Value::Array(std::vec![]));
    assert_eq!(
        reply_for(&request(Value::from(u64::MAX)), &query).await,
        reply_for(&request(Value::Nil), &query).await
    );
}

#[futures_test::test]
async fn interface_stats_renders_each_held_interface_with_its_live_counters() {
    let query = StubQuery {
        links: 0,
        packet_phy: None,
        rates: std::vec![],
        routes: std::vec![],
        interfaces: std::vec![
            InterfaceInventoryEntry {
                name: Some("Default Interface".into()),
                origin: prns_core::interfaces::InterfaceOriginKind::Configured,
                attachment_epoch: 1,
                frame_accounting: crate::node_introspection::FrameAccountingCoverage::Unavailable,
                snapshot: prns_core::interfaces::InterfaceSnapshot {
                    id: InterfaceId::new([0x07; 8]),
                    mode: prns_core::interfaces::InterfaceMode::Boundary,
                    gravity: prns_core::interfaces::InterfaceGravity::new(-8),
                    connection: ConnectionState::Connected,
                    failure_reason: None,
                    rx_bytes: 1234,
                    tx_bytes: 56,
                    transfer_rates: Some(prns_core::interfaces::TransferRates {
                        rx_bps: 800,
                        tx_bps: 100,
                    }),
                    destinations: 0,
                    links: 0,
                    transported_links: 0,
                    membership: prns_core::interfaces::Membership::Independent,
                },
                ifac: Some(InterfaceIfacSnapshot {
                    signature: [0x5a; 64],
                    size: prns_core::interfaces::IfacSize::WIDE,
                    network_name: Some("private-net".into()),
                }),
            },
            InterfaceInventoryEntry {
                name: Some("Remote bridge".into()),
                origin: prns_core::interfaces::InterfaceOriginKind::Configured,
                attachment_epoch: 2,
                frame_accounting: crate::node_introspection::FrameAccountingCoverage::Unavailable,
                snapshot: prns_core::interfaces::InterfaceSnapshot {
                    id: InterfaceId::new([0x09; 8]),
                    mode: prns_core::interfaces::InterfaceMode::Full,
                    gravity: prns_core::interfaces::InterfaceGravity::ZERO,
                    connection: ConnectionState::Reconnecting,
                    failure_reason: None,
                    rx_bytes: 10,
                    tx_bytes: 2,
                    transfer_rates: Some(prns_core::interfaces::TransferRates {
                        rx_bps: 5,
                        tx_bps: 7,
                    }),
                    destinations: 0,
                    links: 0,
                    transported_links: 0,
                    membership: prns_core::interfaces::Membership::Independent,
                },
                ifac: None,
            },
        ],
    };
    let reply = reply_for(b"\x81\xa3get\xafinterface_stats", &query).await;
    let decoded = rmpv::decode::read_value(&mut std::io::Cursor::new(reply.as_slice())).unwrap();
    let interfaces = value_field(&decoded, "interfaces")
        .and_then(Value::as_array)
        .unwrap();
    assert_eq!(interfaces.len(), 2);
    assert_eq!(
        value_field(&interfaces[0], "name").and_then(Value::as_str),
        Some("Default Interface")
    );
    assert_eq!(
        value_field(&interfaces[0], "short_name").and_then(Value::as_str),
        Some("Default Interface")
    );
    assert_eq!(
        value_field(&interfaces[0], "status"),
        Some(&Value::Boolean(true))
    );
    assert_eq!(
        value_field(&interfaces[1], "short_name").and_then(Value::as_str),
        Some("Remote bridge")
    );
    assert_eq!(
        value_field(&interfaces[1], "status"),
        Some(&Value::Boolean(false))
    );
    assert_eq!(reply[0], 0x86, "the top dict still has its 6 keys");
    let contains = |needle: &[u8]| reply.windows(needle.len()).any(|w| w == needle);
    assert!(
        contains(b"interfaces\x92"),
        "the interfaces value is a 2-element array"
    );
    assert!(contains(b"rxb") && contains(b"status") && contains(b"mode"));
    assert!(contains(b"ifac_signature") && contains(b"ifac_netname"));
    assert!(contains(b"private-net") && contains(&[0xc4, 64, 0x5a, 0x5a]));
    assert!(contains(b"\xa4type\xa8AutoWifi"));
    assert!(contains(b"\xa4type\xabLocalServer"));
    assert!(
        contains(&[0xc3]) && contains(&[0xc2]),
        "the connected interface is up (true), the reconnecting one is down (false)"
    );
    assert!(
        contains(&[
            0xa3, b'r', b'x', b'b', 0xcd, 0x04, 0xdc, 0xa3, b't', b'x', b'b', 0x3a, 0xa3, b'r',
            b'x', b's', 0xcd, 0x03, 0x25, 0xa3, b't', b'x', b's', 0x6b,
        ]),
        "top-level counters sum every interface row"
    );
}

fn mp_route_request(verb: &[u8], destination: &[u8; 16]) -> Vec<u8> {
    let mut request = std::vec![0x82, 0xa3];
    request.extend_from_slice(b"get");
    request.push(0xa0 | verb.len() as u8);
    request.extend_from_slice(verb);
    request.push(0xb0);
    request.extend_from_slice(b"destination_hash");
    request.extend_from_slice(&[0xc4, 0x10]);
    request.extend_from_slice(destination);
    request
}

fn one_via_route() -> StubQuery {
    StubQuery {
        links: 0,
        packet_phy: None,
        rates: std::vec![],
        routes: std::vec![RouteSnapshot {
            destination: DestinationHash::new([0xab; 16]),
            hops: 2,
            via: NextHop::Via(prns_core::wire::TransportId::new([0xcd; 16])),
            learned_at: prns_core::engine::InstantMillis(0),
            last_route_activity_at: prns_core::engine::InstantMillis(0),
            expires_at: prns_core::engine::InstantMillis(0),
            interface: InterfaceId::new([0x07; 8]),
        }],
        interfaces: std::vec![],
    }
}

#[futures_test::test]
async fn next_hop_answers_the_via_hash_or_nil_for_an_unknown_destination() {
    let query = one_via_route();

    let reply = reply_for(&mp_route_request(b"next_hop", &[0xab; 16]), &query).await;
    assert_eq!(
        &reply[..2],
        &[0xc4, 0x10],
        "a next-hop hash is a 16-byte bin"
    );
    assert_eq!(&reply[2..], &[0xcd; 16], "the hash is the via transport");

    let unknown = reply_for(&mp_route_request(b"next_hop", &[0x11; 16]), &query).await;
    assert_eq!(unknown, b"\xc0", "an unknown destination has no next hop");
}

#[futures_test::test]
async fn a_directly_reachable_next_hop_is_the_destination_itself() {
    let query = StubQuery {
        links: 0,
        packet_phy: None,
        rates: std::vec![],
        routes: std::vec![RouteSnapshot {
            destination: DestinationHash::new([0xab; 16]),
            hops: 1,
            via: NextHop::Direct,
            learned_at: prns_core::engine::InstantMillis(0),
            last_route_activity_at: prns_core::engine::InstantMillis(0),
            expires_at: prns_core::engine::InstantMillis(0),
            interface: InterfaceId::new([0x07; 8]),
        }],
        interfaces: std::vec![],
    };
    let reply = reply_for(&mp_route_request(b"next_hop", &[0xab; 16]), &query).await;
    assert_eq!(
        &reply[2..],
        &[0xab; 16],
        "a direct route hops straight to it"
    );
}

#[futures_test::test]
async fn next_hop_if_name_is_the_interface_name_or_the_string_none() {
    let query = one_via_route();

    let reply = reply_for(&mp_route_request(b"next_hop_if_name", &[0xab; 16]), &query).await;
    assert_eq!(reply[0] & 0xe0, 0xa0, "a short name is a msgpack fixstr");
    let name = std::str::from_utf8(&reply[1..]).unwrap();
    assert!(name.contains('['), "renders kind[hashprefix]: {name}");

    let unknown = reply_for(&mp_route_request(b"next_hop_if_name", &[0x11; 16]), &query).await;
    assert_eq!(unknown, b"\xa4None", "an unknown route's name is str(None)");
}

#[futures_test::test]
async fn a_legacy_pickle_client_gets_next_hop_in_pickle() {
    let query = one_via_route();
    let mut request = std::vec![0x80, 0x02];
    request.extend_from_slice(b"next_hopdestination_hash");
    request.extend_from_slice(&[b'C', 0x10]);
    request.extend_from_slice(&[0xab; 16]);

    let reply = reply_for(&request, &query).await;
    assert_eq!(
        &reply[..4],
        &[0x80, 0x03, b'C', 0x10],
        "pickle SHORT_BINBYTES"
    );
    assert_eq!(&reply[4..20], &[0xcd; 16]);
    assert_eq!(reply[20], b'.', "pickle STOP");
}
