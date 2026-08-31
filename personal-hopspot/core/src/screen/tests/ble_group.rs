use super::*;
use crate::screen::state::ble_group::{
    BleGroupChoice, BleGroupCustomRow, BleGroupEdit, BleGroupName, BleGroupScreen,
    BLE_GROUP_CHOICES, DEFAULT_BLE_GROUP,
};

fn ble_screen(state: &UiState) -> BleGroupScreen {
    match state.mode {
        UiMode::BleGroupEditor { screen, .. } => screen,
        other => panic!("not in the ble group editor: {other:?}"),
    }
}

fn ble_name(state: &UiState) -> BleGroupName {
    match state.mode {
        UiMode::BleGroupEditor { name, .. } => name,
        other => panic!("not in the ble group editor: {other:?}"),
    }
}

fn input(state: &mut UiState, event: InputEvent) -> UiAction {
    state.handle_input(event, test_content(&test_cards::<1>(CardKind::Ble)))
}

fn tap(state: &mut UiState, times: usize) {
    for _ in 0..times {
        input(state, InputEvent::ShortPress);
    }
}

fn test_ui_state_with_ble_group() -> UiState {
    UiState::new(UiConfiguration {
        storage_limits: DisplayedStorageLimits::DYNAMIC,
        user_blanking: UserBlanking::unavailable(),
        access_point: AccessPointState::Unsupported,
        shared_instance_config_export: SharedInstanceConfigExport::Unavailable,
        gnss: GnssAvailability::Unavailable,
        ble_group_editor: BleGroupEditor::Available,
    })
}

fn open_menu(state: &mut UiState) {
    let cards = test_cards::<1>(CardKind::Ble);
    let content = test_content(&cards);
    state.handle_input(InputEvent::ShortPress, content);
    state.handle_input(InputEvent::LongPress, content);
}

#[test]
fn ble_group_menu_item_opens_the_editor() {
    let mut state = test_ui_state_with_ble_group();
    open_menu(&mut state);
    tap(&mut state, 1);
    assert_eq!(
        state.interface_menu_selected_item(),
        Some(BLE_GROUP_MENU_ITEM)
    );
    assert_eq!(
        input(&mut state, InputEvent::LongPress),
        UiAction::OpenBleGroupEditor
    );
}

#[test]
fn use_default_commits_reticulum() {
    let mut state = test_ui_state_with_ble_group();
    state.open_ble_group_editor("mt-leg-a");
    assert_eq!(
        ble_screen(&state),
        BleGroupScreen::Choice {
            cursor: BLE_GROUP_CHOICES
                .iter()
                .position(|choice| *choice == BleGroupChoice::Custom)
                .expect("custom is present"),
        }
    );
    tap(&mut state, 2);
    let action = input(&mut state, InputEvent::LongPress);
    assert_eq!(
        action,
        UiAction::SetBleDiscoveryGroup(BleGroupName::reticulum())
    );
    assert_eq!(state.mode, UiMode::Cards);
}

#[test]
fn custom_opens_a_name_editor_and_save_commits() {
    let mut state = test_ui_state_with_ble_group();
    state.open_ble_group_editor("mt-leg-a");
    assert_eq!(input(&mut state, InputEvent::LongPress), UiAction::None);
    assert_eq!(ble_name(&state).as_str(), "mt-leg-a");
    tap(&mut state, 2);
    assert_eq!(
        ble_screen(&state),
        BleGroupScreen::Custom {
            cursor: BleGroupCustomRow::Save,
            edit: BleGroupEdit::Browsing,
        }
    );
    assert_eq!(
        input(&mut state, InputEvent::LongPress),
        UiAction::SetBleDiscoveryGroup(BleGroupName::parse("mt-leg-a").expect("valid group"))
    );
}

#[test]
fn back_from_the_choice_list_returns_to_cards() {
    let mut state = test_ui_state_with_ble_group();
    state.open_ble_group_editor(DEFAULT_BLE_GROUP);
    tap(&mut state, 2);
    assert_eq!(input(&mut state, InputEvent::LongPress), UiAction::None);
    assert_eq!(state.mode, UiMode::Cards);
}

#[test]
fn del_removes_the_last_character() {
    let mut state = test_ui_state_with_ble_group();
    state.open_ble_group_editor("mt");
    input(&mut state, InputEvent::LongPress);
    tap(&mut state, 1);
    assert_eq!(
        ble_screen(&state),
        BleGroupScreen::Custom {
            cursor: BleGroupCustomRow::Del,
            edit: BleGroupEdit::Browsing,
        }
    );
    input(&mut state, InputEvent::LongPress);
    assert_eq!(ble_name(&state).as_str(), "m");
}

#[test]
fn long_press_past_the_last_character_leaves_name_editing() {
    let mut state = test_ui_state_with_ble_group();
    state.open_ble_group_editor("ab");
    input(&mut state, InputEvent::LongPress);
    input(&mut state, InputEvent::LongPress);
    assert!(matches!(
        ble_screen(&state),
        BleGroupScreen::Custom {
            cursor: BleGroupCustomRow::Name,
            edit: BleGroupEdit::Char { index: 0 },
        }
    ));
    input(&mut state, InputEvent::LongPress);
    assert!(matches!(
        ble_screen(&state),
        BleGroupScreen::Custom {
            cursor: BleGroupCustomRow::Name,
            edit: BleGroupEdit::Char { index: 1 },
        }
    ));
    input(&mut state, InputEvent::LongPress);
    assert!(matches!(
        ble_screen(&state),
        BleGroupScreen::Custom {
            cursor: BleGroupCustomRow::Name,
            edit: BleGroupEdit::Char { index: 2 },
        }
    ));
    input(&mut state, InputEvent::LongPress);
    assert_eq!(
        ble_screen(&state),
        BleGroupScreen::Custom {
            cursor: BleGroupCustomRow::Name,
            edit: BleGroupEdit::Browsing,
        }
    );
    assert_eq!(ble_name(&state).as_str(), "ab");
}
