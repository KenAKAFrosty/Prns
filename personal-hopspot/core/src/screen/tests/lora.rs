use super::*;

fn lora_screen(state: &UiState) -> LoRaScreen {
    match state.mode {
        UiMode::LoRaEditor { screen, .. } => screen,
        other => panic!("not in the lora editor: {other:?}"),
    }
}

fn lora_working_profile(state: &UiState) -> RadioProfile {
    match state.mode {
        UiMode::LoRaEditor { profile, .. } => profile,
        other => panic!("not in the lora editor: {other:?}"),
    }
}

fn input(state: &mut UiState, event: InputEvent) -> UiAction {
    state.handle_input(event, test_content(&test_cards::<1>(CardKind::LoRa)))
}

fn tap(state: &mut UiState, times: usize) {
    for _ in 0..times {
        input(state, InputEvent::ShortPress);
    }
}

fn preset_choice_index(choice: PresetChoice) -> usize {
    PRESET_CHOICES
        .iter()
        .position(|&candidate| candidate == choice)
        .expect("preset choice is present")
}

fn tap_to_preset_choice(state: &mut UiState, choice: PresetChoice) {
    let current = match lora_screen(state) {
        LoRaScreen::Preset { cursor } => cursor,
        other => panic!("not on the preset list: {other:?}"),
    };
    let target = preset_choice_index(choice);
    tap(
        state,
        (target + PRESET_CHOICES.len() - current) % PRESET_CHOICES.len(),
    );
    assert_eq!(lora_screen(state), LoRaScreen::Preset { cursor: target });
}

#[test]
fn the_tuner_opens_on_the_region_list_at_the_current_region() {
    let mut state = test_ui_state();
    state.open_lora_editor(DEFAULT_915_PROFILE);
    assert_eq!(
        lora_screen(&state),
        LoRaScreen::Region {
            cursor: region_index(DEFAULT_915_PROFILE.region),
        }
    );
}

#[test]
fn accepting_a_region_snaps_the_default_frequency_and_power_ceiling() {
    let mut state = test_ui_state();
    state.open_lora_editor(DEFAULT_915_PROFILE);
    let target = region_index(Region::Eu868);
    tap(&mut state, target);
    input(&mut state, InputEvent::LongPress);

    assert!(matches!(lora_screen(&state), LoRaScreen::Preset { .. }));
    let profile = lora_working_profile(&state);
    assert_eq!(profile.region, Region::Eu868);
    assert_eq!(profile.frequency, Region::Eu868.default_frequency());
    assert_eq!(profile.tx_power, Region::Eu868.max_tx_power());
}

#[test]
fn cancel_from_the_region_list_returns_to_cards_without_committing() {
    let mut state = test_ui_state();
    state.open_lora_editor(DEFAULT_915_PROFILE);
    tap(
        &mut state,
        LORA_REGION_CANCEL - region_index(DEFAULT_915_PROFILE.region),
    );
    assert_eq!(
        lora_screen(&state),
        LoRaScreen::Region {
            cursor: LORA_REGION_CANCEL,
        }
    );
    let action = input(&mut state, InputEvent::LongPress);
    assert_eq!(action, UiAction::None);
    assert_eq!(state.mode, UiMode::Cards);
}

#[test]
fn a_nonpreset_modulation_lands_the_cursor_on_custom() {
    let mut state = test_ui_state();
    let mut profile = DEFAULT_915_PROFILE;
    profile.modulation = step_custom_row(DEFAULT_915_PROFILE, CustomRow::Bandwidth).modulation;
    state.open_lora_editor(profile);
    input(&mut state, InputEvent::LongPress);
    assert_eq!(
        lora_screen(&state),
        LoRaScreen::Preset {
            cursor: preset_choice_index(PresetChoice::Custom),
        }
    );
}

#[test]
fn choosing_a_named_preset_applies_it_then_opens_the_frequency_step() {
    let mut state = test_ui_state();
    state.open_lora_editor(DEFAULT_915_PROFILE);
    input(&mut state, InputEvent::LongPress);
    tap_to_preset_choice(&mut state, PresetChoice::Preset(ModemPreset::ShortFast));
    let action = input(&mut state, InputEvent::LongPress);

    assert_eq!(action, UiAction::None);
    assert_eq!(
        lora_screen(&state),
        LoRaScreen::Frequency {
            cursor: FreqRow::Channel,
            edit: EditMode::Browsing,
        }
    );
    assert_eq!(
        lora_working_profile(&state).modulation,
        ModemPreset::ShortFast.modulation()
    );
}

#[test]
fn the_channel_row_cycles_to_the_next_band_channel_center() {
    let mut state = test_ui_state();
    state.open_lora_editor(DEFAULT_915_PROFILE);
    input(&mut state, InputEvent::LongPress);
    tap_to_preset_choice(&mut state, PresetChoice::Preset(ModemPreset::ShortFast));
    input(&mut state, InputEvent::LongPress);
    assert_eq!(
        lora_screen(&state),
        LoRaScreen::Frequency {
            cursor: FreqRow::Channel,
            edit: EditMode::Browsing,
        }
    );
    input(&mut state, InputEvent::LongPress);
    input(&mut state, InputEvent::ShortPress);

    let hz = lora_working_profile(&state).frequency.hz();
    let (low, _) = Region::Us915.band();
    assert_eq!((hz - low - 125_000) % 250_000, 0);
    assert_eq!(hz, 915_375_000);
}

#[test]
fn the_frequency_step_dials_a_channel_then_saves_with_the_preset() {
    let mut state = test_ui_state();
    state.open_lora_editor(DEFAULT_915_PROFILE);
    input(&mut state, InputEvent::LongPress);
    tap_to_preset_choice(&mut state, PresetChoice::Preset(ModemPreset::ShortFast));
    input(&mut state, InputEvent::LongPress);
    assert_eq!(
        lora_screen(&state),
        LoRaScreen::Frequency {
            cursor: FreqRow::Channel,
            edit: EditMode::Browsing,
        }
    );
    tap(&mut state, 2);
    input(&mut state, InputEvent::LongPress);
    tap(&mut state, 6);
    input(&mut state, InputEvent::LongPress);
    tap(&mut state, 2);
    input(&mut state, InputEvent::LongPress);
    tap(&mut state, 5);
    input(&mut state, InputEvent::LongPress);
    assert_eq!(lora_working_profile(&state).frequency.hz(), 915_625_000);

    tap(&mut state, 1);
    let committed = input(&mut state, InputEvent::LongPress);
    let mut expected = DEFAULT_915_PROFILE;
    expected.modulation = ModemPreset::ShortFast.modulation();
    expected.frequency = Frequency::new(915_625_000);
    assert_eq!(committed, UiAction::SetLoRaProfile(expected));
    assert_eq!(state.mode, UiMode::Cards);
}

#[test]
fn back_from_the_frequency_step_returns_to_the_preset_list() {
    let mut state = test_ui_state();
    state.open_lora_editor(DEFAULT_915_PROFILE);
    input(&mut state, InputEvent::LongPress);
    tap_to_preset_choice(&mut state, PresetChoice::Preset(ModemPreset::ShortFast));
    input(&mut state, InputEvent::LongPress);
    tap(&mut state, 4);
    assert_eq!(
        lora_screen(&state),
        LoRaScreen::Frequency {
            cursor: FreqRow::Back,
            edit: EditMode::Browsing,
        }
    );
    input(&mut state, InputEvent::LongPress);
    assert!(matches!(lora_screen(&state), LoRaScreen::Preset { .. }));
}

fn open_custom(state: &mut UiState) {
    state.open_lora_editor(DEFAULT_915_PROFILE);
    input(state, InputEvent::LongPress);
    tap_to_preset_choice(state, PresetChoice::Custom);
    input(state, InputEvent::LongPress);
    assert_eq!(
        lora_screen(state),
        LoRaScreen::Custom {
            cursor: CustomRow::SpreadingFactor,
            edit: EditMode::Browsing,
        }
    );
}

#[test]
fn custom_grabs_a_field_steps_it_and_saves() {
    let mut state = test_ui_state();
    open_custom(&mut state);
    tap(&mut state, 1);
    input(&mut state, InputEvent::LongPress);
    tap(&mut state, 1);
    input(&mut state, InputEvent::LongPress);
    tap(&mut state, 5);
    let committed = input(&mut state, InputEvent::LongPress);

    let mut expected = DEFAULT_915_PROFILE;
    expected.modulation = step_custom_row(DEFAULT_915_PROFILE, CustomRow::Bandwidth).modulation;
    assert_eq!(committed, UiAction::SetLoRaProfile(expected));
}

#[test]
fn custom_dials_a_fractional_frequency_across_the_two_rows() {
    let mut state = test_ui_state();
    open_custom(&mut state);
    tap(&mut state, 4);
    assert_eq!(
        lora_screen(&state),
        LoRaScreen::Custom {
            cursor: CustomRow::FreqKhz,
            edit: EditMode::Browsing,
        }
    );
    input(&mut state, InputEvent::LongPress);
    tap(&mut state, 6);
    input(&mut state, InputEvent::LongPress);
    tap(&mut state, 2);
    input(&mut state, InputEvent::LongPress);
    tap(&mut state, 5);
    input(&mut state, InputEvent::LongPress);
    assert_eq!(lora_working_profile(&state).frequency.hz(), 915_625_000);

    tap(&mut state, 2);
    match input(&mut state, InputEvent::LongPress) {
        UiAction::SetLoRaProfile(profile) => assert_eq!(profile.frequency.hz(), 915_625_000),
        other => panic!("expected SetLoRaProfile, got {other:?}"),
    }
}

#[test]
fn custom_clamps_an_out_of_band_frequency_to_the_region_edge() {
    let mut state = test_ui_state();
    open_custom(&mut state);
    tap(&mut state, 3);
    input(&mut state, InputEvent::LongPress);
    input(&mut state, InputEvent::LongPress);
    tap(&mut state, 2);
    input(&mut state, InputEvent::LongPress);
    input(&mut state, InputEvent::LongPress);
    assert_eq!(lora_working_profile(&state).frequency.hz(), 928_000_000);
}

#[test]
fn back_from_custom_returns_to_the_preset_list() {
    let mut state = test_ui_state();
    open_custom(&mut state);
    tap(&mut state, 7);
    assert_eq!(
        lora_screen(&state),
        LoRaScreen::Custom {
            cursor: CustomRow::Back,
            edit: EditMode::Browsing,
        }
    );
    input(&mut state, InputEvent::LongPress);
    assert!(matches!(lora_screen(&state), LoRaScreen::Preset { .. }));
}

#[test]
fn each_lora_screen_renders_its_selected_row_within_bounds() {
    let screens = [
        LoRaScreen::Region {
            cursor: LORA_REGION_CANCEL,
        },
        LoRaScreen::Preset {
            cursor: PRESET_CHOICES.len() - 1,
        },
        LoRaScreen::Frequency {
            cursor: FreqRow::Back,
            edit: EditMode::Browsing,
        },
        LoRaScreen::Custom {
            cursor: CustomRow::Back,
            edit: EditMode::Browsing,
        },
    ];
    for screen in screens {
        let mut display = MockDisplay::new();
        display.set_allow_overdraw(true);
        display.set_allow_out_of_bounds_drawing(true);
        let mut state = test_ui_state();
        state.open_lora_editor(DEFAULT_915_PROFILE);
        if let UiMode::LoRaEditor { profile, .. } = state.mode {
            state.mode = UiMode::LoRaEditor { screen, profile };
        }

        render_with_state(&mut display, &[], PowerSnapshot::UNKNOWN, &state);

        assert_eq!(
            display.get_pixel(Point::new(LORA_DOT_X, LORA_EDITOR_TOP + 3)),
            Some(BinaryColor::On)
        );
    }
}
