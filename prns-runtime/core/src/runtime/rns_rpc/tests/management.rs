use super::*;

#[futures_test::test]
async fn rns_1_4_2_management_verbs_get_typed_conservative_replies() {
    let query = StubQuery {
        links: 0,
        packet_phy: None,
        rates: std::vec![],
        routes: std::vec![],
        interfaces: std::vec![],
    };

    let is_blackholed = msgpack_request(std::vec![
        ("get", Value::from("is_blackholed")),
        ("identity_hash", Value::Binary(std::vec![0; 16])),
    ]);
    assert_eq!(
        reply_for(&is_blackholed, &query).await,
        b"\xc2",
        "an unknown identity is not blackholed"
    );
    let drop_path = msgpack_request(std::vec![
        ("drop", Value::from("path")),
        ("destination_hash", Value::Binary(std::vec![0; 16])),
    ]);
    assert_eq!(
        reply_for(&drop_path, &query).await,
        b"\xc2",
        "unknown path drops report false"
    );
    let drop_all_via = msgpack_request(std::vec![
        ("drop", Value::from("all_via")),
        ("destination_hash", Value::Binary(std::vec![0; 16])),
    ]);
    assert_eq!(
        reply_for(&drop_all_via, &query).await,
        b"\x00",
        "no routes were dropped via an unknown transport"
    );
    assert_eq!(
        reply_for(b"\x81\xa4drop\xafannounce_queues", &query).await,
        b"\xc0",
        "RNS drop_announce_queues returns None"
    );
    let blackhole = msgpack_request(std::vec![
        ("blackhole_identity", Value::Binary(std::vec![0; 16])),
        ("until", Value::Nil),
        ("reason", Value::Nil),
    ]);
    assert_eq!(
        reply_for(&blackhole, &query).await,
        b"\xc2",
        "failed blackhole writes report false"
    );
    let destination_used = msgpack_request(std::vec![
        ("destination_data", Value::from("used")),
        ("destination_hash", Value::Binary(std::vec![0; 16])),
    ]);
    assert_eq!(
        reply_for(&destination_used, &query).await,
        b"\xc2",
        "an unknown destination cannot record use"
    );
    let legacy_drop_path = legacy_string_request("drop", "path");
    assert_eq!(
        reply_for(&legacy_drop_path, &query).await,
        b"I00\n.",
        "legacy clients get the same false value in pickle"
    );
}

#[futures_test::test]
async fn rns_1_4_2_destination_data_projects_every_typed_retention_outcome() {
    let query = StubQuery {
        links: 0,
        packet_phy: None,
        rates: std::vec![],
        routes: std::vec![],
        interfaces: std::vec![],
    };
    let destination = DestinationHash::new([0x62; 16]);
    let request = |operation| {
        msgpack_request(vec![
            ("destination_data", Value::from(operation)),
            (
                "destination_hash",
                Value::Binary(destination.as_bytes().to_vec()),
            ),
        ])
    };
    let (calls, recorded) = std::sync::mpsc::channel();
    let mut retention = StubRetention {
        calls,
        mark_used: Ok(MarkDestinationUsedOutcome::NotFound),
        retain_destination: Ok(RetainDestinationOutcome::NotFound),
        release_destination: Ok(ReleaseDestinationOutcome::NotFound),
        retain_identity: Ok(RetainIdentityOutcome {
            newly_retained_destination_count: 0,
            already_retained_destination_count: 0,
        }),
    };

    for (outcome, expected) in [
        (Ok(MarkDestinationUsedOutcome::Recorded), true),
        (Ok(MarkDestinationUsedOutcome::Refreshed), true),
        (Ok(MarkDestinationUsedOutcome::Retained), false),
        (Ok(MarkDestinationUsedOutcome::NotFound), false),
        (
            Err(DestinationIdentityRetentionControlError::NodeStopped),
            false,
        ),
    ] {
        retention.mark_used = outcome;
        let expected_reply: &[u8] = if expected { b"\xc3" } else { b"\xc2" };
        assert_eq!(
            reply_for_with_retention(&request("used"), &query, &retention).await,
            expected_reply,
        );
        assert_eq!(
            recorded.recv().ok(),
            Some(RetentionCapabilityCall::MarkUsed(destination)),
        );
    }

    for (outcome, expected) in [
        (Ok(RetainDestinationOutcome::Retained), true),
        (Ok(RetainDestinationOutcome::AlreadyRetained), true),
        (Ok(RetainDestinationOutcome::NotFound), false),
        (Err(DestinationIdentityRetentionControlError::Busy), false),
    ] {
        retention.retain_destination = outcome;
        let expected_reply: &[u8] = if expected { b"\xc3" } else { b"\xc2" };
        assert_eq!(
            reply_for_with_retention(&request("retain"), &query, &retention).await,
            expected_reply,
        );
        assert_eq!(
            recorded.recv().ok(),
            Some(RetentionCapabilityCall::RetainDestination(destination)),
        );
    }

    for (outcome, expected) in [
        (Ok(ReleaseDestinationOutcome::Released), true),
        (Ok(ReleaseDestinationOutcome::UseRecorded), true),
        (Ok(ReleaseDestinationOutcome::UseRefreshed), true),
        (Ok(ReleaseDestinationOutcome::NotFound), false),
        (
            Err(DestinationIdentityRetentionControlError::NodeStopped),
            false,
        ),
    ] {
        retention.release_destination = outcome;
        let expected_reply: &[u8] = if expected { b"\xc3" } else { b"\xc2" };
        assert_eq!(
            reply_for_with_retention(&request("unretain"), &query, &retention).await,
            expected_reply,
        );
        assert_eq!(
            recorded.recv().ok(),
            Some(RetentionCapabilityCall::ReleaseDestination(destination)),
        );
    }
}

#[futures_test::test]
async fn rns_1_4_2_identity_retention_is_true_for_any_matching_destination() {
    let query = StubQuery {
        links: 0,
        packet_phy: None,
        rates: std::vec![],
        routes: std::vec![],
        interfaces: std::vec![],
    };
    let identity = IdentityHash::new([0x72; 16]);
    let request = msgpack_request(vec![
        ("identity_data", Value::from("retain")),
        ("identity_hash", Value::Binary(identity.as_bytes().to_vec())),
    ]);
    let (calls, recorded) = std::sync::mpsc::channel();
    let mut retention = StubRetention {
        calls,
        mark_used: Ok(MarkDestinationUsedOutcome::NotFound),
        retain_destination: Ok(RetainDestinationOutcome::NotFound),
        release_destination: Ok(ReleaseDestinationOutcome::NotFound),
        retain_identity: Ok(RetainIdentityOutcome {
            newly_retained_destination_count: 0,
            already_retained_destination_count: 0,
        }),
    };

    for (outcome, expected) in [
        (
            Ok(RetainIdentityOutcome {
                newly_retained_destination_count: 1,
                already_retained_destination_count: 0,
            }),
            true,
        ),
        (
            Ok(RetainIdentityOutcome {
                newly_retained_destination_count: 0,
                already_retained_destination_count: 1,
            }),
            true,
        ),
        (
            Ok(RetainIdentityOutcome {
                newly_retained_destination_count: 0,
                already_retained_destination_count: 0,
            }),
            false,
        ),
        (Err(DestinationIdentityRetentionControlError::Busy), false),
    ] {
        retention.retain_identity = outcome;
        let expected_reply: &[u8] = if expected { b"\xc3" } else { b"\xc2" };
        assert_eq!(
            reply_for_with_retention(&request, &query, &retention).await,
            expected_reply,
        );
        assert_eq!(
            recorded.recv().ok(),
            Some(RetentionCapabilityCall::RetainIdentity(identity)),
        );
    }
}

#[futures_test::test]
async fn rns_1_4_2_blackhole_reads_delegate_and_project_the_live_table() {
    let query = StubQuery {
        links: 0,
        packet_phy: None,
        rates: std::vec![],
        routes: std::vec![],
        interfaces: std::vec![],
    };
    let identity = IdentityHash::new([0x31; 16]);
    let source = IdentityHash::new([0x41; 16]);
    let entries = vec![BlackholedIdentity {
        identity,
        source,
        expiry: BlackholeExpiry::At(prns_core::units::InstantMillis(123_500)),
        reason: Some(String::from("operator")),
    }];
    let (calls, recorded) = std::sync::mpsc::channel();
    let blackholes = StubBlackholes {
        calls,
        entries: Ok(entries),
        is_blackholed: Ok(true),
        blackhole: Ok(BlackholeIdentityOutcome::AlreadyPresent),
        unblackhole: Ok(UnblackholeIdentityOutcome::NotFound),
    };

    let table = msgpack_request(vec![("get", Value::from("blackholed_identities"))]);
    let table_reply = reply_for_with_blackholes(&table, &query, &blackholes).await;
    assert_eq!(
        rmpv::decode::read_value(&mut std::io::Cursor::new(table_reply)).unwrap(),
        Value::Map(vec![(
            Value::Binary(identity.as_bytes().to_vec()),
            Value::Map(vec![
                (
                    Value::from("source"),
                    Value::Binary(source.as_bytes().to_vec()),
                ),
                (Value::from("until"), Value::F64(123.5)),
                (Value::from("reason"), Value::from("operator")),
            ]),
        )])
    );
    assert_eq!(recorded.recv().ok(), Some(BlackholeCapabilityCall::ReadAll));

    let checking = msgpack_request(vec![
        ("get", Value::from("is_blackholed")),
        ("identity_hash", Value::Binary(identity.as_bytes().to_vec())),
    ]);
    assert_eq!(
        reply_for_with_blackholes(&checking, &query, &blackholes).await,
        b"\xc3"
    );
    assert_eq!(
        recorded.recv().ok(),
        Some(BlackholeCapabilityCall::IsBlackholed(identity))
    );
}

#[futures_test::test]
async fn rns_1_4_2_blackhole_writes_delegate_source_and_project_tri_state_replies() {
    let query = StubQuery {
        links: 0,
        packet_phy: None,
        rates: std::vec![],
        routes: std::vec![],
        interfaces: std::vec![],
    };
    let identity = IdentityHash::new([0x31; 16]);
    let request = msgpack_request(vec![
        (
            "blackhole_identity",
            Value::Binary(identity.as_bytes().to_vec()),
        ),
        ("until", Value::F64(123.4567)),
        ("reason", Value::from("operator")),
    ]);
    let (calls, recorded) = std::sync::mpsc::channel();
    let mut blackholes = StubBlackholes {
        calls,
        entries: Ok(vec![]),
        is_blackholed: Ok(false),
        blackhole: Ok(BlackholeIdentityOutcome::Added),
        unblackhole: Ok(UnblackholeIdentityOutcome::Removed),
    };

    assert_eq!(
        reply_for_with_blackholes(&request, &query, &blackholes).await,
        b"\xc3"
    );
    assert_eq!(
        recorded.recv().ok(),
        Some(BlackholeCapabilityCall::Blackhole(BlackholedIdentity {
            identity,
            source: TEST_TRANSPORT_IDENTITY_HASH,
            expiry: BlackholeExpiry::At(prns_core::units::InstantMillis(123_456)),
            reason: Some(String::from("operator")),
        }))
    );

    blackholes.blackhole = Ok(BlackholeIdentityOutcome::AlreadyPresent);
    assert_eq!(
        reply_for_with_blackholes(&request, &query, &blackholes).await,
        b"\xc0"
    );
    assert!(matches!(
        recorded.recv().ok(),
        Some(BlackholeCapabilityCall::Blackhole(_))
    ));

    blackholes.blackhole = Err(IdentityBlackholeControlError::DurabilityFailed);
    assert_eq!(
        reply_for_with_blackholes(&request, &query, &blackholes).await,
        b"\xc2"
    );
    assert!(matches!(
        recorded.recv().ok(),
        Some(BlackholeCapabilityCall::Blackhole(_))
    ));

    let request = msgpack_request(vec![(
        "unblackhole_identity",
        Value::Binary(identity.as_bytes().to_vec()),
    )]);
    assert_eq!(
        reply_for_with_blackholes(&request, &query, &blackholes).await,
        b"\xc3"
    );
    assert_eq!(
        recorded.recv().ok(),
        Some(BlackholeCapabilityCall::Unblackhole(identity))
    );

    blackholes.unblackhole = Ok(UnblackholeIdentityOutcome::NotFound);
    assert_eq!(
        reply_for_with_blackholes(&request, &query, &blackholes).await,
        b"\xc0"
    );
    assert_eq!(
        recorded.recv().ok(),
        Some(BlackholeCapabilityCall::Unblackhole(identity))
    );

    blackholes.unblackhole = Err(IdentityBlackholeControlError::NodeStopped);
    assert_eq!(
        reply_for_with_blackholes(&request, &query, &blackholes).await,
        b"\xc2"
    );
    assert_eq!(
        recorded.recv().ok(),
        Some(BlackholeCapabilityCall::Unblackhole(identity))
    );
}

#[futures_test::test]
async fn rns_1_4_2_drop_verbs_delegate_typed_arguments_and_project_reference_replies() {
    let query = StubQuery {
        links: 0,
        packet_phy: None,
        rates: std::vec![],
        routes: std::vec![],
        interfaces: std::vec![],
    };
    let (calls_tx, calls_rx) = std::sync::mpsc::channel();
    let control = StubRoutingControl {
        calls: calls_tx,
        drop_route: Ok(DropRouteOutcome::Dropped),
        drop_routes_via: Ok(DropRoutesViaOutcome { dropped_routes: 3 }),
        clear_announce_queues: Ok(ClearAnnounceQueuesOutcome {
            dropped_announces: 5,
        }),
    };
    let destination = DestinationHash::new([0xAB; 16]);
    let transport = TransportId::new([0xCD; 16]);

    let drop_path = msgpack_request(std::vec![
        ("drop", Value::from("path")),
        (
            "destination_hash",
            Value::Binary(destination.as_bytes().to_vec()),
        ),
    ]);
    assert_eq!(
        reply_for_with_control(&drop_path, &query, &control).await,
        b"\xc3"
    );
    assert_eq!(
        calls_rx.recv().ok(),
        Some(RoutingControlCall::DropRoute(destination))
    );

    let drop_all_via = msgpack_request(std::vec![
        ("drop", Value::from("all_via")),
        (
            "destination_hash",
            Value::Binary(transport.as_bytes().to_vec()),
        ),
    ]);
    assert_eq!(
        reply_for_with_control(&drop_all_via, &query, &control).await,
        b"\x03"
    );
    assert_eq!(
        calls_rx.recv().ok(),
        Some(RoutingControlCall::DropRoutesVia(transport))
    );

    assert_eq!(
        reply_for_with_control(b"\x81\xa4drop\xafannounce_queues", &query, &control).await,
        b"\xc0"
    );
    assert_eq!(
        calls_rx.recv().ok(),
        Some(RoutingControlCall::ClearAnnounceQueues)
    );
}
