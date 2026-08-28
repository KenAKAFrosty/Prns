use super::advertisement::{
    AD_MANUFACTURER_SPECIFIC, EXPERIMENTAL_ROLE_PERIPHERAL_ONLY, EXPERIMENTAL_ROLE_VERSION,
    EXPERIMENTAL_ROLE_VERSION_MIN,
};
use super::handshake::{CONTROL_CLOSE, CONTROL_HELLO};
use super::*;

fn identity(byte: u8) -> BleIdentity {
    BleIdentity::new([byte; 16])
}

fn caps(l2cap: Option<u16>) -> LinkCapabilities {
    LinkCapabilities {
        l2cap: l2cap.and_then(Psm::new),
        link_mtu: 247,
    }
}

fn mac() -> Endpoint {
    Endpoint::CoreBluetooth(AppleHost::MacOs)
}

fn ios() -> Endpoint {
    Endpoint::CoreBluetooth(AppleHost::Ios)
}

fn ipad() -> Endpoint {
    Endpoint::CoreBluetooth(AppleHost::IpadOs)
}

fn linux() -> Endpoint {
    Endpoint::BlueZ(BlueZHost::Linux)
}

fn android() -> Endpoint {
    Endpoint::Android(AndroidHost::Android)
}

fn nrf() -> Endpoint {
    Endpoint::Nrf52(Nrf52Host::Nrf52)
}

fn esp32() -> Endpoint {
    Endpoint::Esp32(Esp32Host::Esp32)
}

#[test]
fn psm_admits_only_the_le_dynamic_range() {
    assert!(Psm::new(0x0080).is_some());
    assert!(Psm::new(0x00FF).is_some());
    assert!(Psm::new(0x007F).is_none());
    assert!(Psm::new(0x0100).is_none());
}

#[test]
fn an_advertisement_carries_the_shared_reticulum_ble_service() {
    let mut buf = [0u8; MAX_ADVERTISEMENT_LEN];
    let len = encode_advertisement(
        &mut buf,
        BleRoleCapabilities::DualRole,
        default_group_tag(),
    )
    .unwrap();
    assert_eq!(len, MAX_ADVERTISEMENT_LEN);
    assert!(contains_service(&buf[..len]));
    assert_eq!(
        columba_role_capabilities(&buf[..len]),
        Some(BleRoleCapabilities::DualRole)
    );
    assert_eq!(advertisement_group_tag(&buf[..len]), default_group_tag());
    assert!(!contains_service(&[]));
    assert!(!contains_service(&[0x02, 0x01, 0x06]));
}

#[test]
fn a_peripheral_only_advertisement_exposes_its_columba_role_constraint() {
    let mut buf = [0u8; MAX_ADVERTISEMENT_LEN];
    let len = encode_advertisement(
        &mut buf,
        BleRoleCapabilities::PeripheralOnly,
        default_group_tag(),
    )
    .unwrap();

    assert_eq!(
        columba_role_capabilities(&buf[..len]),
        Some(BleRoleCapabilities::PeripheralOnly)
    );
}

#[test]
fn discovery_group_tags_partition_advertisements() {
    let mut leg_a = [0u8; MAX_ADVERTISEMENT_LEN];
    let mut leg_b = [0u8; MAX_ADVERTISEMENT_LEN];
    let tag_a = group_tag(b"mt-leg-a");
    let tag_b = group_tag(b"mt-leg-b");
    let len_a = encode_advertisement(&mut leg_a, BleRoleCapabilities::DualRole, tag_a).unwrap();
    let len_b = encode_advertisement(&mut leg_b, BleRoleCapabilities::DualRole, tag_b).unwrap();

    assert!(discovery_groups_match(tag_a, &leg_a[..len_a]));
    assert!(!discovery_groups_match(tag_a, &leg_b[..len_b]));
    assert!(discovery_groups_match(tag_b, &leg_b[..len_b]));
}

#[test]
fn legacy_v3_advertisements_map_to_the_default_discovery_group() {
    let legacy = [
        2,
        0x01,
        0x06,
        5,
        AD_MANUFACTURER_SPECIFIC,
        0xff,
        0xff,
        EXPERIMENTAL_ROLE_VERSION_MIN,
        0,
    ];
    assert_eq!(advertisement_group_tag(&legacy), default_group_tag());
    assert!(discovery_groups_match(default_group_tag(), &legacy));
    assert!(!discovery_groups_match(group_tag(b"mt-leg-a"), &legacy));
}

#[test]
fn an_unrelated_manufacturer_field_does_not_hide_columba_capabilities() {
    let advertisement = [
        4,
        AD_MANUFACTURER_SPECIFIC,
        0x34,
        0x12,
        0,
        5,
        AD_MANUFACTURER_SPECIFIC,
        0xff,
        0xff,
        EXPERIMENTAL_ROLE_VERSION,
        EXPERIMENTAL_ROLE_PERIPHERAL_ONLY,
    ];

    assert_eq!(
        columba_role_capabilities(&advertisement),
        Some(BleRoleCapabilities::PeripheralOnly)
    );
}

#[test]
fn lower_dual_role_address_dials_and_higher_address_accepts() {
    let lower = BleAddress::new([0, 1, 2, 3, 4, 5]);
    let higher = BleAddress::new([0, 1, 2, 3, 4, 6]);

    assert_eq!(
        columba_connection_role(
            lower,
            BleRoleCapabilities::DualRole,
            higher,
            BleRoleCapabilities::DualRole,
        ),
        ColumbaConnectionRole::Dial
    );
    assert_eq!(
        columba_connection_role(
            higher,
            BleRoleCapabilities::DualRole,
            lower,
            BleRoleCapabilities::DualRole,
        ),
        ColumbaConnectionRole::Accept
    );
}

#[test]
fn hci_addresses_compare_in_display_order() {
    assert_eq!(
        BleAddress::from_hci_bytes([0x17, 0x27, 0x0c, 0x6a, 0x46, 0xfd]),
        BleAddress::new([0xfd, 0x46, 0x6a, 0x0c, 0x27, 0x17])
    );
}

#[test]
fn dual_role_peer_dials_a_peripheral_only_peer() {
    assert_eq!(
        columba_connection_role(
            BleAddress::new([1; 6]),
            BleRoleCapabilities::DualRole,
            BleAddress::new([0; 6]),
            BleRoleCapabilities::PeripheralOnly,
        ),
        ColumbaConnectionRole::Dial
    );
    assert_eq!(
        columba_connection_role(
            BleAddress::new([0; 6]),
            BleRoleCapabilities::PeripheralOnly,
            BleAddress::new([1; 6]),
            BleRoleCapabilities::DualRole,
        ),
        ColumbaConnectionRole::Accept
    );
}

#[test]
fn mac_and_linux_open_when_linux_opens() {
    assert_eq!(
        l2cap_arrangement(mac(), linux()),
        L2capArrangement::Opens(linux())
    );
    assert_eq!(
        l2cap_arrangement(linux(), mac()),
        L2capArrangement::Opens(linux())
    );
}

#[test]
fn mac_and_android_only_open_when_android_opens() {
    assert_eq!(
        l2cap_arrangement(mac(), android()),
        L2capArrangement::Opens(android())
    );
    assert_eq!(
        l2cap_arrangement(android(), mac()),
        L2capArrangement::Opens(android())
    );
}

#[test]
fn apple_mobile_and_linux_stay_on_the_gatt_floor() {
    assert_eq!(
        l2cap_arrangement(ios(), linux()),
        L2capArrangement::GattOnly
    );
    assert_eq!(
        l2cap_arrangement(linux(), ios()),
        L2capArrangement::GattOnly
    );
    assert_eq!(
        l2cap_arrangement(ipad(), linux()),
        L2capArrangement::GattOnly
    );
    assert_eq!(
        l2cap_arrangement(linux(), ipad()),
        L2capArrangement::GattOnly
    );
}

#[test]
fn apple_mobile_and_android_only_open_when_apple_mobile_opens() {
    assert_eq!(
        l2cap_arrangement(ios(), android()),
        L2capArrangement::Opens(ios())
    );
    assert_eq!(
        l2cap_arrangement(android(), ios()),
        L2capArrangement::Opens(ios())
    );
    assert_eq!(
        l2cap_arrangement(ipad(), android()),
        L2capArrangement::Opens(ipad())
    );
    assert_eq!(
        l2cap_arrangement(android(), ipad()),
        L2capArrangement::Opens(ipad())
    );
}

#[test]
fn two_apple_devices_stay_on_the_gatt_floor() {
    assert_eq!(l2cap_arrangement(ios(), mac()), L2capArrangement::GattOnly);
    assert_eq!(l2cap_arrangement(mac(), ios()), L2capArrangement::GattOnly);
    assert_eq!(l2cap_arrangement(ios(), ios()), L2capArrangement::GattOnly);
}

#[test]
fn bluez_and_android_either_open_the_fast_lane() {
    let arr = l2cap_arrangement(linux(), android());
    assert_eq!(arr, L2capArrangement::EitherOpens);
    assert_eq!(
        l2cap_arrangement(android(), linux()),
        L2capArrangement::EitherOpens
    );

    assert_eq!(
        l2cap_plan(
            arr,
            HandshakeRole::Dialer,
            linux(),
            &caps(Some(0x0083)),
            &caps(Some(0x0080)),
        ),
        L2capPlan::Open {
            psm: Psm::new(0x0080).unwrap()
        }
    );
    assert_eq!(
        l2cap_plan(
            arr,
            HandshakeRole::Listener,
            android(),
            &caps(Some(0x0080)),
            &caps(Some(0x0083)),
        ),
        L2capPlan::Accept
    );
}

#[test]
fn the_nrf_either_opens_the_fast_lane_with_bluez_and_android() {
    assert_eq!(
        l2cap_arrangement(linux(), nrf()),
        L2capArrangement::EitherOpens
    );
    assert_eq!(
        l2cap_arrangement(nrf(), linux()),
        L2capArrangement::EitherOpens
    );
    assert_eq!(
        l2cap_arrangement(android(), nrf()),
        L2capArrangement::EitherOpens
    );
    assert_eq!(
        l2cap_arrangement(nrf(), android()),
        L2capArrangement::EitherOpens
    );

    let arr = l2cap_arrangement(nrf(), linux());
    assert_eq!(
        l2cap_plan(
            arr,
            HandshakeRole::Dialer,
            nrf(),
            &caps(Some(0x0080)),
            &caps(Some(0x0083)),
        ),
        L2capPlan::Open {
            psm: Psm::new(0x0083).unwrap()
        }
    );
    assert_eq!(
        l2cap_plan(
            arr,
            HandshakeRole::Listener,
            nrf(),
            &caps(Some(0x0080)),
            &caps(Some(0x0083)),
        ),
        L2capPlan::Accept
    );
}

#[test]
fn the_esp32_either_opens_the_fast_lane_with_its_peers() {
    assert_eq!(
        l2cap_arrangement(esp32(), esp32()),
        L2capArrangement::EitherOpens
    );
    assert_eq!(
        l2cap_arrangement(esp32(), nrf()),
        L2capArrangement::EitherOpens
    );
    assert_eq!(
        l2cap_arrangement(nrf(), esp32()),
        L2capArrangement::EitherOpens
    );
    assert_eq!(
        l2cap_arrangement(linux(), esp32()),
        L2capArrangement::EitherOpens
    );
    assert_eq!(
        l2cap_arrangement(esp32(), linux()),
        L2capArrangement::EitherOpens
    );
    assert_eq!(
        l2cap_arrangement(android(), esp32()),
        L2capArrangement::EitherOpens
    );
    assert_eq!(
        l2cap_arrangement(esp32(), android()),
        L2capArrangement::EitherOpens
    );

    let arr = l2cap_arrangement(esp32(), esp32());
    assert_eq!(
        l2cap_plan(
            arr,
            HandshakeRole::Dialer,
            esp32(),
            &caps(Some(0x0080)),
            &caps(Some(0x0080)),
        ),
        L2capPlan::Open {
            psm: Psm::new(0x0080).unwrap()
        }
    );
    assert_eq!(
        l2cap_plan(
            arr,
            HandshakeRole::Listener,
            esp32(),
            &caps(Some(0x0080)),
            &caps(Some(0x0080)),
        ),
        L2capPlan::Accept
    );
}

#[test]
fn the_esp32_stays_on_the_gatt_floor_with_windows_and_apple() {
    assert_eq!(
        l2cap_arrangement(esp32(), Endpoint::WinRt(WinRtHost::Windows)),
        L2capArrangement::GattOnly
    );
    assert_eq!(
        l2cap_arrangement(esp32(), mac()),
        L2capArrangement::GattOnly
    );
}

#[test]
fn an_untested_pair_falls_to_the_gatt_floor() {
    assert_eq!(l2cap_arrangement(mac(), mac()), L2capArrangement::GattOnly);
    assert_eq!(
        l2cap_arrangement(android(), android()),
        L2capArrangement::GattOnly
    );
    assert_eq!(
        l2cap_arrangement(mac(), Endpoint::WinRt(WinRtHost::Windows)),
        L2capArrangement::GattOnly
    );
}

#[test]
fn the_arrangement_table_is_order_independent() {
    let endpoints = [
        mac(),
        linux(),
        android(),
        Endpoint::CoreBluetooth(AppleHost::Ios),
        Endpoint::Esp32(Esp32Host::Esp32),
        Endpoint::WinRt(WinRtHost::Windows),
    ];
    for &a in &endpoints {
        for &b in &endpoints {
            assert_eq!(l2cap_arrangement(a, b), l2cap_arrangement(b, a));
        }
    }
}

#[test]
fn opens_always_names_one_of_the_pair() {
    let endpoints = [
        mac(),
        linux(),
        android(),
        Endpoint::CoreBluetooth(AppleHost::Ios),
        Endpoint::Esp32(Esp32Host::Esp32),
        Endpoint::WinRt(WinRtHost::Windows),
    ];
    for &a in &endpoints {
        for &b in &endpoints {
            if let L2capArrangement::Opens(opener) = l2cap_arrangement(a, b) {
                assert!(opener == a || opener == b);
            }
        }
    }
}

#[test]
fn both_ends_keep_the_same_connection_for_an_opens_pair() {
    let arr = l2cap_arrangement(mac(), android());
    let mac_id = identity(1);
    let android_id = identity(2);

    let mac_dials_mac_view = is_keeper(arr, HandshakeRole::Dialer, mac_id, mac(), android_id);
    let mac_dials_android_view =
        is_keeper(arr, HandshakeRole::Listener, android_id, android(), mac_id);
    assert_eq!(mac_dials_mac_view, mac_dials_android_view);
    assert!(!mac_dials_mac_view);

    let android_dials_mac_view = is_keeper(arr, HandshakeRole::Listener, mac_id, mac(), android_id);
    let android_dials_android_view =
        is_keeper(arr, HandshakeRole::Dialer, android_id, android(), mac_id);
    assert_eq!(android_dials_mac_view, android_dials_android_view);
    assert!(android_dials_mac_view);
}

#[test]
fn both_ends_keep_the_same_connection_for_an_either_opens_pair() {
    let arr = L2capArrangement::EitherOpens;
    let low = identity(1);
    let high = identity(9);

    let low_dials_low_view = is_keeper(arr, HandshakeRole::Dialer, low, mac(), high);
    let low_dials_high_view = is_keeper(arr, HandshakeRole::Listener, high, linux(), low);
    assert_eq!(low_dials_low_view, low_dials_high_view);
    assert!(low_dials_low_view);

    let high_dials_low_view = is_keeper(arr, HandshakeRole::Listener, low, mac(), high);
    let high_dials_high_view = is_keeper(arr, HandshakeRole::Dialer, high, linux(), low);
    assert_eq!(high_dials_low_view, high_dials_high_view);
    assert!(!high_dials_low_view);
}

#[test]
fn opens_arrangement_rejects_either_wrong_gatt_role() {
    let opens_android = l2cap_arrangement(mac(), android());
    assert!(needs_redial(
        opens_android,
        HandshakeRole::Listener,
        android()
    ));
    assert!(!needs_redial(
        opens_android,
        HandshakeRole::Dialer,
        android()
    ));
    assert!(!needs_redial(opens_android, HandshakeRole::Listener, mac()));
    assert!(needs_redial(opens_android, HandshakeRole::Dialer, mac()));

    let either = l2cap_arrangement(linux(), android());
    assert!(!needs_redial(either, HandshakeRole::Listener, linux()));
    assert!(!needs_redial(either, HandshakeRole::Dialer, linux()));
}

#[test]
fn either_opens_central_opens_and_peripheral_accepts() {
    let arr = L2capArrangement::EitherOpens;
    let mine = caps(Some(0x00c0));
    let theirs = caps(Some(0x0083));
    assert_eq!(
        l2cap_plan(arr, HandshakeRole::Dialer, mac(), &mine, &theirs),
        L2capPlan::Open {
            psm: Psm::new(0x0083).unwrap()
        }
    );
    assert_eq!(
        l2cap_plan(arr, HandshakeRole::Listener, mac(), &mine, &theirs),
        L2capPlan::Accept
    );
}

#[test]
fn opens_lets_only_the_named_side_open_and_only_as_central() {
    let arr = l2cap_arrangement(mac(), android());
    let android_caps = caps(Some(0x0080));
    let mac_caps = caps(Some(0x00c0));

    assert_eq!(
        l2cap_plan(
            arr,
            HandshakeRole::Dialer,
            android(),
            &android_caps,
            &mac_caps
        ),
        L2capPlan::Open {
            psm: Psm::new(0x00c0).unwrap()
        }
    );
    assert_eq!(
        l2cap_plan(
            arr,
            HandshakeRole::Listener,
            android(),
            &android_caps,
            &mac_caps
        ),
        L2capPlan::None
    );
    assert_eq!(
        l2cap_plan(
            arr,
            HandshakeRole::Listener,
            mac(),
            &mac_caps,
            &android_caps
        ),
        L2capPlan::Accept
    );
    assert_eq!(
        l2cap_plan(arr, HandshakeRole::Dialer, mac(), &mac_caps, &android_caps),
        L2capPlan::Accept
    );
}

#[test]
fn apple_mobile_and_bluez_never_plan_l2cap() {
    let apple_caps = caps(Some(0x00c0));
    let linux_caps = caps(Some(0x0083));

    assert_eq!(
        l2cap_plan(
            l2cap_arrangement(ios(), linux()),
            HandshakeRole::Dialer,
            ios(),
            &apple_caps,
            &linux_caps,
        ),
        L2capPlan::None
    );
    assert_eq!(
        l2cap_plan(
            l2cap_arrangement(linux(), ios()),
            HandshakeRole::Listener,
            linux(),
            &linux_caps,
            &apple_caps,
        ),
        L2capPlan::None
    );
    assert_eq!(
        l2cap_plan(
            l2cap_arrangement(ipad(), linux()),
            HandshakeRole::Dialer,
            ipad(),
            &apple_caps,
            &linux_caps,
        ),
        L2capPlan::None
    );
}

#[test]
fn ios_keeps_android_on_gatt_when_it_withholds_l2cap() {
    let ios_caps = caps(None);
    let android_caps = caps(Some(0x0080));

    assert_eq!(
        l2cap_plan(
            l2cap_arrangement(ios(), android()),
            HandshakeRole::Dialer,
            ios(),
            &ios_caps,
            &android_caps,
        ),
        L2capPlan::None
    );
}

#[test]
fn the_acceptor_stands_down_when_the_peer_has_no_listener() {
    let arr = l2cap_arrangement(mac(), android());
    assert_eq!(
        l2cap_plan(
            arr,
            HandshakeRole::Listener,
            mac(),
            &caps(Some(0x00c0)),
            &caps(None),
        ),
        L2capPlan::None
    );
}

#[test]
fn gatt_only_never_plans_l2cap() {
    assert_eq!(
        l2cap_plan(
            L2capArrangement::GattOnly,
            HandshakeRole::Dialer,
            mac(),
            &caps(Some(0x00c0)),
            &caps(Some(0x0080))
        ),
        L2capPlan::None
    );
}

#[test]
fn an_opener_whose_peer_has_no_listener_cannot_open() {
    let arr = L2capArrangement::EitherOpens;
    assert_eq!(
        l2cap_plan(
            arr,
            HandshakeRole::Dialer,
            mac(),
            &caps(Some(0x00c0)),
            &caps(None)
        ),
        L2capPlan::None
    );
}

#[test]
fn a_dialer_and_listener_settle_exchanging_endpoints_and_caps() {
    let dialer_local = LocalPeer {
        identity: identity(1),
        endpoint: mac(),
        capabilities: caps(Some(0x00c0)),
        group_tag: default_group_tag(),
    };
    let listener_local = LocalPeer {
        identity: identity(2),
        endpoint: android(),
        capabilities: caps(Some(0x0080)),
        group_tag: default_group_tag(),
    };
    let (mut dialer, opening) = Handshake::begin(HandshakeRole::Dialer, dialer_local, Some(-40));
    let (mut listener, silent) =
        Handshake::begin(HandshakeRole::Listener, listener_local, Some(-55));
    assert!(silent.is_none());

    let listener_reaction = listener.absorb(opening.unwrap());
    let dialer_reaction = dialer.absorb(listener_reaction.reply.unwrap());

    if let (HandshakeOutcome::Settled(at_listener), HandshakeOutcome::Settled(at_dialer)) =
        (listener_reaction.outcome, dialer_reaction.outcome)
    {
        assert_eq!(
            at_listener,
            EstablishedPeer {
                identity: identity(1),
                transport: EstablishedTransport::Native {
                    endpoint: mac(),
                    capabilities: caps(Some(0x00c0)),
                },
                peer_rssi: Some(-40),
            }
        );
        assert_eq!(
            at_dialer,
            EstablishedPeer {
                identity: identity(2),
                transport: EstablishedTransport::Native {
                    endpoint: android(),
                    capabilities: caps(Some(0x0080)),
                },
                peer_rssi: Some(-55),
            }
        );
    } else {
        panic!("expected both sides to settle");
    }
}

#[test]
fn a_self_connection_aborts_and_closes() {
    let local = LocalPeer {
        identity: identity(5),
        endpoint: mac(),
        capabilities: caps(Some(0x0090)),
        group_tag: default_group_tag(),
    };
    let (mut listener, _) = Handshake::begin(HandshakeRole::Listener, local, None);
    let reaction = listener.absorb(Control::Hello {
        identity: identity(5),
        endpoint: mac(),
        capabilities: caps(Some(0x0090)),
        peer_rssi: None,
        group_tag: Some(default_group_tag()),
    });
    assert_eq!(
        reaction.outcome,
        HandshakeOutcome::Aborted(CloseReason::SelfConnection)
    );
    assert_eq!(
        reaction.reply,
        Some(Control::Close {
            reason: CloseReason::SelfConnection
        })
    );
}

#[test]
fn a_payload_round_trips_through_fragmentation() {
    let payload: [u8; 500] = core::array::from_fn(|i| i as u8);
    let mut reassembler = Reassembler::<512>::new();
    let mut completed = None;
    for fragment in fragments_of(&payload, 64) {
        let mut buf = [0u8; 64];
        let len = fragment.encode(&mut buf).unwrap();
        let decoded = Fragment::decode(&buf[..len]).unwrap();
        if let Some(done) = reassembler.absorb(&decoded) {
            completed = Some(done.to_vec());
        }
    }
    assert_eq!(completed.as_deref(), Some(&payload[..]));
}

#[test]
fn a_small_payload_is_a_single_start_fragment() {
    let payload = [1u8, 2, 3];
    let mut fragments = fragments_of(&payload, 64);
    let only = fragments.next().unwrap();
    assert_eq!(only.kind, FragmentKind::Start);
    assert_eq!(only.total, 1);
    assert!(fragments.next().is_none());
}

#[test]
fn a_hello_round_trips_through_the_control_codec() {
    let hello = Control::Hello {
        identity: identity(7),
        endpoint: android(),
        capabilities: caps(Some(0x0081)),
        peer_rssi: Some(-63),
        group_tag: Some(default_group_tag()),
    };
    let mut buf = [0u8; CONTROL_MAX_LEN];
    let len = hello.encode(&mut buf).unwrap();
    assert_eq!(Control::decode(&buf[..len]), Some(hello));
}

#[test]
fn every_endpoint_round_trips_through_the_greeting() {
    for endpoint in [
        mac(),
        linux(),
        android(),
        Endpoint::CoreBluetooth(AppleHost::Ios),
        Endpoint::CoreBluetooth(AppleHost::IpadOs),
        Endpoint::WinRt(WinRtHost::Windows),
        Endpoint::Esp32(Esp32Host::Esp32),
    ] {
        let hello = Control::Hello {
            identity: identity(3),
            endpoint,
            capabilities: caps(None),
            peer_rssi: None,
            group_tag: Some(default_group_tag()),
        };
        let mut buf = [0u8; CONTROL_MAX_LEN];
        let len = hello.encode(&mut buf).unwrap();
        match Control::decode(&buf[..len]) {
            Some(Control::Hello { endpoint: decoded, .. }) => assert_eq!(decoded, endpoint),
            other => panic!("endpoint failed to round-trip: {other:?}"),
        }
    }
}

#[test]
fn a_greeting_without_the_trailing_rssi_byte_still_decodes() {
    // Legacy wire (no group tag): dropping the final RSSI byte still yields a greeting.
    let hello = Control::Hello {
        identity: identity(7),
        endpoint: mac(),
        capabilities: caps(Some(0x0081)),
        peer_rssi: Some(-63),
        group_tag: None,
    };
    let mut buf = [0u8; CONTROL_MAX_LEN];
    let len = hello.encode(&mut buf).unwrap();
    assert_eq!(len, CONTROL_LEGACY_GREETING_LEN);
    let trimmed = Control::decode(&buf[..len - 1]).unwrap();
    assert_eq!(
        trimmed,
        Control::Hello {
            identity: identity(7),
            endpoint: mac(),
            capabilities: caps(Some(0x0081)),
            peer_rssi: None,
            group_tag: None,
        }
    );
}

#[test]
fn a_legacy_hello_matches_the_default_discovery_group() {
    let dialer_local = LocalPeer {
        identity: identity(1),
        endpoint: mac(),
        capabilities: caps(Some(0x00c0)),
        group_tag: default_group_tag(),
    };
    let listener_local = LocalPeer {
        identity: identity(2),
        endpoint: android(),
        capabilities: caps(Some(0x0080)),
        group_tag: default_group_tag(),
    };
    let (mut dialer, opening) = Handshake::begin(HandshakeRole::Dialer, dialer_local, None);
    let (mut listener, _) = Handshake::begin(HandshakeRole::Listener, listener_local, None);
    let legacy = Control::Hello {
        identity: identity(1),
        endpoint: mac(),
        capabilities: caps(Some(0x00c0)),
        peer_rssi: None,
        group_tag: None,
    };
    let reaction = listener.absorb(legacy);
    assert!(matches!(reaction.outcome, HandshakeOutcome::Settled(_)));
    let welcome = reaction.reply.unwrap();
    assert!(matches!(
        dialer.absorb(welcome).outcome,
        HandshakeOutcome::Settled(_)
    ));
    let _ = opening;
}

#[test]
fn a_discovery_group_mismatch_aborts_handshake() {
    let dialer_local = LocalPeer {
        identity: identity(1),
        endpoint: mac(),
        capabilities: caps(Some(0x00c0)),
        group_tag: group_tag(b"mt-leg-a"),
    };
    let listener_local = LocalPeer {
        identity: identity(2),
        endpoint: android(),
        capabilities: caps(Some(0x0080)),
        group_tag: group_tag(b"mt-leg-b"),
    };
    let (_, opening) = Handshake::begin(HandshakeRole::Dialer, dialer_local, None);
    let (mut listener, _) = Handshake::begin(HandshakeRole::Listener, listener_local, None);
    let reaction = listener.absorb(opening.unwrap());
    assert_eq!(
        reaction.outcome,
        HandshakeOutcome::Aborted(CloseReason::Incompatible)
    );
    assert_eq!(
        reaction.reply,
        Some(Control::Close {
            reason: CloseReason::Incompatible
        })
    );
}

#[test]
fn a_gatt_only_welcome_round_trips_with_no_psm() {
    let welcome = Control::Welcome {
        identity: identity(9),
        endpoint: linux(),
        capabilities: LinkCapabilities {
            l2cap: None,
            link_mtu: 23,
        },
        peer_rssi: None,
        group_tag: Some(default_group_tag()),
    };
    let mut buf = [0u8; CONTROL_MAX_LEN];
    let len = welcome.encode(&mut buf).unwrap();
    let decoded = Control::decode(&buf[..len]).unwrap();
    assert_eq!(decoded, welcome);
    if let Control::Welcome { capabilities, .. } = decoded {
        assert!(capabilities.l2cap.is_none());
    }
}

#[test]
fn every_close_reason_round_trips() {
    for reason in [
        CloseReason::SelfConnection,
        CloseReason::DuplicateLink,
        CloseReason::Incompatible,
    ] {
        let close = Control::Close { reason };
        let mut buf = [0u8; CONTROL_MAX_LEN];
        let len = close.encode(&mut buf).unwrap();
        assert_eq!(Control::decode(&buf[..len]), Some(close));
    }
}

#[test]
fn the_control_codec_rejects_garbage() {
    assert_eq!(Control::decode(&[]), None);
    assert_eq!(Control::decode(&[0xFF]), None);
    assert_eq!(Control::decode(&[CONTROL_HELLO, 0x00]), None);
    assert_eq!(Control::decode(&[CONTROL_CLOSE, 0x00]), None);
}

#[test]
fn control_encode_refuses_a_short_buffer() {
    let hello = Control::Hello {
        identity: identity(1),
        endpoint: mac(),
        capabilities: caps(Some(0x0090)),
        peer_rssi: None,
        group_tag: Some(default_group_tag()),
    };
    let mut tiny = [0u8; 4];
    assert_eq!(hello.encode(&mut tiny), None);
}

#[test]
fn a_frame_round_trips_through_stream_framing() {
    let frame = [0x10u8, 0x20, 0x30, 0x40, 0x50];
    let mut wire = [0u8; 64];
    let n = encode_stream_frame(&frame, &mut wire).unwrap();
    assert_eq!(n, STREAM_FRAME_PREFIX_LEN + frame.len());
    let mut deframer = StreamDeframer::<256>::new();
    assert!(deframer.absorb(&wire[..n]));
    let mut out = [0u8; 64];
    let got = deframer.next_frame(&mut out).unwrap();
    assert_eq!(&out[..got], &frame);
    assert!(deframer.next_frame(&mut out).is_none());
}

#[test]
fn two_frames_in_one_chunk_pop_individually() {
    let mut wire = [0u8; 64];
    let mut total = 0;
    for frame in [&[1u8, 2, 3][..], &[9u8, 8][..]] {
        total += encode_stream_frame(frame, &mut wire[total..]).unwrap();
    }
    let mut deframer = StreamDeframer::<256>::new();
    assert!(deframer.absorb(&wire[..total]));
    let mut out = [0u8; 64];
    let a = deframer.next_frame(&mut out).unwrap();
    assert_eq!(&out[..a], &[1, 2, 3]);
    let b = deframer.next_frame(&mut out).unwrap();
    assert_eq!(&out[..b], &[9, 8]);
    assert!(deframer.next_frame(&mut out).is_none());
}

#[test]
fn a_frame_split_across_chunks_reassembles() {
    let frame = [7u8; 40];
    let mut wire = [0u8; 64];
    let n = encode_stream_frame(&frame, &mut wire).unwrap();
    let mut deframer = StreamDeframer::<256>::new();
    let mut out = [0u8; 64];
    assert!(deframer.absorb(&wire[..10]));
    assert!(deframer.next_frame(&mut out).is_none());
    assert!(deframer.absorb(&wire[10..n]));
    let got = deframer.next_frame(&mut out).unwrap();
    assert_eq!(&out[..got], &frame);
}

#[test]
fn the_stream_deframer_reports_overflow() {
    let mut deframer = StreamDeframer::<4>::new();
    assert!(deframer.absorb(&[1, 2, 3, 4]));
    assert!(!deframer.absorb(&[5]));
}
