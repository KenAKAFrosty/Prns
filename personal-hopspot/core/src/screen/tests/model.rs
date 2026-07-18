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
fn activity_tracker_stamps_age_when_a_card_changes() {
    let mut tracker = CardActivityTracker::<2>::new();
    let mut cards = [test_card("USB")];
    cards[0].liveness = Liveness::Dormant;

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
    let mut rows = InterfaceMenuDetailRows::new();
    push_interface_menu_info(&mut rows, "AP", "Hopspot-EW53");
    let count = push_supervisor_peer_rows(
        &mut rows,
        [
            SupervisorPeerMenuStatus {
                id: InterfaceId::new([0, 0xab, 0xcd, 0, 0, 0, 0, 0]),
                liveness: Liveness::Live,
            },
            SupervisorPeerMenuStatus {
                id: InterfaceId::new([0, 0x12, 0x34, 0, 0, 0, 0, 0]),
                liveness: Liveness::Dormant,
            },
        ],
    );

    assert_eq!(count, 2);
    assert_eq!(rows[0].text(), "AP Hopspot-EW53");
    assert_eq!(rows[1].text(), "Peers 2");
    assert_eq!(rows[2].text(), "P abcd Live");
    assert_eq!(rows[3].text(), "P 1234 Dorm");
    assert_eq!(rows[2].kind(), InterfaceMenuDetailKind::Peer);
}

#[test]
fn named_peer_rows_format_single_link_interfaces() {
    let mut rows = InterfaceMenuDetailRows::new();
    let count = push_named_peer_row(&mut rows, "USB", Some(Liveness::Live));

    assert_eq!(count, 1);
    assert_eq!(rows[0].text(), "Peers 1");
    assert_eq!(rows[1].text(), "P USB Live");
    assert_eq!(rows[1].kind(), InterfaceMenuDetailKind::Peer);

    rows.clear();
    let count = push_named_peer_row(&mut rows, "USB", None);
    assert_eq!(count, 0);
    assert_eq!(rows[0].text(), "Peers 0");
}
