use super::*;

#[test]
fn render_marks_selected_card_below_global_row() {
    let mut display = MockDisplay::new();
    display.set_allow_overdraw(true);
    display.set_allow_out_of_bounds_drawing(true);
    let cards = [test_card("A"), test_card("B")];
    let mut state = test_ui_state();
    state.handle_input(InputEvent::ShortPress, &cards);

    render_with_state(&mut display, &cards, BatteryState::Unknown, &state);

    let selected_top = FIRST_CARD_WITH_GLOBAL_TOP;
    assert!(state
        .selected_card(&cards)
        .is_some_and(|selected| core::ptr::eq(selected, &cards[0])));
    assert_eq!(state.visible_start(cards.len()), 0);
    assert_eq!(
        display.get_pixel(Point::new(NAME_BACKING_X, selected_top + NAME_BACKING_Y)),
        Some(BinaryColor::On)
    );
    assert_eq!(
        display.get_pixel(Point::new(0, selected_top)),
        Some(BinaryColor::On)
    );
    assert_ne!(
        display.get_pixel(Point::new(
            GLOBAL_BACKING_X,
            GLOBAL_ROW_TOP + GLOBAL_BACKING_Y
        )),
        Some(BinaryColor::On)
    );
}

#[test]
fn render_shows_selected_global_row() {
    let mut display = MockDisplay::new();
    display.set_allow_overdraw(true);
    display.set_allow_out_of_bounds_drawing(true);
    let cards = [test_card("USB")];
    let state = test_ui_state();

    render_with_state(&mut display, &cards, BatteryState::Unknown, &state);

    assert!(state.global_selected());
    assert_eq!(
        display.get_pixel(Point::new(
            GLOBAL_BACKING_X,
            GLOBAL_ROW_TOP + GLOBAL_BACKING_Y
        )),
        Some(BinaryColor::On)
    );
    assert_eq!(
        display.get_pixel(Point::new(GLOBAL_ICON_X, GLOBAL_ROW_TOP + NAME_LINE_Y)),
        Some(BinaryColor::Off)
    );
    assert_eq!(
        display.get_pixel(Point::new(NAME_ICON_X, GLOBAL_ROW_TOP + NAME_LINE_Y)),
        Some(BinaryColor::Off)
    );
    assert_eq!(
        display.get_pixel(Point::new(GLOBAL_BACKING_X, GLOBAL_ROW_TOP)),
        Some(BinaryColor::Off)
    );
    assert_eq!(
        display.get_pixel(Point::new(
            GLOBAL_BACKING_X,
            GLOBAL_ROW_TOP + GLOBAL_BACKING_Y + GLOBAL_BACKING_H as i32
        )),
        Some(BinaryColor::Off)
    );
    assert_eq!(
        display.get_pixel(Point::new(0, GLOBAL_ROW_TOP + GLOBAL_ROW_H - 1)),
        Some(BinaryColor::Off)
    );
    assert_eq!(
        display.get_pixel(Point::new(0, FIRST_CARD_WITH_GLOBAL_TOP)),
        Some(BinaryColor::On)
    );
}

#[test]
fn render_scrolls_local_docs_after_the_last_card() {
    let cards = [test_card("USB"), test_card("BLE"), test_card("WiFi")];
    let mut state = test_ui_state();
    let local_docs = LocalDocsAccess {
        wifi_ssid: "Hopspot-EW53",
        docs_host: "127.0.0.1",
    };
    for _ in 0..4 {
        state.handle_input_with_footer(InputEvent::ShortPress, &cards, true);
    }

    assert!(state.selected_card(&cards).is_none());
    assert_eq!(state.visible_start_with_footer(cards.len(), true), 3);

    let mut display = PanelDisplay::new();
    render_with_local_docs(
        &mut display,
        &cards,
        BatteryState::Unknown,
        &state,
        &local_docs,
    );
    assert!(has_on_pixel(
        &display,
        0..WIDTH,
        (CARD_TOP + CARD_SLOT_STEP)..(CARD_TOP + CARD_SLOT_STEP + FOOTER_SECOND_LINE_OFFSET + 8)
    ));
}

#[test]
fn render_shows_local_docs_access_details() {
    let cards = [test_card("USB"), test_card("BLE"), test_card("WiFi")];
    let mut state = test_ui_state();
    let local_docs = LocalDocsAccess {
        wifi_ssid: "Hopspot-EW53",
        docs_host: "192.168.4.1",
    };
    for _ in 0..4 {
        state.handle_input_with_footer(InputEvent::ShortPress, &cards, true);
    }

    let mut display = PanelDisplay::new();
    render_with_local_docs(
        &mut display,
        &cards,
        BatteryState::Unknown,
        &state,
        &local_docs,
    );
    assert!(has_on_pixel(
        &display,
        0..WIDTH,
        (CARD_TOP + CARD_SLOT_STEP + FOOTER_FOURTH_LINE_OFFSET)
            ..(CARD_TOP + CARD_SLOT_STEP + FOOTER_FOURTH_LINE_OFFSET + 10)
    ));
}

#[test]
fn footer_focus_long_press_opens_docs() {
    let cards = [test_card("USB")];
    let mut state = test_ui_state();

    assert_eq!(
        state.handle_input_with_footer(InputEvent::ShortPress, &cards, true),
        UiAction::None
    );
    assert!(state
        .selected_card(&cards)
        .is_some_and(|selected| core::ptr::eq(selected, &cards[0])));

    assert_eq!(
        state.handle_input_with_footer(InputEvent::ShortPress, &cards, true),
        UiAction::None
    );
    assert!(state.selected_card(&cards).is_none());

    assert_eq!(
        state.handle_input_with_footer(InputEvent::LongPress, &cards, true),
        UiAction::OpenDocs
    );
}

#[test]
fn render_scrolls_global_row_out_of_card_window() {
    let mut display = MockDisplay::new();
    display.set_allow_overdraw(true);
    display.set_allow_out_of_bounds_drawing(true);
    let cards = [test_card("A"), test_card("B"), test_card("C")];
    let mut state = test_ui_state();
    state.handle_input(InputEvent::ShortPress, &cards);
    state.handle_input(InputEvent::ShortPress, &cards);
    state.handle_input(InputEvent::ShortPress, &cards);

    render_with_state(&mut display, &cards, BatteryState::Unknown, &state);

    assert!(state
        .selected_card(&cards)
        .is_some_and(|selected| core::ptr::eq(selected, &cards[2])));
    assert_eq!(state.visible_start(cards.len()), 2);
    assert_eq!(
        display.get_pixel(Point::new(0, CARD_TOP)),
        Some(BinaryColor::On)
    );
    assert_ne!(
        display.get_pixel(Point::new(NAME_BACKING_X, CARD_TOP + NAME_BACKING_Y)),
        Some(BinaryColor::On)
    );
}

#[test]
fn render_shows_global_menu() {
    let mut display = MockDisplay::new();
    display.set_allow_overdraw(true);
    display.set_allow_out_of_bounds_drawing(true);
    let cards = [test_card("USB")];
    let mut state = test_ui_state();
    state.handle_input(InputEvent::LongPress, &cards);

    render_with_state(&mut display, &cards, BatteryState::Unknown, &state);

    assert_eq!(state.global_menu_selected_item(), Some(0));
    assert_eq!(
        display.get_pixel(Point::new(NAME_ICON_X, MENU_HEADER_Y)),
        Some(BinaryColor::On)
    );
    assert_eq!(
        display.get_pixel(Point::new(MENU_BACKING_X, MENU_ITEM_TOP - 1)),
        Some(BinaryColor::On)
    );
    assert_eq!(
        display.get_pixel(Point::new(MENU_MARK_X, MENU_ITEM_TOP + 2)),
        Some(BinaryColor::Off)
    );
    assert_eq!(
        display.get_pixel(Point::new(0, MENU_DIVIDER_Y)),
        Some(BinaryColor::On)
    );
}

#[test]
fn render_shows_selected_interface_menu() {
    let mut display = MockDisplay::new();
    display.set_allow_overdraw(true);
    display.set_allow_out_of_bounds_drawing(true);
    let cards = [
        test_card("USB"),
        Card {
            id: InterfaceId::new([0; 8]),
            kind: CardKind::Ble,
            label: card_label("BLE"),
            liveness: Liveness::Live,
            failure_reason: None,
            tx_bytes: 0,
            rx_bytes: 0,
            links: 0,
            destinations: 0,
            rate_bytes_per_sec: 0,
            last_activity_secs: None,
        },
    ];
    let mut state = test_ui_state();
    state.handle_input(InputEvent::ShortPress, &cards);
    state.handle_input(InputEvent::ShortPress, &cards);
    state.handle_input(InputEvent::LongPress, &cards);

    render_with_state(&mut display, &cards, BatteryState::Unknown, &state);

    assert!(state
        .selected_card(&cards)
        .is_some_and(|selected| core::ptr::eq(selected, &cards[1])));
    assert_eq!(state.interface_menu_selected_item(), Some(0));
    assert_eq!(
        display.get_pixel(Point::new(NAME_ICON_X + 4, MENU_HEADER_Y)),
        Some(BinaryColor::On)
    );
    assert_eq!(
        display.get_pixel(Point::new(MENU_BACKING_X, MENU_ITEM_TOP - 1)),
        Some(BinaryColor::On)
    );
    assert_eq!(
        display.get_pixel(Point::new(MENU_MARK_X, MENU_ITEM_TOP + 2)),
        Some(BinaryColor::Off)
    );
    assert_eq!(
        display.get_pixel(Point::new(0, MENU_DIVIDER_Y)),
        Some(BinaryColor::On)
    );
    assert_eq!(
        display.get_pixel(Point::new(0, CARD_TOP)),
        Some(BinaryColor::Off)
    );
}

#[test]
fn interface_menu_draws_detail_rows_below_actions() {
    let mut display = PanelDisplay::new();
    let mut card = test_card("WiFi/LAN");
    card.kind = CardKind::Wifi;
    let mut details = InterfaceMenuDetails::empty();
    details.push_info("STA", "None");
    details.push_info("AP", "Hopspot-EW53");
    let _ = details.push_supervisor_peers([(
        InterfaceId::new([0, 0xab, 0xcd, 0, 0, 0, 0, 0]),
        Liveness::Live,
    )]);

    draw_interface_menu(&mut display, &card, POWER_MENU_ITEM, details.as_slice());

    let detail_top = MENU_ITEM_TOP + POWER_ONLY_MENU_ITEMS.len() as i32 * MENU_ITEM_STEP + 1;
    assert!(
        has_on_pixel(&display, MENU_REASON_X..WIDTH, detail_top..HEIGHT),
        "interface menus should render supplied detail rows below the actions"
    );
}

#[test]
fn failed_interface_menu_draws_failure_reason() {
    let mut display = PanelDisplay::new();
    let mut card = test_card("BLE");
    card.kind = CardKind::Ble;
    card.liveness = Liveness::Failed;
    card.failure_reason = Some("BlueZ GATT Channels >1; set Channels=1");

    draw_interface_menu(&mut display, &card, POWER_MENU_ITEM, &[]);

    let reason_top = MENU_ITEM_TOP + POWER_ONLY_MENU_ITEMS.len() as i32 * MENU_ITEM_STEP - 1;
    assert!(
        has_on_pixel(&display, MENU_REASON_X..WIDTH, reason_top..HEIGHT),
        "failed-card menus should show the failure reason below the actions"
    );
}
