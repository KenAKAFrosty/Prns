use super::*;

#[test]
fn display_sort_pins_usb_last_and_prioritizes_radios() {
    let mut cards: HVec<Card, 8> = HVec::new();
    for kind in [
        CardKind::Usb,
        CardKind::Wifi,
        CardKind::Tcp,
        CardKind::Ble,
        CardKind::EspNow,
        CardKind::LoRa,
    ] {
        let mut card = test_card("iface");
        card.kind = kind;
        let _ = cards.push(card);
    }

    sort_cards_for_display(&mut cards);

    let kinds: HVec<CardKind, 8> = cards.iter().map(|card| card.kind).collect();
    assert_eq!(
        kinds.as_slice(),
        &[
            CardKind::LoRa,
            CardKind::Wifi,
            CardKind::Ble,
            CardKind::EspNow,
            CardKind::Tcp,
            CardKind::Usb,
        ]
    );
}

#[test]
fn display_sort_gives_same_kind_cards_one_fixed_order() {
    let build = |ids: &[(u8, &'static str)]| {
        let mut cards: HVec<Card, 8> = HVec::new();
        for &(byte, label) in ids {
            let mut card = test_card(label);
            card.kind = CardKind::Tcp;
            card.id = InterfaceId::new([byte; 8]);
            let _ = cards.push(card);
        }
        cards
    };

    let peers = [(0x30, "TCP 3030"), (0x10, "TCP 1010"), (0x20, "TCP 2020")];
    let mut forward = build(&peers);
    let mut reversed = build(&[peers[2], peers[1], peers[0]]);

    sort_cards_for_display(&mut forward);
    sort_cards_for_display(&mut reversed);

    fn order(cards: &HVec<Card, 8>) -> HVec<&str, 8> {
        cards.iter().map(|card| card.label.as_str()).collect()
    }
    assert_eq!(
        order(&forward).as_slice(),
        &["TCP 1010", "TCP 2020", "TCP 3030"]
    );
    assert_eq!(order(&forward), order(&reversed));
}

#[test]
fn activity_tracker_stamps_age_when_a_card_changes() {
    let mut tracker = CardActivityTracker::<2>::new();
    let mut cards = [test_card("USB")];
    cards[0].connection = ConnectionState::Disconnected;

    tracker.update(&mut cards, 10);
    assert_eq!(cards[0].last_activity_secs, None);

    cards[0].rx_bytes = 16;
    tracker.update(&mut cards, 12);
    assert_eq!(cards[0].last_activity_secs, Some(0));

    tracker.update(&mut cards, 17);
    assert_eq!(cards[0].last_activity_secs, Some(5));
}

#[test]
fn supervisor_peer_rows_format_count_and_compact_peer_statuses() {
    let mut details = InterfaceMenuDetails::empty();
    details.push_info("AP", "Hopspot-EW53");
    let count = details.push_supervisor_peers([
        (
            InterfaceId::new([0, 0xab, 0xcd, 0, 0, 0, 0, 0]),
            ConnectionState::Connected,
        ),
        (
            InterfaceId::new([0, 0x12, 0x34, 0, 0, 0, 0, 0]),
            ConnectionState::Disconnected,
        ),
    ]);
    let rows = details.as_slice();

    assert_eq!(count, 2);
    assert_eq!(rows[0].text(), "AP Hopspot-EW53");
    assert_eq!(rows[1].text(), "Peers 2");
    assert_eq!(rows[2].text(), "P abcd Live");
    assert_eq!(rows[3].text(), "P 1234 Disc");
    assert_eq!(rows[2].kind(), InterfaceMenuDetailKind::Peer);
}

#[test]
fn egress_pressure_is_hidden_until_a_drop_is_observed() {
    let mut details = InterfaceMenuDetails::empty();
    details.push_egress_pressure(0);
    assert!(details.as_slice().is_empty());

    details.push_egress_pressure(23);
    assert_eq!(details.as_slice()[0].text(), "Egress drops 23");
}

#[test]
fn ingress_pressure_is_hidden_until_a_drop_is_observed() {
    let mut details = InterfaceMenuDetails::empty();
    details.push_ingress_pressure(0);
    assert!(details.as_slice().is_empty());

    details.push_ingress_pressure(17);
    assert_eq!(details.as_slice()[0].text(), "RX drops 17");
}

#[test]
fn bluetooth_recovery_is_one_compact_bounded_row() {
    let mut details = InterfaceMenuDetails::empty();
    details.push_bluetooth_recovery(BluetoothRecoveryMenuDetails {
        receive_pressure: 0,
        setup_failures: 0,
        transport_closures: 0,
    });
    assert!(details.as_slice().is_empty());

    details.push_bluetooth_recovery(BluetoothRecoveryMenuDetails {
        receive_pressure: 7,
        setup_failures: 12,
        transport_closures: u32::MAX,
    });
    assert_eq!(details.as_slice()[0].text(), "R7/S12/C99+");
}

#[test]
fn bluetooth_recovery_coexists_with_five_peer_rows() {
    let mut details = InterfaceMenuDetails::empty();
    let peers = (0..5).map(|index| {
        (
            InterfaceId::new([0, index, index, 0, 0, 0, 0, 0]),
            ConnectionState::Connected,
        )
    });
    details.push_supervisor_peers(peers);
    details.push_bluetooth_recovery(BluetoothRecoveryMenuDetails {
        receive_pressure: 1,
        setup_failures: 2,
        transport_closures: 3,
    });

    let rows = details.as_slice();
    assert_eq!(rows.len(), 7);
    assert_eq!(rows[6].text(), "R1/S2/C3");
}

#[test]
fn lora_profile_details_list_every_tune_parameter() {
    let mut details = InterfaceMenuDetails::empty();
    details.push_lora_profile(DEFAULT_915_PROFILE);
    let rows = details.as_slice();

    assert_eq!(rows.len(), 8);
    assert_eq!(rows[0].text(), "Reg US915");
    assert_eq!(rows[1].text(), "Rate MediumFast");
    assert_eq!(rows[2].text(), "SF 9");
    assert_eq!(rows[3].text(), "BW 250 kHz");
    assert_eq!(rows[4].text(), "CR 4/5");
    assert_eq!(rows[5].text(), "915.000 MHz");
    assert_eq!(rows[6].text(), "Pwr 22 dBm");
    assert_eq!(rows[7].text(), "Pre 18");
}

#[test]
fn lora_spectrum_details_keep_the_common_case_compact() {
    let mut details = InterfaceMenuDetails::empty();
    details.push_lora_spectrum(LoRaSpectrumMenuDetails {
        channel_busy_per_mille: 123,
        noise_floor_dbm: Some(-120),
        cca_threshold_dbm: Some(-109),
        deferrals: 4,
        false_preambles: 0,
        contention_timeouts: 1,
        duty_holds: 0,
        duty_timeouts: 0,
        radio_recoveries: 0,
    });
    let rows = details.as_slice();

    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0].text(), "Busy 12.3%");
    assert_eq!(rows[1].text(), "N/CCA -120/-109");
    assert_eq!(rows[2].text(), "Defers 4");
    assert_eq!(rows[3].text(), "CCA drops 1");
}

#[test]
fn named_peer_rows_format_single_link_interfaces() {
    let mut details = InterfaceMenuDetails::empty();
    let count = details.push_named_peer("USB", Some(ConnectionState::Connected));
    let rows = details.as_slice();

    assert_eq!(count, 1);
    assert_eq!(rows[0].text(), "Peers 1");
    assert_eq!(rows[1].text(), "P USB Live");
    assert_eq!(rows[1].kind(), InterfaceMenuDetailKind::Peer);

    let mut details = InterfaceMenuDetails::empty();
    let count = details.push_named_peer("USB", None);
    let rows = details.as_slice();
    assert_eq!(count, 0);
    assert_eq!(rows[0].text(), "Peers 0");
}
