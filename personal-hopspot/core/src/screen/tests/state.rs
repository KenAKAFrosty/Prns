use super::*;
use core::future::Future;
use core::task::{Context, Poll};
use std::boxed::Box;
use std::cell::RefCell;
use std::task::Waker;

fn block_on<F: Future>(future: F) -> F::Output {
    let mut context = Context::from_waker(Waker::noop());
    let mut future = Box::pin(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

#[test]
fn radio_profile_save_result_waits_for_apply_and_verified_persistence() {
    let steps = RefCell::new(std::vec::Vec::new());
    let result = block_on(apply_and_persist_radio_profile(
        async {
            steps.borrow_mut().push("applied");
            true
        },
        || async {
            assert_eq!(steps.borrow().as_slice(), ["applied"]);
            steps.borrow_mut().push("verified");
            true
        },
    ));

    assert_eq!(result, RadioProfileChangeResult::Saved);
    assert_eq!(result.notice(), UiNotice::Saved);
    assert_eq!(steps.into_inner(), ["applied", "verified"]);
}

#[test]
fn radio_profile_save_result_distinguishes_apply_and_persistence_failures() {
    let apply_failed = block_on(apply_and_persist_radio_profile(async { false }, || async {
        panic!("persistence must not run after a rejected profile")
    }));
    assert_eq!(apply_failed, RadioProfileChangeResult::ApplyFailed);
    assert!(!apply_failed.applied());

    let persistence_failed = block_on(apply_and_persist_radio_profile(async { true }, || async {
        false
    }));
    assert_eq!(
        persistence_failed,
        RadioProfileChangeResult::ProfileNotSaved
    );
    assert!(persistence_failed.applied());
}

#[test]
fn persistence_notices_are_brief_across_deferred_failed_and_recovered_states() {
    let mut state = test_ui_state();
    let mut persistence = PersistenceNotice::new();

    assert!(!persistence.update(&mut state, PersistenceState::Durable, 0));
    assert_eq!(state.notice(), None);
    assert!(persistence.update(&mut state, PersistenceState::Deferred, 1_000));
    assert_eq!(state.notice(), Some(UiNotice::SaveDeferred));
    assert!(!persistence.update(&mut state, PersistenceState::Deferred, 5_999));
    assert_eq!(state.notice(), Some(UiNotice::SaveDeferred));
    assert!(persistence.update(&mut state, PersistenceState::Deferred, 6_000));
    assert_eq!(state.notice(), None);

    assert!(persistence.update(&mut state, PersistenceState::Failed, 7_000));
    assert_eq!(state.notice(), Some(UiNotice::SaveFailed));
    assert!(persistence.update(&mut state, PersistenceState::Recovered, 8_000));
    assert_eq!(state.notice(), Some(UiNotice::StateRecovered));
    assert!(persistence.update(&mut state, PersistenceState::Durable, 9_000));
    assert_eq!(state.notice(), Some(UiNotice::Saved));
    assert!(persistence.update(&mut state, PersistenceState::Durable, 14_000));
    assert_eq!(state.notice(), None);
}

#[test]
fn persistence_timer_only_clears_the_notice_it_owns() {
    let mut state = test_ui_state();
    let mut persistence = PersistenceNotice::new();

    persistence.update(&mut state, PersistenceState::Failed, 1_000);
    state.show_notice(UiNotice::Announcing);
    assert!(!persistence.update(&mut state, PersistenceState::Failed, 6_000));
    assert_eq!(state.notice(), Some(UiNotice::Announcing));
    assert!(!state.clear_notice_if(UiNotice::SaveFailed));
    assert_eq!(state.notice(), Some(UiNotice::Announcing));
    assert!(state.clear_notice_if(UiNotice::Announcing));
    assert_eq!(state.notice(), None);
}

#[test]
fn persistence_changes_can_be_timed_by_a_physical_presenter() {
    let mut persistence = PersistenceNotice::new();

    assert_eq!(persistence.observe(PersistenceState::Durable), None);
    assert_eq!(
        persistence.observe(PersistenceState::Deferred),
        Some(UiNotice::SaveDeferred)
    );
    assert_eq!(persistence.observe(PersistenceState::Deferred), None);
    assert_eq!(
        persistence.observe(PersistenceState::Recovered),
        Some(UiNotice::StateRecovered)
    );
}

#[test]
fn every_notice_line_fits_its_rendered_font() {
    for notice in UiNotice::ALL {
        let lines = notice.lines();
        let char_width = if lines.as_slice().len() > 1 {
            FONT_4X6_CHAR_W
        } else {
            FONT_5X8_CHAR_W
        };
        let max_chars = (WIDTH / char_width) as usize;
        assert!(
            lines
                .as_slice()
                .iter()
                .all(|line| line.chars().count() <= max_chars),
            "{notice:?} exceeds {max_chars} characters"
        );
    }
}

#[test]
fn short_press_dismisses_notice_without_moving_focus() {
    let cards = test_cards::<2>(CardKind::Wifi);
    let content = test_content(&cards);
    let mut state = test_ui_state();
    state.show_notice(UiNotice::SaveDeferred);

    assert_eq!(
        state.handle_input(InputEvent::ShortPress, content),
        UiAction::None
    );
    assert_eq!(state.notice(), None);
    assert!(state.global_selected());
}

#[test]
fn sleeping_notice_does_not_consume_the_wake_press() {
    let cards = test_cards::<1>(CardKind::Usb);
    let content = test_content(&cards);
    let mut state = test_ui_state();

    state.handle_input(InputEvent::LongPress, content);
    for _ in 0..2 {
        state.handle_input(InputEvent::ShortPress, content);
    }
    assert_eq!(
        state.handle_input(InputEvent::LongPress, content),
        UiAction::Sleep
    );
    state.show_notice(UiNotice::Sleeping);

    assert_eq!(
        state.handle_input(InputEvent::ShortPress, content),
        UiAction::Wake
    );
    assert_eq!(state.visible_notice(), None);
}

#[test]
fn short_press_cycles_global_then_cards_and_pages_visible_window() {
    let cards = test_cards::<5>(CardKind::Usb);
    let content = test_content(&cards);
    let mut state = test_ui_state();
    state.sync(content);

    assert!(state.global_selected());
    assert_eq!(state.selected_card_index(5), None);
    assert_eq!(state.visible_start, 0);

    state.handle_input(InputEvent::ShortPress, content);
    assert_eq!(state.selected_card_index(5), Some(0));
    assert_eq!(state.visible_start, 0);

    state.handle_input(InputEvent::ShortPress, content);
    assert_eq!(state.selected_card_index(5), Some(1));
    assert_eq!(state.visible_start, 0);

    state.handle_input(InputEvent::ShortPress, content);
    assert_eq!(state.selected_card_index(5), Some(2));
    assert_eq!(state.visible_start, 2);

    state.handle_input(InputEvent::ShortPress, content);
    assert_eq!(state.selected_card_index(5), Some(3));
    assert_eq!(state.visible_start, 3);

    state.handle_input(InputEvent::ShortPress, content);
    assert_eq!(state.selected_card_index(5), Some(4));
    assert_eq!(state.visible_start, 4);

    state.handle_input(InputEvent::ShortPress, content);
    assert!(state.global_selected());
    assert_eq!(state.selected_card_index(5), None);
    assert_eq!(state.visible_start, 0);
}

#[test]
fn long_press_opens_global_menu_and_short_press_cycles_menu_items() {
    let cards = test_cards::<4>(CardKind::Usb);
    let content = test_content(&cards);
    let mut state = test_ui_state();

    state.handle_input(InputEvent::LongPress, content);

    assert_eq!(state.selected_card_index(4), None);
    assert_eq!(state.visible_start, 0);
    assert_eq!(state.global_menu_selected_item(), Some(0));

    state.handle_input(InputEvent::ShortPress, content);

    assert_eq!(state.selected_card_index(4), None);
    assert_eq!(state.global_menu_selected_item(), Some(1));

    state.handle_input(InputEvent::ShortPress, content);
    assert_eq!(state.global_menu_selected_item(), Some(2));
    state.handle_input(InputEvent::ShortPress, content);
    assert_eq!(state.global_menu_selected_item(), Some(3));

    state.handle_input(InputEvent::LongPress, content);

    assert!(state.global_selected());
}

#[test]
fn long_press_on_the_announce_item_returns_the_announce_action() {
    let cards = test_cards::<4>(CardKind::Usb);
    let content = test_content(&cards);
    let mut state = test_ui_state();

    assert_eq!(
        state.handle_input(InputEvent::LongPress, content),
        UiAction::None
    );
    assert_eq!(state.global_menu_selected_item(), Some(ANNOUNCE_MENU_ITEM));

    assert_eq!(
        state.handle_input(InputEvent::LongPress, content),
        UiAction::Announce,
    );
    assert!(state.global_selected());
}

#[test]
fn long_press_on_limits_opens_the_paged_limits_page() {
    let cards = test_cards::<4>(CardKind::Usb);
    let content = test_content(&cards);
    let mut state = test_ui_state();
    state.handle_input(InputEvent::LongPress, content);
    state.handle_input(InputEvent::ShortPress, content);

    assert_eq!(
        state.handle_input(InputEvent::LongPress, content),
        UiAction::None
    );
    assert_eq!(state.mode, UiMode::LimitsPage { page: 0 });
    assert_eq!(
        state.handle_input(InputEvent::ShortPress, content),
        UiAction::None
    );
    assert_eq!(state.mode, UiMode::LimitsPage { page: 1 });
    assert_eq!(
        state.handle_input(InputEvent::LongPress, content),
        UiAction::None
    );
    assert!(state.global_selected());
}

#[test]
fn available_gnss_menu_toggles_the_live_panel_and_receiver_demand() {
    let cards = test_cards::<2>(CardKind::Usb);
    let content = test_content(&cards);
    let mut state = test_ui_state_with_gnss();

    state.handle_input(InputEvent::LongPress, content);
    state.handle_input(InputEvent::ShortPress, content);
    state.handle_input(InputEvent::ShortPress, content);
    assert_eq!(state.global_menu_selected_item(), Some(2));
    assert_eq!(state.global_menu_item_label(GlobalMenuItem::Gnss), "GPS On");
    assert_eq!(
        state.handle_input(InputEvent::LongPress, content),
        UiAction::ControlGnss(GnssReceiverCommand::Enable)
    );
    assert!(state.gnss_visible());

    state.handle_input(InputEvent::LongPress, content);
    state.handle_input(InputEvent::ShortPress, content);
    state.handle_input(InputEvent::ShortPress, content);
    assert_eq!(
        state.global_menu_item_label(GlobalMenuItem::Gnss),
        "GPS Off"
    );
    assert_eq!(
        state.handle_input(InputEvent::LongPress, content),
        UiAction::ControlGnss(GnssReceiverCommand::Disable)
    );
    assert!(!state.gnss_visible());
}

#[test]
fn long_press_on_sleep_enters_sleep_and_next_press_wakes() {
    let cards = test_cards::<4>(CardKind::Usb);
    let content = test_content(&cards);
    let mut state = test_ui_state();
    state.handle_input(InputEvent::LongPress, content);
    state.handle_input(InputEvent::ShortPress, content);
    state.handle_input(InputEvent::ShortPress, content);

    assert_eq!(
        state.handle_input(InputEvent::LongPress, content),
        UiAction::Sleep
    );
    assert_eq!(state.mode, UiMode::Sleeping);
    assert_eq!(
        state.handle_input(InputEvent::ShortPress, content),
        UiAction::Wake
    );
    assert!(state.global_selected());
}

#[test]
fn display_capable_menu_offers_display_controls_before_sleep() {
    let cards = test_cards::<4>(CardKind::Usb);
    let content = test_content(&cards);
    let mut state = test_ui_state_with_display_power();
    state.handle_input(InputEvent::LongPress, content);
    state.handle_input(InputEvent::ShortPress, content);
    state.handle_input(InputEvent::ShortPress, content);

    assert_eq!(
        state.global_menu_selected_item(),
        Some(BLANK_DISPLAY_MENU_ITEM)
    );
    assert_eq!(
        state.global_menu_item_label(GlobalMenuItem::BlankDisplay),
        "Screen Off"
    );
    assert_eq!(
        state.handle_input(InputEvent::LongPress, content),
        UiAction::BlankDisplay
    );
    assert!(state.global_selected());

    state.handle_input(InputEvent::LongPress, content);
    for _ in 0..DISPLAY_AUTO_OFF_MENU_ITEM {
        state.handle_input(InputEvent::ShortPress, content);
    }
    assert_eq!(
        state.global_menu_item_label(GlobalMenuItem::DisplayAutoOff),
        "Auto Off"
    );
    assert_eq!(
        state.handle_input(InputEvent::LongPress, content),
        UiAction::ToggleDisplayAutoOff
    );
    assert!(state.global_selected());

    state.handle_input(InputEvent::LongPress, content);
    for _ in 0..SLEEP_MENU_ITEM {
        state.handle_input(InputEvent::ShortPress, content);
    }
    assert_eq!(
        state.handle_input(InputEvent::LongPress, content),
        UiAction::Sleep
    );
}

#[test]
fn long_press_on_back_closes_the_global_menu() {
    let cards = test_cards::<4>(CardKind::Usb);
    let content = test_content(&cards);
    let mut state = test_ui_state();
    state.handle_input(InputEvent::LongPress, content);
    for _ in 0..3 {
        state.handle_input(InputEvent::ShortPress, content);
    }

    assert_eq!(
        state.handle_input(InputEvent::LongPress, content),
        UiAction::None
    );
    assert!(state.global_selected());
}

#[test]
fn global_menu_cycles_only_actionable_items() {
    let cards = test_cards::<1>(CardKind::Usb);
    let content = test_content(&cards);
    let mut state = test_ui_state();
    state.handle_input(InputEvent::LongPress, content);

    assert_eq!(state.global_menu_selected_item(), Some(0));
    state.handle_input(InputEvent::ShortPress, content);
    assert_eq!(state.global_menu_selected_item(), Some(1));
    state.handle_input(InputEvent::ShortPress, content);
    assert_eq!(state.global_menu_selected_item(), Some(2));
    state.handle_input(InputEvent::ShortPress, content);
    assert_eq!(state.global_menu_selected_item(), Some(3));
    state.handle_input(InputEvent::ShortPress, content);
    assert_eq!(state.global_menu_selected_item(), Some(0));
}

#[test]
fn supported_access_point_states_offer_the_radio_swap_action() {
    let cards = test_cards::<1>(CardKind::Usb);
    let content = test_content(&cards);
    for access_point in [AccessPointState::Inactive, AccessPointState::Active] {
        let mut state = test_ui_state_with_access_point(access_point);
        state.handle_input(InputEvent::LongPress, content);
        for _ in 0..RADIO_MENU_ITEM_NO_DISPLAY {
            state.handle_input(InputEvent::ShortPress, content);
        }

        assert_eq!(
            state.global_menu_selected_item(),
            Some(RADIO_MENU_ITEM_NO_DISPLAY)
        );
        assert_eq!(
            state.handle_input(InputEvent::LongPress, content),
            UiAction::None
        );
        assert_eq!(state.mode, UiMode::ConfirmRadioSwap { confirm: false });
        state.handle_input(InputEvent::ShortPress, content);
        assert_eq!(
            state.handle_input(InputEvent::LongPress, content),
            UiAction::SwapRadioMode
        );
    }
}

#[test]
fn ble_interface_menu_is_power_and_back_only() {
    let cards = test_cards::<1>(CardKind::Ble);
    let content = test_content(&cards);
    let mut state = test_ui_state();
    state.handle_input(InputEvent::ShortPress, content);
    state.handle_input(InputEvent::LongPress, content);
    assert_eq!(state.interface_menu_selected_item(), Some(POWER_MENU_ITEM));
    state.handle_input(InputEvent::ShortPress, content);
    assert_eq!(state.interface_menu_selected_item(), Some(1));
    assert_eq!(
        state.handle_input(InputEvent::LongPress, content),
        UiAction::None
    );
}

#[test]
fn non_lora_interface_menus_cycle_power_and_back_only() {
    let cards = test_cards::<1>(CardKind::Usb);
    let content = test_content(&cards);
    let mut state = test_ui_state();
    state.handle_input(InputEvent::ShortPress, content);
    state.handle_input(InputEvent::LongPress, content);

    assert_eq!(state.interface_menu_selected_item(), Some(0));
    state.handle_input(InputEvent::ShortPress, content);
    assert_eq!(state.interface_menu_selected_item(), Some(1));
    state.handle_input(InputEvent::ShortPress, content);
    assert_eq!(state.interface_menu_selected_item(), Some(0));
}

#[test]
fn shared_instance_config_action_requires_platform_capability() {
    let cards = test_cards::<1>(CardKind::SharedInstance);
    let content = test_content(&cards);
    let mut unavailable = test_ui_state();
    unavailable.handle_input(InputEvent::ShortPress, content);
    unavailable.handle_input(InputEvent::LongPress, content);
    unavailable.handle_input(InputEvent::ShortPress, content);
    assert_eq!(unavailable.interface_menu_selected_item(), Some(1));
    unavailable.handle_input(InputEvent::ShortPress, content);
    assert_eq!(
        unavailable.interface_menu_selected_item(),
        Some(POWER_MENU_ITEM)
    );

    let mut available = test_ui_state_with_shared_instance_config();
    available.handle_input(InputEvent::ShortPress, content);
    available.handle_input(InputEvent::LongPress, content);
    available.handle_input(InputEvent::ShortPress, content);
    assert_eq!(
        available.interface_menu_selected_item(),
        Some(SHARED_INSTANCE_CONFIG_MENU_ITEM)
    );
    assert_eq!(
        available.handle_input(InputEvent::LongPress, content),
        UiAction::CopySharedInstanceConfig
    );
}

#[test]
fn ordinary_tcp_menu_never_exports_shared_instance_config() {
    let cards = test_cards::<1>(CardKind::Tcp);
    let content = test_content(&cards);
    let mut state = test_ui_state_with_shared_instance_config();
    state.handle_input(InputEvent::ShortPress, content);
    state.handle_input(InputEvent::LongPress, content);
    state.handle_input(InputEvent::ShortPress, content);
    assert_eq!(state.interface_menu_selected_item(), Some(1));
    state.handle_input(InputEvent::ShortPress, content);
    assert_eq!(state.interface_menu_selected_item(), Some(POWER_MENU_ITEM));
}

#[test]
fn configured_wifi_menu_exposes_station_uplink_action() {
    let cards = test_cards::<1>(CardKind::WifiStation);
    let content = test_content(&cards);
    let mut state = test_ui_state();
    state.handle_input(InputEvent::ShortPress, content);
    state.handle_input(InputEvent::LongPress, content);

    assert_eq!(state.interface_menu_selected_item(), Some(POWER_MENU_ITEM));
    state.handle_input(InputEvent::ShortPress, content);
    assert_eq!(
        state.interface_menu_selected_item(),
        Some(STATION_UPLINK_MENU_ITEM)
    );
    assert_eq!(
        state.handle_input(InputEvent::LongPress, content),
        UiAction::ToggleStationUplink
    );
}

#[test]
fn unconfigured_wifi_menu_keeps_power_and_back_only() {
    let cards = test_cards::<1>(CardKind::Wifi);
    let content = test_content(&cards);
    let mut state = test_ui_state();
    state.handle_input(InputEvent::ShortPress, content);
    state.handle_input(InputEvent::LongPress, content);

    state.handle_input(InputEvent::ShortPress, content);
    assert_eq!(state.interface_menu_selected_item(), Some(1));
    state.handle_input(InputEvent::ShortPress, content);
    assert_eq!(state.interface_menu_selected_item(), Some(POWER_MENU_ITEM));
}

#[test]
fn lora_interface_menu_keeps_tune_and_reset() {
    let cards = test_cards::<1>(CardKind::LoRa);
    let content = test_content(&cards);
    let mut state = test_ui_state();
    state.handle_input(InputEvent::ShortPress, content);
    state.handle_input(InputEvent::LongPress, content);

    assert_eq!(state.interface_menu_selected_item(), Some(0));
    state.handle_input(InputEvent::ShortPress, content);
    assert_eq!(
        state.interface_menu_selected_item(),
        Some(LORA_TUNE_MENU_ITEM)
    );
    state.handle_input(InputEvent::ShortPress, content);
    assert_eq!(
        state.interface_menu_selected_item(),
        Some(LORA_RESET_MENU_ITEM)
    );
    state.handle_input(InputEvent::ShortPress, content);
    assert_eq!(state.interface_menu_selected_item(), Some(3));
    state.handle_input(InputEvent::ShortPress, content);
    assert_eq!(state.interface_menu_selected_item(), Some(0));
}

#[test]
fn lora_reset_is_distinct_from_saving_the_current_default_values() {
    let cards = test_cards::<1>(CardKind::LoRa);
    let content = test_content(&cards);
    let mut state = test_ui_state();
    state.handle_input(InputEvent::ShortPress, content);
    state.handle_input(InputEvent::LongPress, content);
    for _ in 0..LORA_RESET_MENU_ITEM {
        state.handle_input(InputEvent::ShortPress, content);
    }

    assert_eq!(
        state.handle_input(InputEvent::LongPress, content),
        UiAction::ResetLoRaProfile
    );
}

#[test]
fn long_press_opens_interface_menu_after_card_focus() {
    let cards = test_cards::<4>(CardKind::Usb);
    let content = test_content(&cards);
    let mut state = test_ui_state();
    state.handle_input(InputEvent::ShortPress, content);

    state.handle_input(InputEvent::LongPress, content);

    assert_eq!(state.selected_card_index(4), Some(0));
    assert_eq!(state.visible_start, 0);
    assert_eq!(state.interface_menu_selected_item(), Some(0));

    state.handle_input(InputEvent::ShortPress, content);

    assert_eq!(state.selected_card_index(4), Some(0));
    assert_eq!(state.interface_menu_selected_item(), Some(1));

    state.handle_input(InputEvent::LongPress, content);

    assert_eq!(state.selected_card_index(4), Some(0));
}
