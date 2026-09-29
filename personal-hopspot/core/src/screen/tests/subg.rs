use super::*;

fn subg_screen(state: &UiState) -> SubGScreen {
    match state.mode {
        UiMode::SubGEditor { screen, .. } => screen,
        other => panic!("not in the SubG editor: {other:?}"),
    }
}

fn subg_working_profile(state: &UiState) -> RadioProfile {
    match state.mode {
        UiMode::SubGEditor { profile, .. } => profile,
        other => panic!("not in the SubG editor: {other:?}"),
    }
}

fn manual_configuration(profile: RadioProfile) -> SubGConfiguration {
    SubGConfiguration::manual_lora_for(profile.region(), ManualLoRaParameters::from(profile))
        .unwrap()
}

fn open_profile(state: &mut UiState, profile: RadioProfile) {
    state.open_subg_editor(SubGConfigurationState::Configured(manual_configuration(
        profile,
    )));
}

fn open_us_manual(state: &mut UiState) {
    state.open_subg_editor(SubGConfigurationState::Configured(Us915::auto_lora()));
    input(state, InputEvent::LongPress);
    tap(state, 1);
    input(state, InputEvent::LongPress);
    assert!(matches!(subg_screen(state), SubGScreen::Preset { .. }));
}

fn input(state: &mut UiState, event: InputEvent) -> UiAction {
    state.handle_input(
        event,
        test_content(&test_cards::<1>(CardKind::SubG(SubGCardState::AutoLoRa))),
    )
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
    let current = match subg_screen(state) {
        SubGScreen::Preset { cursor } => cursor,
        other => panic!("not on the preset list: {other:?}"),
    };
    let target = preset_choice_index(choice);
    tap(
        state,
        (target + PRESET_CHOICES.len() - current) % PRESET_CHOICES.len(),
    );
    assert_eq!(subg_screen(state), SubGScreen::Preset { cursor: target });
}

#[test]
fn region_picker_derives_from_the_regulatory_inventory() {
    for (index, region) in RegulatoryRegion::ALL.into_iter().enumerate() {
        let region = SubGRegion::Regulated(region);
        assert_eq!(subg_region_choice(index), SubGRegionChoice::Region(region));
        assert_eq!(region_index(region), index);
    }
    assert_eq!(
        subg_region_choice(RegulatoryRegion::ALL.len()),
        SubGRegionChoice::Region(SubGRegion::Custom)
    );
    assert_eq!(
        region_index(SubGRegion::Custom),
        RegulatoryRegion::ALL.len()
    );
    assert_eq!(
        subg_region_choice(SUBG_REGION_CANCEL),
        SubGRegionChoice::Cancel
    );
}

#[test]
fn the_subg_editor_opens_on_the_region_list_at_the_current_region() {
    let mut state = test_ui_state();
    state.open_subg_editor(SubGConfigurationState::Configured(Us915::auto_lora()));
    assert_eq!(
        subg_screen(&state),
        SubGScreen::Region {
            cursor: region_index(US915_AUTO_LORA_PROFILE.region()),
        }
    );
}

#[test]
fn an_unconfigured_interface_starts_region_first_without_committing_a_default() {
    let mut state = test_ui_state();
    state.open_subg_editor(SubGConfigurationState::Unconfigured);
    assert_eq!(
        subg_screen(&state),
        SubGScreen::Region {
            cursor: region_index(SubGRegion::Regulated(RegulatoryRegion::Us915)),
        }
    );
}

#[test]
fn us915_auto_lora_commits_directly_from_the_mode_screen() {
    let mut state = test_ui_state();
    state.open_subg_editor(SubGConfigurationState::Unconfigured);
    input(&mut state, InputEvent::LongPress);
    assert_eq!(subg_screen(&state), SubGScreen::Mode { cursor: 0 });
    assert_eq!(
        input(&mut state, InputEvent::LongPress),
        UiAction::SetSubGConfiguration(Us915::auto_lora())
    );
}

#[test]
fn product_ui_does_not_offer_turbo_or_auto_outside_us915() {
    let mut state = test_ui_state();
    state.open_subg_editor(SubGConfigurationState::Unconfigured);
    tap(
        &mut state,
        region_index(SubGRegion::Regulated(RegulatoryRegion::Eu868)),
    );
    input(&mut state, InputEvent::LongPress);
    assert_eq!(subg_screen(&state), SubGScreen::Mode { cursor: 0 });
    input(&mut state, InputEvent::ShortPress);
    assert_eq!(subg_screen(&state), SubGScreen::Mode { cursor: 1 });
    input(&mut state, InputEvent::ShortPress);
    assert_eq!(subg_screen(&state), SubGScreen::Mode { cursor: 0 });
    input(&mut state, InputEvent::LongPress);
    assert!(matches!(subg_screen(&state), SubGScreen::Preset { .. }));
}

#[test]
fn product_ui_keeps_us915_turbo_unselectable() {
    let mut state = test_ui_state();
    state.open_subg_editor(SubGConfigurationState::Unconfigured);
    input(&mut state, InputEvent::LongPress);
    tap(&mut state, 2);
    assert_eq!(subg_screen(&state), SubGScreen::Mode { cursor: 2 });
    input(&mut state, InputEvent::LongPress);
    assert!(matches!(subg_screen(&state), SubGScreen::Region { .. }));
}

#[test]
fn accepting_a_region_snaps_the_default_frequency_and_power_ceiling() {
    let mut state = test_ui_state();
    state.open_subg_editor(SubGConfigurationState::Configured(Us915::auto_lora()));
    let region = SubGRegion::Regulated(RegulatoryRegion::Eu868);
    let target = region_index(region);
    tap(&mut state, target);
    input(&mut state, InputEvent::LongPress);
    assert_eq!(subg_screen(&state), SubGScreen::Mode { cursor: 0 });
    input(&mut state, InputEvent::LongPress);

    assert!(matches!(subg_screen(&state), SubGScreen::Preset { .. }));
    let profile = subg_working_profile(&state);
    assert_eq!(profile.region(), region);
    assert_eq!(profile.frequency(), region.default_frequency());
    assert_eq!(profile.tx_power(), region.max_tx_power());
}

#[test]
fn cancel_from_the_region_list_returns_to_cards_without_committing() {
    let mut state = test_ui_state();
    state.open_subg_editor(SubGConfigurationState::Configured(Us915::auto_lora()));
    tap(
        &mut state,
        SUBG_REGION_CANCEL - region_index(US915_AUTO_LORA_PROFILE.region()),
    );
    assert_eq!(
        subg_screen(&state),
        SubGScreen::Region {
            cursor: SUBG_REGION_CANCEL,
        }
    );
    let action = input(&mut state, InputEvent::LongPress);
    assert_eq!(action, UiAction::None);
    assert_eq!(state.mode, UiMode::Cards);
}

#[test]
fn a_nonpreset_modulation_lands_the_cursor_on_custom() {
    let mut state = test_ui_state();
    let profile = US915_AUTO_LORA_PROFILE
        .with_modulation(
            step_custom_row(US915_AUTO_LORA_PROFILE, CustomRow::Bandwidth).modulation(),
        )
        .unwrap();
    open_profile(&mut state, profile);
    input(&mut state, InputEvent::LongPress);
    tap(&mut state, 1);
    input(&mut state, InputEvent::LongPress);
    assert_eq!(
        subg_screen(&state),
        SubGScreen::Preset {
            cursor: preset_choice_index(PresetChoice::Custom),
        }
    );
}

#[test]
fn choosing_a_named_preset_applies_it_then_opens_the_frequency_step() {
    let mut state = test_ui_state();
    open_us_manual(&mut state);
    tap_to_preset_choice(&mut state, PresetChoice::Preset(ModemPreset::ShortFast));
    let action = input(&mut state, InputEvent::LongPress);

    assert_eq!(action, UiAction::None);
    assert_eq!(
        subg_screen(&state),
        SubGScreen::Frequency {
            cursor: FreqRow::Channel,
            edit: EditMode::Browsing,
        }
    );
    assert_eq!(
        subg_working_profile(&state).modulation(),
        ModemPreset::ShortFast.modulation()
    );
}

#[test]
fn the_channel_row_cycles_to_the_next_band_channel_center() {
    let mut state = test_ui_state();
    open_us_manual(&mut state);
    tap_to_preset_choice(&mut state, PresetChoice::Preset(ModemPreset::ShortFast));
    input(&mut state, InputEvent::LongPress);
    assert_eq!(
        subg_screen(&state),
        SubGScreen::Frequency {
            cursor: FreqRow::Channel,
            edit: EditMode::Browsing,
        }
    );
    input(&mut state, InputEvent::LongPress);
    input(&mut state, InputEvent::ShortPress);

    let hz = subg_working_profile(&state).frequency().hz();
    let low = RegulatoryRegion::Us915.frequency_range().minimum().hz();
    assert_eq!((hz - low - 125_000) % 250_000, 0);
    assert_eq!(hz, 921_875_000);
}

#[test]
fn the_frequency_step_dials_a_channel_then_saves_with_the_preset() {
    let mut state = test_ui_state();
    open_us_manual(&mut state);
    tap_to_preset_choice(&mut state, PresetChoice::Preset(ModemPreset::ShortFast));
    input(&mut state, InputEvent::LongPress);
    assert_eq!(
        subg_screen(&state),
        SubGScreen::Frequency {
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
    assert_eq!(subg_working_profile(&state).frequency().hz(), 921_125_000);

    tap(&mut state, 1);
    let committed = input(&mut state, InputEvent::LongPress);
    let expected = US915_AUTO_LORA_PROFILE
        .with_modulation(ModemPreset::ShortFast.modulation())
        .unwrap()
        .with_frequency(Frequency::new(921_125_000))
        .unwrap();
    assert_eq!(
        committed,
        UiAction::SetSubGConfiguration(manual_configuration(expected))
    );
    assert_eq!(state.mode, UiMode::Cards);
}

#[test]
fn back_from_the_frequency_step_returns_to_the_preset_list() {
    let mut state = test_ui_state();
    open_us_manual(&mut state);
    tap_to_preset_choice(&mut state, PresetChoice::Preset(ModemPreset::ShortFast));
    input(&mut state, InputEvent::LongPress);
    tap(&mut state, 4);
    assert_eq!(
        subg_screen(&state),
        SubGScreen::Frequency {
            cursor: FreqRow::Back,
            edit: EditMode::Browsing,
        }
    );
    input(&mut state, InputEvent::LongPress);
    assert!(matches!(subg_screen(&state), SubGScreen::Preset { .. }));
}

fn open_custom(state: &mut UiState) {
    open_us_manual(state);
    tap_to_preset_choice(state, PresetChoice::Custom);
    input(state, InputEvent::LongPress);
    assert_eq!(
        subg_screen(state),
        SubGScreen::Custom {
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

    let expected = US915_AUTO_LORA_PROFILE
        .with_modulation(
            step_custom_row(US915_AUTO_LORA_PROFILE, CustomRow::Bandwidth).modulation(),
        )
        .unwrap();
    assert_eq!(
        committed,
        UiAction::SetSubGConfiguration(manual_configuration(expected))
    );
}

#[test]
fn custom_dials_a_fractional_frequency_across_the_two_rows() {
    let mut state = test_ui_state();
    open_custom(&mut state);
    tap(&mut state, 4);
    assert_eq!(
        subg_screen(&state),
        SubGScreen::Custom {
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
    assert_eq!(subg_working_profile(&state).frequency().hz(), 921_125_000);

    tap(&mut state, 2);
    match input(&mut state, InputEvent::LongPress) {
        UiAction::SetSubGConfiguration(configuration) => {
            let personal_rns::interfaces::subghz::ResolvedSubGMode::LoRa(profile) =
                configuration.resolve();
            assert_eq!(profile.frequency().hz(), 921_125_000);
        }
        other => panic!("expected SetSubGConfiguration, got {other:?}"),
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
    assert_eq!(subg_working_profile(&state).frequency().hz(), 927_750_000);
}

#[test]
fn back_from_custom_returns_to_the_preset_list() {
    let mut state = test_ui_state();
    open_custom(&mut state);
    tap(&mut state, 7);
    assert_eq!(
        subg_screen(&state),
        SubGScreen::Custom {
            cursor: CustomRow::Back,
            edit: EditMode::Browsing,
        }
    );
    input(&mut state, InputEvent::LongPress);
    assert!(matches!(subg_screen(&state), SubGScreen::Preset { .. }));
}

#[test]
fn each_subg_screen_renders_its_selected_row_within_bounds() {
    let screens = [
        SubGScreen::Region {
            cursor: SUBG_REGION_CANCEL,
        },
        SubGScreen::Mode { cursor: 2 },
        SubGScreen::Preset {
            cursor: PRESET_CHOICES.len() - 1,
        },
        SubGScreen::Frequency {
            cursor: FreqRow::Back,
            edit: EditMode::Browsing,
        },
        SubGScreen::Custom {
            cursor: CustomRow::Back,
            edit: EditMode::Browsing,
        },
    ];
    for screen in screens {
        let mut display = MockDisplay::new();
        display.set_allow_overdraw(true);
        display.set_allow_out_of_bounds_drawing(true);
        let mut state = test_ui_state();
        state.open_subg_editor(SubGConfigurationState::Configured(Us915::auto_lora()));
        if let UiMode::SubGEditor { profile, .. } = state.mode {
            state.mode = UiMode::SubGEditor { screen, profile };
        }

        render_with_state(&mut display, &[], PowerSnapshot::UNKNOWN, &state);

        assert_eq!(
            display.get_pixel(Point::new(SUBG_DOT_X, SUBG_EDITOR_TOP + 3)),
            Some(BinaryColor::On)
        );
    }
}
