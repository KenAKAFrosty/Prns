use super::*;
use personal_rns::interfaces::{DiscoveryGroupSet, InterfaceKind};

fn group_editor_state() -> (UiState, [Card; 1]) {
    let mut state = test_ui_state();
    state.discovery_groups = super::super::DiscoveryGroupEditorAvailability::Available;
    let mut cards = test_cards::<1>(CardKind::Ble);
    cards[0].id = InterfaceId::new([InterfaceKind::BluetoothAuto as u8, 0, 0, 0, 0, 0, 0, 0]);
    (state, cards)
}

#[test]
fn group_menu_is_available_only_when_the_target_can_apply_it() {
    let (mut available, cards) = group_editor_state();
    let content = test_content(&cards);
    assert_eq!(
        available.handle_input(InputEvent::ShortPress, content),
        UiAction::None
    );
    assert_eq!(
        available.handle_input(InputEvent::LongPress, content),
        UiAction::None
    );
    assert_eq!(
        available.handle_input(InputEvent::ShortPress, content),
        UiAction::None
    );
    assert_eq!(
        available.handle_input(InputEvent::LongPress, content),
        UiAction::OpenDiscoveryGroupsEditor(cards[0].id())
    );

    let mut unavailable = test_ui_state();
    unavailable.handle_input(InputEvent::ShortPress, content);
    unavailable.handle_input(InputEvent::LongPress, content);
    unavailable.handle_input(InputEvent::ShortPress, content);
    assert_eq!(
        unavailable.handle_input(InputEvent::LongPress, content),
        UiAction::None
    );
}

#[test]
fn editor_adds_a_portable_name_and_commits_the_complete_set() {
    let (mut state, cards) = group_editor_state();
    let content = test_content(&cards);
    state.open_discovery_groups_editor(cards[0].id(), &DiscoveryGroupSet::reticulum());

    // Select Add, enter `a`, finish editing, then select Save.
    assert_eq!(
        state.handle_input(InputEvent::ShortPress, content),
        UiAction::None
    );
    assert_eq!(
        state.handle_input(InputEvent::LongPress, content),
        UiAction::None
    );
    assert_eq!(
        state.handle_input(InputEvent::ShortPress, content),
        UiAction::None
    );
    assert_eq!(
        state.handle_input(InputEvent::ShortPress, content),
        UiAction::None
    );
    assert_eq!(
        state.handle_input(InputEvent::LongPress, content),
        UiAction::None
    );
    assert_eq!(
        state.handle_input(InputEvent::LongPress, content),
        UiAction::None
    );
    assert_eq!(
        state.handle_input(InputEvent::ShortPress, content),
        UiAction::None
    );
    assert_eq!(
        state.handle_input(InputEvent::ShortPress, content),
        UiAction::None
    );

    if state.handle_input(InputEvent::LongPress, content) != UiAction::ReplaceDiscoveryGroups {
        panic!("save must return an atomic replacement");
    }
    let replacement = state
        .take_discovery_group_replacement()
        .expect("save must retain the atomic replacement until it is taken");
    assert_eq!(replacement.interface_id(), cards[0].id());
    let groups = replacement.groups();
    assert_eq!(
        groups
            .iter()
            .map(|group| group.as_str())
            .collect::<std::vec::Vec<_>>(),
        std::vec!["a", "reticulum"]
    );
}

#[test]
fn editor_never_removes_the_last_group() {
    let (mut state, cards) = group_editor_state();
    let content = test_content(&cards);
    state.open_discovery_groups_editor(cards[0].id(), &DiscoveryGroupSet::reticulum());

    // Open the only group, select Remove, and hold. The group remains and Save stays valid.
    state.handle_input(InputEvent::LongPress, content);
    state.handle_input(InputEvent::ShortPress, content);
    state.handle_input(InputEvent::LongPress, content);
    state.handle_input(InputEvent::ShortPress, content);
    state.handle_input(InputEvent::ShortPress, content);
    if state.handle_input(InputEvent::LongPress, content) != UiAction::ReplaceDiscoveryGroups {
        panic!("save must remain available");
    }
    let replacement = state
        .take_discovery_group_replacement()
        .expect("save must retain the atomic replacement until it is taken");
    assert_eq!(replacement.groups(), &DiscoveryGroupSet::reticulum());
}

#[test]
fn editor_cannot_clear_the_last_group_as_an_edit() {
    let (mut state, cards) = group_editor_state();
    let content = test_content(&cards);
    state.open_discovery_groups_editor(cards[0].id(), &DiscoveryGroupSet::reticulum());

    state.handle_input(InputEvent::LongPress, content); // Open the only item.
    state.handle_input(InputEvent::LongPress, content); // Edit it.
    state.handle_input(InputEvent::ShortPress, content); // Select backspace.
    for _ in 0.."reticulum".len() {
        state.handle_input(InputEvent::LongPress, content);
    }
    for _ in 0..39 {
        state.handle_input(InputEvent::ShortPress, content); // Wrap back to Done.
    }

    assert_eq!(
        state.handle_input(InputEvent::LongPress, content),
        UiAction::None
    );
    assert_eq!(state.visible_notice(), Some(UiNotice::GroupsInvalid));
}
