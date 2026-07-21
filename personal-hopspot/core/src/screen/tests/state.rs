use super::*;

#[test]
fn short_press_cycles_global_then_cards_and_pages_visible_window() {
    let mut state = test_ui_state();
    state.sync_card_count(5);

    assert!(state.global_selected());
    assert_eq!(state.selected_card(5), None);
    assert_eq!(state.visible_start(5), 0);

    state.handle_input(InputEvent::ShortPress, 5, Some(CardKind::Usb));
    assert_eq!(state.selected_card(5), Some(0));
    assert_eq!(state.visible_start(5), 0);

    state.handle_input(InputEvent::ShortPress, 5, Some(CardKind::Usb));
    assert_eq!(state.selected_card(5), Some(1));
    assert_eq!(state.visible_start(5), 0);

    state.handle_input(InputEvent::ShortPress, 5, Some(CardKind::Usb));
    assert_eq!(state.selected_card(5), Some(2));
    assert_eq!(state.visible_start(5), 2);

    state.handle_input(InputEvent::ShortPress, 5, Some(CardKind::Usb));
    assert_eq!(state.selected_card(5), Some(3));
    assert_eq!(state.visible_start(5), 3);

    state.handle_input(InputEvent::ShortPress, 5, Some(CardKind::Usb));
    assert_eq!(state.selected_card(5), Some(4));
    assert_eq!(state.visible_start(5), 4);

    state.handle_input(InputEvent::ShortPress, 5, Some(CardKind::Usb));
    assert!(state.global_selected());
    assert_eq!(state.selected_card(5), None);
    assert_eq!(state.visible_start(5), 0);
}

#[test]
fn long_press_opens_global_menu_and_short_press_cycles_menu_items() {
    let mut state = test_ui_state();

    state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb));

    assert_eq!(state.selected_card(4), None);
    assert_eq!(state.visible_start(4), 0);
    assert_eq!(state.global_menu_selected_item(), Some(0));
    assert_eq!(state.menu_selected_item(), Some(0));

    state.handle_input(InputEvent::ShortPress, 4, Some(CardKind::Usb));

    assert_eq!(state.selected_card(4), None);
    assert_eq!(state.global_menu_selected_item(), Some(1));
    assert_eq!(state.menu_selected_item(), Some(1));

    state.handle_input(InputEvent::ShortPress, 4, Some(CardKind::Usb));
    assert_eq!(state.global_menu_selected_item(), Some(2));
    state.handle_input(InputEvent::ShortPress, 4, Some(CardKind::Usb));
    assert_eq!(state.global_menu_selected_item(), Some(3));

    state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb));

    assert!(state.global_selected());
    assert_eq!(state.menu_selected_item(), None);
}

#[test]
fn long_press_on_the_announce_item_returns_the_announce_action() {
    let mut state = test_ui_state();

    assert_eq!(
        state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb)),
        UiAction::None
    );
    assert_eq!(state.global_menu_selected_item(), Some(ANNOUNCE_MENU_ITEM));

    assert_eq!(
        state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb)),
        UiAction::Announce,
    );
    assert_eq!(state.menu_selected_item(), None);
    assert!(state.global_selected());
}

#[test]
fn long_press_on_limits_opens_the_paged_limits_page() {
    let mut state = test_ui_state();
    state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb));
    state.handle_input(InputEvent::ShortPress, 4, Some(CardKind::Usb));

    assert_eq!(
        state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb)),
        UiAction::None
    );
    assert_eq!(state.mode, UiMode::LimitsPage { page: 0 });
    assert_eq!(state.menu_selected_item(), None);
    assert_eq!(
        state.handle_input(InputEvent::ShortPress, 4, Some(CardKind::Usb)),
        UiAction::None
    );
    assert_eq!(state.mode, UiMode::LimitsPage { page: 1 });
    assert_eq!(
        state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb)),
        UiAction::None
    );
    assert!(state.global_selected());
}

#[test]
fn long_press_on_sleep_enters_sleep_and_next_press_wakes() {
    let mut state = test_ui_state();
    state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb));
    state.handle_input(InputEvent::ShortPress, 4, Some(CardKind::Usb));
    state.handle_input(InputEvent::ShortPress, 4, Some(CardKind::Usb));

    assert_eq!(
        state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb)),
        UiAction::Sleep
    );
    assert_eq!(state.mode, UiMode::Sleeping);
    assert_eq!(
        state.handle_input(InputEvent::ShortPress, 4, Some(CardKind::Usb)),
        UiAction::Wake
    );
    assert!(state.global_selected());
}

#[test]
fn oled_capable_menu_offers_display_off_before_sleep() {
    let mut state = test_ui_state_with_display_power();
    state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb));
    state.handle_input(InputEvent::ShortPress, 4, Some(CardKind::Usb));
    state.handle_input(InputEvent::ShortPress, 4, Some(CardKind::Usb));

    assert_eq!(state.global_menu_selected_item(), Some(OLED_OFF_MENU_ITEM));
    assert_eq!(
        state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb)),
        UiAction::OledOff
    );
    assert!(state.global_selected());

    state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb));
    for _ in 0..SLEEP_MENU_ITEM {
        state.handle_input(InputEvent::ShortPress, 4, Some(CardKind::Usb));
    }
    assert_eq!(
        state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb)),
        UiAction::Sleep
    );
}

#[test]
fn long_press_on_back_closes_the_global_menu() {
    let mut state = test_ui_state();
    state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb));
    for _ in 0..3 {
        state.handle_input(InputEvent::ShortPress, 4, Some(CardKind::Usb));
    }

    assert_eq!(
        state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb)),
        UiAction::None
    );
    assert_eq!(state.menu_selected_item(), None);
    assert!(state.global_selected());
}

#[test]
fn global_menu_cycles_only_actionable_items() {
    let mut state = test_ui_state();
    state.handle_input(InputEvent::LongPress, 1, Some(CardKind::Usb));

    assert_eq!(state.global_menu_selected_item(), Some(0));
    state.handle_input(InputEvent::ShortPress, 1, Some(CardKind::Usb));
    assert_eq!(state.global_menu_selected_item(), Some(1));
    state.handle_input(InputEvent::ShortPress, 1, Some(CardKind::Usb));
    assert_eq!(state.global_menu_selected_item(), Some(2));
    state.handle_input(InputEvent::ShortPress, 1, Some(CardKind::Usb));
    assert_eq!(state.global_menu_selected_item(), Some(3));
    state.handle_input(InputEvent::ShortPress, 1, Some(CardKind::Usb));
    assert_eq!(state.global_menu_selected_item(), Some(0));
}

#[test]
fn supported_access_point_states_offer_the_radio_swap_action() {
    for access_point in [AccessPointState::Inactive, AccessPointState::Active] {
        let mut state = test_ui_state_with_access_point(access_point);
        state.handle_input(InputEvent::LongPress, 1, Some(CardKind::Usb));
        for _ in 0..RADIO_MENU_ITEM_NO_DISPLAY {
            state.handle_input(InputEvent::ShortPress, 1, Some(CardKind::Usb));
        }

        assert_eq!(
            state.global_menu_selected_item(),
            Some(RADIO_MENU_ITEM_NO_DISPLAY)
        );
        assert_eq!(
            state.handle_input(InputEvent::LongPress, 1, Some(CardKind::Usb)),
            UiAction::None
        );
        assert_eq!(state.mode, UiMode::ConfirmRadioSwap { confirm: false });
        state.handle_input(InputEvent::ShortPress, 1, Some(CardKind::Usb));
        assert_eq!(
            state.handle_input(InputEvent::LongPress, 1, Some(CardKind::Usb)),
            UiAction::SwapRadioMode
        );
    }
}

#[test]
fn non_lora_interface_menus_cycle_power_and_back_only() {
    let mut state = test_ui_state();
    state.handle_input(InputEvent::ShortPress, 1, Some(CardKind::Usb));
    state.handle_input(InputEvent::LongPress, 1, Some(CardKind::Usb));

    assert_eq!(state.interface_menu_selected_item(), Some(0));
    state.handle_input(InputEvent::ShortPress, 1, Some(CardKind::Usb));
    assert_eq!(state.interface_menu_selected_item(), Some(1));
    state.handle_input(InputEvent::ShortPress, 1, Some(CardKind::Usb));
    assert_eq!(state.interface_menu_selected_item(), Some(0));
}

#[test]
fn lora_interface_menu_keeps_tune_and_reset() {
    let mut state = test_ui_state();
    state.handle_input(InputEvent::ShortPress, 1, Some(CardKind::LoRa));
    state.handle_input(InputEvent::LongPress, 1, Some(CardKind::LoRa));

    assert_eq!(state.interface_menu_selected_item(), Some(0));
    state.handle_input(InputEvent::ShortPress, 1, Some(CardKind::LoRa));
    assert_eq!(
        state.interface_menu_selected_item(),
        Some(LORA_TUNE_MENU_ITEM)
    );
    state.handle_input(InputEvent::ShortPress, 1, Some(CardKind::LoRa));
    assert_eq!(
        state.interface_menu_selected_item(),
        Some(LORA_RESET_MENU_ITEM)
    );
    state.handle_input(InputEvent::ShortPress, 1, Some(CardKind::LoRa));
    assert_eq!(state.interface_menu_selected_item(), Some(3));
    state.handle_input(InputEvent::ShortPress, 1, Some(CardKind::LoRa));
    assert_eq!(state.interface_menu_selected_item(), Some(0));
}

#[test]
fn long_press_opens_interface_menu_after_card_focus() {
    let mut state = test_ui_state();
    state.handle_input(InputEvent::ShortPress, 4, Some(CardKind::Usb));

    state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb));

    assert_eq!(state.selected_card(4), Some(0));
    assert_eq!(state.visible_start(4), 0);
    assert_eq!(state.interface_menu_selected_item(), Some(0));

    state.handle_input(InputEvent::ShortPress, 4, Some(CardKind::Usb));

    assert_eq!(state.selected_card(4), Some(0));
    assert_eq!(state.interface_menu_selected_item(), Some(1));

    state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb));

    assert_eq!(state.selected_card(4), Some(0));
    assert_eq!(state.menu_selected_item(), None);
}
