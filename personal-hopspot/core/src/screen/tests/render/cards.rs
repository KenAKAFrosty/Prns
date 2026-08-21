use super::*;

#[test]
fn cards_name_each_connection_state_without_a_dormant_bucket() {
    let states = [
        (
            CardKind::Tcp,
            ConnectionState::Initializing,
            Some("Initializing"),
        ),
        (CardKind::Tcp, ConnectionState::Connected, None),
        (CardKind::Tcp, ConnectionState::Degraded, Some("Degraded")),
        (
            CardKind::Tcp,
            ConnectionState::Reconnecting,
            Some("Retrying"),
        ),
        (CardKind::Tcp, ConnectionState::Failed, Some("Failed")),
        (
            CardKind::Tcp,
            ConnectionState::Disconnected,
            Some("Disconnected"),
        ),
        (CardKind::Tcp, ConnectionState::Disabled, Some("Off")),
        (CardKind::Tcp, ConnectionState::Unknown, Some("Unknown")),
        (
            CardKind::Wifi,
            ConnectionState::Disconnected,
            Some("Waiting"),
        ),
        (
            CardKind::WifiStation,
            ConnectionState::Disconnected,
            Some("Waiting"),
        ),
        (
            CardKind::WifiStationDisabled,
            ConnectionState::Disconnected,
            Some("Waiting"),
        ),
        (
            CardKind::Usb,
            ConnectionState::Disconnected,
            Some("Waiting"),
        ),
    ];

    let labels: HVec<Option<&str>, 12> = states
        .iter()
        .map(|(kind, connection, _)| connection_status_label(*kind, *connection))
        .collect();
    let expected: HVec<Option<&str>, 12> =
        states.iter().map(|(_, _, expected)| *expected).collect();

    assert_eq!(labels, expected);
}

#[test]
fn card_label_budgets_follow_the_rendered_font_and_slot() {
    assert_eq!(card_label_max_chars(CardKind::Wifi), 8);
    assert_eq!(card_label_max_chars(CardKind::Tcp), 8);
    assert_eq!(card_label_max_chars(CardKind::Peer), 10);
}

#[test]
fn card_stacks_traffic_and_moves_destinations_right() {
    let mut display = MockDisplay::new();
    display.set_allow_overdraw(true);
    let card = Card {
        id: InterfaceId::new([0; 8]),
        kind: CardKind::Usb,
        label: card_label("USB"),
        connection: ConnectionState::Connected,
        failure_reason: None,
        tx_bytes: 123,
        rx_bytes: 456,
        links: 5,
        peers: None,
        destinations: 7,
        rate_bytes_per_sec: 12_345,
        last_activity_secs: Some(3),
    };

    draw_card_with_selection(&mut display, 0, &card, false);

    assert_eq!(display.get_pixel(Point::new(4, 14)), Some(BinaryColor::On));
    assert_eq!(display.get_pixel(Point::new(4, 20)), None);
    assert_eq!(display.get_pixel(Point::new(4, 22)), None);
    assert_eq!(display.get_pixel(Point::new(4, 23)), Some(BinaryColor::On));
    assert_eq!(display.get_pixel(Point::new(4, 28)), Some(BinaryColor::On));
    assert_eq!(display.get_pixel(Point::new(4, 29)), None);
    assert_eq!(display.get_pixel(Point::new(33, 14)), None);
    assert_eq!(display.get_pixel(Point::new(37, 14)), Some(BinaryColor::On));
    assert_eq!(display.get_pixel(Point::new(35, 14)), None);
    assert_eq!(display.get_pixel(Point::new(42, 14)), None);
    assert_eq!(display.get_pixel(Point::new(35, 23)), Some(BinaryColor::On));
    assert_eq!(display.get_pixel(Point::new(37, 23)), None);
    assert_eq!(display.get_pixel(Point::new(5, 32)), None);
    assert_eq!(display.get_pixel(Point::new(38, 32)), Some(BinaryColor::On));
}

#[test]
fn large_link_and_destination_counts_fit_right_column() {
    let mut display = MockDisplay::new();
    display.set_allow_overdraw(true);
    let card = Card {
        id: InterfaceId::new([0; 8]),
        kind: CardKind::Wifi,
        label: card_label("Wi-Fi"),
        connection: ConnectionState::Connected,
        failure_reason: None,
        tx_bytes: 999_999_999,
        rx_bytes: 999_999_999,
        links: 999_999,
        peers: Some(12),
        destinations: 1_234_567_890,
        rate_bytes_per_sec: 999_999_999,
        last_activity_secs: Some(3599),
    };

    draw_card_with_selection(&mut display, 0, &card, false);

    assert_eq!(compact_numeric_width("999K"), 20);
    assert_eq!(compact_numeric_width("1.2B"), 17);
    assert!(STAT_TEXT_X + compact_numeric_width("999K") < WIDTH);
    assert!(8 + compact_numeric_width("999M") < STAT_ICON_X);
    assert!(ACTIVITY_TEXT_X + compact_numeric_width("-") < WIDTH);
}

#[test]
fn supervisor_cards_render_destinations_instead_of_unlabelled_peer_count() {
    fn render(peers: u32, destinations: u32) -> MockDisplay<BinaryColor> {
        let mut display = MockDisplay::new();
        display.set_allow_overdraw(true);
        let card = Card {
            id: InterfaceId::new([0; 8]),
            kind: CardKind::Wifi,
            label: card_label("LAN"),
            connection: ConnectionState::Connected,
            failure_reason: None,
            tx_bytes: 0,
            rx_bytes: 0,
            links: 0,
            peers: Some(peers),
            destinations,
            rate_bytes_per_sec: 0,
            last_activity_secs: None,
        };

        draw_card_with_selection(&mut display, 0, &card, false);
        display
    }

    assert_eq!(render(2, 512), render(9, 512));
    assert_ne!(render(2, 512), render(2, 511));
}

#[test]
fn offline_card_centers_status_and_hides_metrics() {
    let mut display = MockDisplay::new();
    display.set_allow_overdraw(true);
    let card = Card {
        id: InterfaceId::new([0; 8]),
        kind: CardKind::EspNow,
        label: card_label("ESP-NOW"),
        connection: ConnectionState::Failed,
        failure_reason: Some("BlueZ GATT Channels >1; set Channels=1"),
        tx_bytes: 123,
        rx_bytes: 456,
        links: 5,
        peers: None,
        destinations: 7,
        rate_bytes_per_sec: 123,
        last_activity_secs: Some(12),
    };

    draw_card_with_selection(&mut display, 0, &card, false);

    assert_eq!(display.get_pixel(Point::new(18, 21)), Some(BinaryColor::On));
    assert_eq!(display.get_pixel(Point::new(3, 11)), None);
    assert_eq!(display.get_pixel(Point::new(4, 10)), None);
    assert_eq!(display.get_pixel(Point::new(5, 9)), Some(BinaryColor::On));
    assert_eq!(display.get_pixel(Point::new(3, 4)), None);
    assert_eq!(display.get_pixel(Point::new(4, 14)), None);
    assert_eq!(display.get_pixel(Point::new(44, 14)), None);
    assert_eq!(display.get_pixel(Point::new(45, 23)), None);
    assert_eq!(display.get_pixel(Point::new(5, 32)), None);
    assert_eq!(display.get_pixel(Point::new(36, 32)), None);
}

#[test]
fn selected_card_inverts_name_content() {
    let mut display = MockDisplay::new();
    display.set_allow_overdraw(true);
    let card = Card {
        id: InterfaceId::new([0; 8]),
        kind: CardKind::Wifi,
        label: card_label("Wi-Fi"),
        connection: ConnectionState::Connected,
        failure_reason: None,
        tx_bytes: 0,
        rx_bytes: 0,
        links: 0,
        peers: Some(0),
        destinations: 0,
        rate_bytes_per_sec: 0,
        last_activity_secs: None,
    };

    draw_card_with_selection(&mut display, 0, &card, true);

    assert_eq!(display.get_pixel(Point::new(0, 0)), Some(BinaryColor::On));
    assert_eq!(display.get_pixel(Point::new(63, 0)), Some(BinaryColor::On));
    assert_eq!(display.get_pixel(Point::new(0, 11)), Some(BinaryColor::On));
    assert_eq!(display.get_pixel(Point::new(63, 11)), Some(BinaryColor::On));
    assert_eq!(display.get_pixel(Point::new(1, 1)), None);
    assert_eq!(display.get_pixel(Point::new(2, 1)), None);
    assert_eq!(display.get_pixel(Point::new(45, 1)), None);
    assert_eq!(display.get_pixel(Point::new(0, 12)), Some(BinaryColor::On));
    assert_eq!(
        display.get_pixel(Point::new(0, CARD_H - 1)),
        Some(BinaryColor::On)
    );
    assert_eq!(
        display.get_pixel(Point::new(63, CARD_H - 1)),
        Some(BinaryColor::On)
    );
    assert_eq!(
        display.get_pixel(Point::new(31, CARD_H - 1)),
        Some(BinaryColor::On)
    );
    assert_eq!(display.get_pixel(Point::new(2, 2)), Some(BinaryColor::On));
    assert_eq!(display.get_pixel(Point::new(2, 10)), Some(BinaryColor::On));
    assert_eq!(display.get_pixel(Point::new(2, 11)), None);
    assert_eq!(display.get_pixel(Point::new(5, 2)), Some(BinaryColor::Off));
}
