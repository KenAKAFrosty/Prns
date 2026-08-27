use super::*;
use crate::runtime::node_introspection::InterfaceTimingSnapshot;
use prns_core::interfaces::{
    BitrateBps, EgressCapability, IngressCapability, InterfaceCapabilities, InterfaceKind,
    TransportCapability,
};

#[derive(Clone)]
struct TimingQuery {
    routes: Vec<RouteSnapshot>,
    timing: Vec<InterfaceTimingSnapshot>,
}

impl NodeIntrospection for TimingQuery {
    fn interface_inventory(&self) -> Vec<InterfaceInventoryEntry> {
        Vec::new()
    }

    fn interface_timing_inventory(&self) -> Vec<InterfaceTimingSnapshot> {
        self.timing.clone()
    }

    async fn link_count(&self) -> u32 {
        0
    }

    fn packet_phy(&self, _packet_hash: PacketHash) -> Option<PacketPhyStats> {
        None
    }

    async fn announce_rates(&self) -> Vec<AnnounceRateSnapshot> {
        Vec::new()
    }

    async fn routes(&self) -> Vec<RouteSnapshot> {
        self.routes.clone()
    }

    async fn route(&self, destination: DestinationHash) -> Option<RouteSnapshot> {
        self.routes
            .iter()
            .find(|route| route.destination == destination)
            .cloned()
    }
}

async fn timing_reply(request: &[u8], query: &TimingQuery) -> Vec<u8> {
    let request = RpcRequest::decode(request).unwrap();
    let controls = StubQuery {
        links: 0,
        packet_phy: None,
        rates: Vec::new(),
        routes: Vec::new(),
        interfaces: Vec::new(),
    };
    reply_for_decoded(
        &request,
        query,
        &controls,
        &controls,
        &controls,
        TEST_TRANSPORT_IDENTITY_HASH,
        None,
    )
    .await
    .unwrap()
}

fn transmitting_capabilities() -> InterfaceCapabilities {
    InterfaceCapabilities {
        ingress: IngressCapability::Enabled,
        egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
    }
}

#[futures_test::test]
async fn bitrate_timing_queries_use_online_eligible_interfaces_in_both_dialects() {
    let destination = DestinationHash::new([0xAB; 16]);
    let selected = InterfaceId::new([0x07; 8]);
    let local = InterfaceId::from_channel_tag(InterfaceKind::LocalServer, b"app");
    let disabled = InterfaceId::new([0x08; 8]);
    let disconnected = InterfaceId::new([0x0B; 8]);
    let query = TimingQuery {
        routes: vec![RouteSnapshot {
            destination,
            hops: 1,
            via: NextHop::Direct,
            learned_at: prns_core::engine::InstantMillis(0),
            last_route_activity_at: prns_core::engine::InstantMillis(0),
            expires_at: prns_core::engine::InstantMillis(u64::MAX),
            interface: selected,
        }],
        timing: vec![
            InterfaceTimingSnapshot {
                id: selected,
                bitrate: BitrateBps::guess(333),
                capabilities: transmitting_capabilities(),
                connection: ConnectionState::Connected,
            },
            InterfaceTimingSnapshot {
                id: local,
                bitrate: BitrateBps::guess(5),
                capabilities: transmitting_capabilities(),
                connection: ConnectionState::Connected,
            },
            InterfaceTimingSnapshot {
                id: disabled,
                bitrate: BitrateBps::guess(5),
                capabilities: InterfaceCapabilities {
                    ingress: IngressCapability::Enabled,
                    egress: EgressCapability::Disabled,
                },
                connection: ConnectionState::Connected,
            },
            InterfaceTimingSnapshot {
                id: disconnected,
                bitrate: BitrateBps::guess(5),
                capabilities: transmitting_capabilities(),
                connection: ConnectionState::Disconnected,
            },
        ],
    };
    let decode = |reply| rmpv::decode::read_value(&mut std::io::Cursor::new(reply)).unwrap();

    let first_hop = msgpack_request(vec![
        ("get", Value::from("first_hop_timeout")),
        (
            "destination_hash",
            Value::Binary(destination.as_bytes().to_vec()),
        ),
    ]);
    assert_eq!(
        decode(timing_reply(&first_hop, &query).await),
        Value::F64(18.013),
    );
    assert_eq!(
        decode(
            timing_reply(
                &msgpack_request(vec![("get", Value::from("lowest_interface_bitrate"),)]),
                &query,
            )
            .await,
        ),
        Value::from(333),
    );
    assert_eq!(
        decode(
            timing_reply(
                &msgpack_request(vec![("get", Value::from("medium_path_timeout"))]),
                &query,
            )
            .await,
        ),
        Value::F64(30.026),
    );

    assert_eq!(
        timing_reply(
            &legacy_string_request("get", "lowest_interface_bitrate"),
            &query,
        )
        .await,
        b"I333\n.",
    );
    assert_eq!(
        timing_reply(&legacy_string_request("get", "medium_path_timeout"), &query,).await,
        b"F30.026\n.",
    );

    let no_bitrate = TimingQuery {
        routes: query.routes.clone(),
        timing: Vec::new(),
    };
    assert_eq!(
        decode(
            timing_reply(
                &msgpack_request(vec![("get", Value::from("lowest_interface_bitrate"),)]),
                &no_bitrate,
            )
            .await,
        ),
        Value::Nil,
    );
    assert_eq!(
        decode(
            timing_reply(
                &msgpack_request(vec![("get", Value::from("medium_path_timeout"))]),
                &no_bitrate,
            )
            .await,
        ),
        Value::from(0),
    );
    assert_eq!(
        decode(timing_reply(&first_hop, &no_bitrate).await),
        Value::from(6),
    );
    assert_eq!(
        timing_reply(
            &legacy_string_request("get", "lowest_interface_bitrate"),
            &no_bitrate,
        )
        .await,
        b"N.",
    );
    assert_eq!(
        timing_reply(
            &legacy_string_request("get", "medium_path_timeout"),
            &no_bitrate,
        )
        .await,
        b"I0\n.",
    );
}

#[futures_test::test]
async fn the_set_answers_phy_stats_none_timeout_default_and_a_real_link_count() {
    let query = StubQuery {
        links: 2,
        packet_phy: None,
        rates: std::vec![],
        routes: std::vec![],
        interfaces: std::vec![],
    };
    let rssi = legacy_string_request("get", "packet_rssi");
    assert_eq!(reply_for(&rssi, &query).await, b"N.");
    let timeout = legacy_string_request("get", "first_hop_timeout");
    assert_eq!(reply_for(&timeout, &query).await, b"I6\n.");
    let links = legacy_string_request("get", "link_count");
    assert_eq!(reply_for(&links, &query).await, b"I2\n.");
    let path_table = legacy_string_request("get", "path_table");
    assert_eq!(reply_for(&path_table, &query).await, b"].");
    let rate_table = legacy_string_request("get", "rate_table");
    assert_eq!(reply_for(&rate_table, &query).await, b"].");
    let blackholes = legacy_string_request("get", "blackholed_identities");
    assert_eq!(reply_for(&blackholes, &query).await, b"}.");
}

#[futures_test::test]
async fn a_msgpack_client_gets_msgpack_replies_in_its_own_dialect() {
    let query = StubQuery {
        links: 2,
        packet_phy: None,
        rates: std::vec![],
        routes: std::vec![],
        interfaces: std::vec![],
    };
    let interface_stats = b"\x81\xa3get\xafinterface_stats";
    assert_eq!(
        reply_for(interface_stats, &query).await,
        b"\x86\xaainterfaces\x90\xa3rxb\x00\xa3txb\x00\xa3rxs\x00\xa3txs\x00\xa3rss\xc0",
        "no status handles -> an empty interface list with zeroed totals"
    );
    let timeout = msgpack_request(std::vec![
        ("get", Value::from("first_hop_timeout")),
        ("destination_hash", Value::Binary(std::vec![0; 16])),
    ]);
    assert_eq!(reply_for(&timeout, &query).await, b"\x06");
    let links = b"\x81\xa3get\xaalink_count";
    assert_eq!(reply_for(links, &query).await, b"\x02");
    let rssi = msgpack_request(std::vec![
        ("get", Value::from("packet_rssi")),
        ("packet_hash", Value::Binary(std::vec![0; 32])),
    ]);
    assert_eq!(reply_for(&rssi, &query).await, b"\xc0");
    let path_table = msgpack_request(std::vec![
        ("get", Value::from("path_table")),
        ("max_hops", Value::Nil),
    ]);
    assert_eq!(reply_for(&path_table, &query).await, b"\x90");
    let rate_table = b"\x81\xa3get\xaarate_table";
    assert_eq!(reply_for(rate_table, &query).await, b"\x90");
    let blackholes = msgpack_request(std::vec![("get", Value::from("blackholed_identities"),)]);
    assert_eq!(reply_for(&blackholes, &query).await, b"\x80");
}

#[futures_test::test]
async fn packet_phy_reads_project_rns_units_and_truthful_absence() {
    let packet_hash = PacketHash::new([0x42; PACKET_HASH_LEN]);
    let query = StubQuery {
        links: 0,
        packet_phy: Some((
            packet_hash,
            PacketPhyStats {
                rssi: Some(RssiDbm::new(-82)),
                snr: Some(SnrQuarterDb::new(-9)),
                quality: SignalQualityTenthsPercent::new(875),
            },
        )),
        rates: std::vec![],
        routes: std::vec![],
        interfaces: std::vec![],
    };
    let request = |metric: &str, hash: &[u8]| {
        msgpack_request(std::vec![
            ("get", Value::from(metric)),
            ("packet_hash", Value::Binary(hash.to_vec())),
        ])
    };
    let decode = |reply| rmpv::decode::read_value(&mut std::io::Cursor::new(reply)).unwrap();

    assert_eq!(
        decode(reply_for(&request("packet_rssi", packet_hash.as_bytes()), &query).await),
        Value::from(-82)
    );
    assert_eq!(
        decode(reply_for(&request("packet_snr", packet_hash.as_bytes()), &query).await),
        Value::F64(-2.25)
    );
    assert_eq!(
        decode(reply_for(&request("packet_q", packet_hash.as_bytes()), &query).await),
        Value::F64(87.5)
    );
    assert_eq!(
        decode(reply_for(&request("packet_rssi", &[0x24; PACKET_HASH_LEN]), &query).await),
        Value::Nil
    );
    assert_eq!(
        decode(reply_for(&request("packet_rssi", &[0x42; 16]), &query).await),
        Value::Nil
    );

    let partial = StubQuery {
        packet_phy: Some((
            packet_hash,
            PacketPhyStats {
                rssi: Some(RssiDbm::new(-82)),
                snr: None,
                quality: None,
            },
        )),
        ..query
    };
    assert_eq!(
        decode(reply_for(&request("packet_snr", packet_hash.as_bytes()), &partial).await),
        Value::Nil
    );
}

#[futures_test::test]
async fn a_msgpack_rate_table_projects_complete_rns_rows_in_seconds() {
    let query = StubQuery {
        links: 0,
        packet_phy: None,
        rates: std::vec![
            AnnounceRateSnapshot {
                destination: DestinationHash::new([0x41; 16]),
                last_allowed_announce_at: prns_core::engine::InstantMillis(1_500),
                blocked_until: prns_core::engine::InstantMillis(0),
                rate_violations: 1,
                observed_at: std::vec![
                    prns_core::engine::InstantMillis(1_000),
                    prns_core::engine::InstantMillis(1_500),
                ],
            },
            AnnounceRateSnapshot {
                destination: DestinationHash::new([0x42; 16]),
                last_allowed_announce_at: prns_core::engine::InstantMillis(2_000),
                blocked_until: prns_core::engine::InstantMillis(4_250),
                rate_violations: 4,
                observed_at: std::vec![
                    prns_core::engine::InstantMillis(2_000),
                    prns_core::engine::InstantMillis(2_500),
                ],
            },
        ],
        routes: std::vec![],
        interfaces: std::vec![],
    };
    let reply = reply_for(b"\x81\xa3get\xaarate_table", &query).await;
    let decoded = rmpv::decode::read_value(&mut std::io::Cursor::new(reply)).unwrap();

    assert_eq!(
        decoded,
        Value::Array(std::vec![
            Value::Map(std::vec![
                ("hash".into(), Value::Binary(std::vec![0x41; 16])),
                ("last".into(), Value::F64(1.5)),
                ("rate_violations".into(), Value::from(1u64)),
                ("blocked_until".into(), Value::from(0)),
                (
                    "timestamps".into(),
                    Value::Array(std::vec![Value::F64(1.0), Value::F64(1.5)]),
                ),
            ]),
            Value::Map(std::vec![
                ("hash".into(), Value::Binary(std::vec![0x42; 16])),
                ("last".into(), Value::F64(2.0)),
                ("rate_violations".into(), Value::from(4u64)),
                ("blocked_until".into(), Value::F64(4.25)),
                (
                    "timestamps".into(),
                    Value::Array(std::vec![Value::F64(2.0), Value::F64(2.5)]),
                ),
            ]),
        ])
    );
}
