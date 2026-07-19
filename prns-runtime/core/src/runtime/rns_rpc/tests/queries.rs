use super::*;

#[futures_test::test]
async fn the_set_answers_phy_stats_none_timeout_default_and_a_real_link_count() {
    let query = StubQuery {
        links: 2,
        packet_phy: None,
        rates: std::vec![],
        routes: std::vec![],
        interfaces: std::vec![],
    };
    let rssi = b"\x80\x04\x95...{'get': 'packet_rssi'}";
    assert_eq!(reply_for(rssi, &query).await, b"N.");
    let timeout = b"{'get': 'first_hop_timeout'}";
    assert_eq!(reply_for(timeout, &query).await, b"I6\n.");
    let links = b"{'get': 'link_count'}";
    assert_eq!(reply_for(links, &query).await, b"I2\n.");
    let path_table = b"{'get': 'path_table'}";
    assert_eq!(reply_for(path_table, &query).await, b"].");
    let rate_table = b"{'get': 'rate_table'}";
    assert_eq!(reply_for(rate_table, &query).await, b"].");
    let blackholes = b"{'get': 'blackholed_identities'}";
    assert_eq!(reply_for(blackholes, &query).await, b"}.");
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
