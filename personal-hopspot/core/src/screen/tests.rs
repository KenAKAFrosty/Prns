use core::convert::Infallible;

use embedded_graphics::mock_display::MockDisplay;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use heapless::Vec as HVec;
use personal_rns::interfaces::lora::core::{
    Frequency, ModemPreset, RadioProfile, Region, DEFAULT_915_PROFILE,
};
use personal_rns::interfaces::InterfaceId;
use personal_rns::storage::{DisplayedStorageLimits, StorageCapacity};

use super::limits::{build_limit_rows, LimitValue};
use super::render::cards::draw_card;
use super::render::glyphs::{
    draw_battery, draw_clock, draw_interface_icon, draw_lightning, draw_link, draw_person,
};
use super::render::layout::{
    ACTIVITY_TEXT_X, CARD_H, CARD_SLOT_STEP, CARD_TOP, FIRST_CARD_WITH_GLOBAL_TOP,
    FOOTER_FOURTH_LINE_OFFSET, FOOTER_SECOND_LINE_OFFSET, GLOBAL_BACKING_H, GLOBAL_BACKING_X,
    GLOBAL_BACKING_Y, GLOBAL_ICON_X, GLOBAL_ROW_H, GLOBAL_ROW_TOP, HEIGHT, MENU_BACKING_X,
    MENU_DIVIDER_Y, MENU_HEADER_Y, MENU_ITEM_STEP, MENU_ITEM_TOP, MENU_MARK_X, MENU_REASON_X,
    NAME_BACKING_X, NAME_BACKING_Y, NAME_ICON_X, NAME_LINE_Y, STAT_ICON_X, STAT_TEXT_X, WIDTH,
};
use super::render::menus::draw_interface_menu;
use super::render::menus::lora::{LORA_DOT_X, LORA_EDITOR_TOP};
use super::render::metrics::{
    compact_numeric_width, draw_compact_number, fmt_activity_age, fmt_count, fmt_rate_bytes_per_sec,
};
use super::state::lora::{
    region_index, step_custom_row, CustomRow, EditMode, FreqRow, LoRaScreen, PresetChoice,
    LORA_REGION_CANCEL, PRESET_CHOICES,
};
use super::state::{
    UiMode, ANNOUNCE_MENU_ITEM, LORA_RESET_MENU_ITEM, LORA_TUNE_MENU_ITEM, OLED_OFF_MENU_ITEM,
    POWER_MENU_ITEM, POWER_ONLY_MENU_ITEMS, SLEEP_MENU_ITEM,
};
use super::*;

const TEST_WIDTH: usize = WIDTH as usize;
const TEST_HEIGHT: usize = HEIGHT as usize;

struct PanelDisplay {
    pixels: [[Option<BinaryColor>; TEST_WIDTH]; TEST_HEIGHT],
}

impl PanelDisplay {
    fn new() -> Self {
        Self {
            pixels: [[None; TEST_WIDTH]; TEST_HEIGHT],
        }
    }

    fn get_pixel(&self, point: Point) -> Option<BinaryColor> {
        if point.x < 0 || point.y < 0 || point.x >= WIDTH || point.y >= HEIGHT {
            return None;
        }
        self.pixels[point.y as usize][point.x as usize]
    }
}

impl DrawTarget for PanelDisplay {
    type Color = BinaryColor;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            if point.x >= 0 && point.y >= 0 && point.x < WIDTH && point.y < HEIGHT {
                self.pixels[point.y as usize][point.x as usize] = Some(color);
            }
        }
        Ok(())
    }
}

impl OriginDimensions for PanelDisplay {
    fn size(&self) -> Size {
        Size::new(WIDTH as u32, HEIGHT as u32)
    }
}

fn test_card(label: &'static str) -> Card {
    Card {
        id: InterfaceId::new([0; 8]),
        kind: CardKind::Usb,
        label: card_label(label),
        selected: false,
        liveness: Liveness::Live,
        failure_reason: None,
        tx_bytes: 0,
        rx_bytes: 0,
        links: 0,
        destinations: 0,
        rate_bytes_per_sec: 0,
        last_activity_secs: None,
    }
}

fn has_on_pixel(
    display: &PanelDisplay,
    xs: core::ops::Range<i32>,
    ys: core::ops::Range<i32>,
) -> bool {
    for y in ys {
        for x in xs.clone() {
            if display.get_pixel(Point::new(x, y)) == Some(BinaryColor::On) {
                return true;
            }
        }
    }
    false
}

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
fn short_press_cycles_global_then_cards_and_pages_visible_window() {
    let mut state = UiState::new();
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
    let mut state = UiState::new();

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
    let mut state = UiState::new();

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
    let mut state = UiState::new();
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
    let mut state = UiState::new();
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
    let mut state = UiState::new();
    state.set_display_power_capable(true);
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
fn limit_rows_use_the_supplied_storage_limits() {
    let rows = build_limit_rows(DisplayedStorageLimits {
        upstream_app_destinations: StorageCapacity::Fixed(4),
        held_identities: StorageCapacity::Fixed(2),
        blackholed_identities: StorageCapacity::Fixed(8),
        blackhole_reason_bytes: StorageCapacity::Fixed(64),
        ..DisplayedStorageLimits::DYNAMIC
    });

    let app_dst = rows
        .iter()
        .find(|row| row.label == "AppDst")
        .map(|row| row.value);
    let held_id = rows
        .iter()
        .find(|row| row.label == "HeldID")
        .map(|row| row.value);
    let blackholes = rows
        .iter()
        .find(|row| row.label == "BlkHole")
        .map(|row| row.value);
    let blackhole_reason_bytes = rows
        .iter()
        .find(|row| row.label == "BlkWhy")
        .map(|row| row.value);

    assert_eq!(app_dst, Some(LimitValue::Count(4)));
    assert_eq!(held_id, Some(LimitValue::Count(2)));
    assert_eq!(blackholes, Some(LimitValue::Count(8)));
    assert_eq!(blackhole_reason_bytes, Some(LimitValue::Bytes(64)));
}

#[test]
fn long_press_on_back_closes_the_global_menu() {
    let mut state = UiState::new();
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
    let mut state = UiState::new();
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
fn non_lora_interface_menus_cycle_power_and_back_only() {
    let mut state = UiState::new();
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
    let mut state = UiState::new();
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
    let mut state = UiState::new();
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

fn tap(state: &mut UiState, times: usize) {
    for _ in 0..times {
        state.handle_input(InputEvent::ShortPress, 1, None);
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
    let mut state = UiState::new();
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
    let mut state = UiState::new();
    state.open_lora_editor(DEFAULT_915_PROFILE);
    let target = region_index(Region::Eu868);
    tap(&mut state, target);
    state.handle_input(InputEvent::LongPress, 1, None);

    assert!(matches!(lora_screen(&state), LoRaScreen::Preset { .. }));
    let profile = lora_working_profile(&state);
    assert_eq!(profile.region, Region::Eu868);
    assert_eq!(profile.frequency, Region::Eu868.default_frequency());
    assert_eq!(profile.tx_power, Region::Eu868.max_tx_power());
}

#[test]
fn cancel_from_the_region_list_returns_to_cards_without_committing() {
    let mut state = UiState::new();
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
    let action = state.handle_input(InputEvent::LongPress, 1, None);
    assert_eq!(action, UiAction::None);
    assert_eq!(state.mode, UiMode::Cards);
}

#[test]
fn a_nonpreset_modulation_lands_the_cursor_on_custom() {
    let mut state = UiState::new();
    let mut profile = DEFAULT_915_PROFILE;
    profile.modulation = step_custom_row(DEFAULT_915_PROFILE, CustomRow::Bandwidth).modulation;
    state.open_lora_editor(profile);
    state.handle_input(InputEvent::LongPress, 1, None);
    assert_eq!(
        lora_screen(&state),
        LoRaScreen::Preset {
            cursor: preset_choice_index(PresetChoice::Custom),
        }
    );
}

#[test]
fn choosing_a_named_preset_applies_it_then_opens_the_frequency_step() {
    let mut state = UiState::new();
    state.open_lora_editor(DEFAULT_915_PROFILE);
    state.handle_input(InputEvent::LongPress, 1, None);
    tap_to_preset_choice(&mut state, PresetChoice::Preset(ModemPreset::ShortFast));
    let action = state.handle_input(InputEvent::LongPress, 1, None);

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
    let mut state = UiState::new();
    state.open_lora_editor(DEFAULT_915_PROFILE);
    state.handle_input(InputEvent::LongPress, 1, None);
    tap_to_preset_choice(&mut state, PresetChoice::Preset(ModemPreset::ShortFast));
    state.handle_input(InputEvent::LongPress, 1, None);
    assert_eq!(
        lora_screen(&state),
        LoRaScreen::Frequency {
            cursor: FreqRow::Channel,
            edit: EditMode::Browsing,
        }
    );
    state.handle_input(InputEvent::LongPress, 1, None);
    state.handle_input(InputEvent::ShortPress, 1, None);

    let hz = lora_working_profile(&state).frequency.hz();
    let (low, _) = Region::Us915.band();
    assert_eq!((hz - low - 125_000) % 250_000, 0);
    assert_eq!(hz, 915_375_000);
}

#[test]
fn the_frequency_step_dials_a_channel_then_saves_with_the_preset() {
    let mut state = UiState::new();
    state.open_lora_editor(DEFAULT_915_PROFILE);
    state.handle_input(InputEvent::LongPress, 1, None);
    tap_to_preset_choice(&mut state, PresetChoice::Preset(ModemPreset::ShortFast));
    state.handle_input(InputEvent::LongPress, 1, None);
    assert_eq!(
        lora_screen(&state),
        LoRaScreen::Frequency {
            cursor: FreqRow::Channel,
            edit: EditMode::Browsing,
        }
    );
    tap(&mut state, 2);
    state.handle_input(InputEvent::LongPress, 1, None);
    tap(&mut state, 6);
    state.handle_input(InputEvent::LongPress, 1, None);
    tap(&mut state, 2);
    state.handle_input(InputEvent::LongPress, 1, None);
    tap(&mut state, 5);
    state.handle_input(InputEvent::LongPress, 1, None);
    assert_eq!(lora_working_profile(&state).frequency.hz(), 915_625_000);

    tap(&mut state, 1);
    let committed = state.handle_input(InputEvent::LongPress, 1, None);
    let mut expected = DEFAULT_915_PROFILE;
    expected.modulation = ModemPreset::ShortFast.modulation();
    expected.frequency = Frequency::new(915_625_000);
    assert_eq!(committed, UiAction::SetLoRaProfile(expected));
    assert_eq!(state.mode, UiMode::Cards);
}

#[test]
fn back_from_the_frequency_step_returns_to_the_preset_list() {
    let mut state = UiState::new();
    state.open_lora_editor(DEFAULT_915_PROFILE);
    state.handle_input(InputEvent::LongPress, 1, None);
    tap_to_preset_choice(&mut state, PresetChoice::Preset(ModemPreset::ShortFast));
    state.handle_input(InputEvent::LongPress, 1, None);
    tap(&mut state, 4);
    assert_eq!(
        lora_screen(&state),
        LoRaScreen::Frequency {
            cursor: FreqRow::Back,
            edit: EditMode::Browsing,
        }
    );
    state.handle_input(InputEvent::LongPress, 1, None);
    assert!(matches!(lora_screen(&state), LoRaScreen::Preset { .. }));
}

fn open_custom(state: &mut UiState) {
    state.open_lora_editor(DEFAULT_915_PROFILE);
    state.handle_input(InputEvent::LongPress, 1, None);
    tap_to_preset_choice(state, PresetChoice::Custom);
    state.handle_input(InputEvent::LongPress, 1, None);
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
    let mut state = UiState::new();
    open_custom(&mut state);
    tap(&mut state, 1);
    state.handle_input(InputEvent::LongPress, 1, None);
    tap(&mut state, 1);
    state.handle_input(InputEvent::LongPress, 1, None);
    tap(&mut state, 5);
    let committed = state.handle_input(InputEvent::LongPress, 1, None);

    let mut expected = DEFAULT_915_PROFILE;
    expected.modulation = step_custom_row(DEFAULT_915_PROFILE, CustomRow::Bandwidth).modulation;
    assert_eq!(committed, UiAction::SetLoRaProfile(expected));
}

#[test]
fn custom_dials_a_fractional_frequency_across_the_two_rows() {
    let mut state = UiState::new();
    open_custom(&mut state);
    tap(&mut state, 4);
    assert_eq!(
        lora_screen(&state),
        LoRaScreen::Custom {
            cursor: CustomRow::FreqKhz,
            edit: EditMode::Browsing,
        }
    );
    state.handle_input(InputEvent::LongPress, 1, None);
    tap(&mut state, 6);
    state.handle_input(InputEvent::LongPress, 1, None);
    tap(&mut state, 2);
    state.handle_input(InputEvent::LongPress, 1, None);
    tap(&mut state, 5);
    state.handle_input(InputEvent::LongPress, 1, None);
    assert_eq!(lora_working_profile(&state).frequency.hz(), 915_625_000);

    tap(&mut state, 2);
    match state.handle_input(InputEvent::LongPress, 1, None) {
        UiAction::SetLoRaProfile(profile) => assert_eq!(profile.frequency.hz(), 915_625_000),
        other => panic!("expected SetLoRaProfile, got {other:?}"),
    }
}

#[test]
fn custom_clamps_an_out_of_band_frequency_to_the_region_edge() {
    let mut state = UiState::new();
    open_custom(&mut state);
    tap(&mut state, 3);
    state.handle_input(InputEvent::LongPress, 1, None);
    state.handle_input(InputEvent::LongPress, 1, None);
    tap(&mut state, 2);
    state.handle_input(InputEvent::LongPress, 1, None);
    state.handle_input(InputEvent::LongPress, 1, None);
    assert_eq!(lora_working_profile(&state).frequency.hz(), 928_000_000);
}

#[test]
fn back_from_custom_returns_to_the_preset_list() {
    let mut state = UiState::new();
    open_custom(&mut state);
    tap(&mut state, 7);
    assert_eq!(
        lora_screen(&state),
        LoRaScreen::Custom {
            cursor: CustomRow::Back,
            edit: EditMode::Browsing,
        }
    );
    state.handle_input(InputEvent::LongPress, 1, None);
    assert!(matches!(lora_screen(&state), LoRaScreen::Preset { .. }));
}

#[test]
fn draw_with_state_marks_selected_card_below_global_row() {
    let mut display = MockDisplay::new();
    display.set_allow_overdraw(true);
    display.set_allow_out_of_bounds_drawing(true);
    let cards = [test_card("A"), test_card("B")];
    let mut state = UiState::new();
    state.handle_input(InputEvent::ShortPress, cards.len(), Some(CardKind::Usb));

    draw_with_state(&mut display, &cards, BatteryState::Unknown, &state);

    let selected_top = FIRST_CARD_WITH_GLOBAL_TOP;
    assert_eq!(state.selected_card(cards.len()), Some(0));
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
fn draw_with_state_renders_selected_global_row() {
    let mut display = MockDisplay::new();
    display.set_allow_overdraw(true);
    display.set_allow_out_of_bounds_drawing(true);
    let cards = [test_card("USB")];
    let state = UiState::new();

    draw_with_state(&mut display, &cards, BatteryState::Unknown, &state);

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
fn draw_with_state_footer_scrolls_after_the_last_card() {
    let cards = [test_card("USB"), test_card("BLE"), test_card("WiFi")];
    let mut state = UiState::new();
    let footer = UiFooter::new("Docs", Some("127.0.0.1"));
    for _ in 0..4 {
        state.handle_input_with_footer(
            InputEvent::ShortPress,
            cards.len(),
            true,
            Some(CardKind::Usb),
        );
    }

    assert_eq!(state.selected_card(cards.len()), None);
    assert_eq!(state.visible_start_with_footer(cards.len(), true), 3);

    let mut display = PanelDisplay::new();
    draw_with_state_footer_at(
        &mut display,
        &cards,
        BatteryState::Unknown,
        &state,
        Some(footer),
        0,
    );
    assert!(has_on_pixel(
        &display,
        0..WIDTH,
        (CARD_TOP + CARD_SLOT_STEP)..(CARD_TOP + CARD_SLOT_STEP + FOOTER_SECOND_LINE_OFFSET + 8)
    ));
}

#[test]
fn draw_with_state_footer_can_show_softap_docs_details() {
    let cards = [test_card("USB"), test_card("BLE"), test_card("WiFi")];
    let mut state = UiState::new();
    let footer = UiFooter::with_lines(
        "WifiAP",
        Some("Hopspot-EW53"),
        Some("docs @"),
        Some("192.168.4.1"),
    );
    for _ in 0..4 {
        state.handle_input_with_footer(
            InputEvent::ShortPress,
            cards.len(),
            true,
            Some(CardKind::Usb),
        );
    }

    let mut display = PanelDisplay::new();
    draw_with_state_footer_at(
        &mut display,
        &cards,
        BatteryState::Unknown,
        &state,
        Some(footer),
        0,
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
    let mut state = UiState::new();

    assert_eq!(
        state.handle_input_with_footer(InputEvent::ShortPress, 1, true, Some(CardKind::Usb)),
        UiAction::None
    );
    assert_eq!(state.selected_card(1), Some(0));

    assert_eq!(
        state.handle_input_with_footer(InputEvent::ShortPress, 1, true, None),
        UiAction::None
    );
    assert_eq!(state.selected_card(1), None);

    assert_eq!(
        state.handle_input_with_footer(InputEvent::LongPress, 1, true, None),
        UiAction::OpenDocs
    );
}

#[test]
fn draw_with_state_scrolls_global_row_out_of_card_window() {
    let mut display = MockDisplay::new();
    display.set_allow_overdraw(true);
    display.set_allow_out_of_bounds_drawing(true);
    let cards = [test_card("A"), test_card("B"), test_card("C")];
    let mut state = UiState::new();
    state.handle_input(InputEvent::ShortPress, cards.len(), Some(CardKind::Usb));
    state.handle_input(InputEvent::ShortPress, cards.len(), Some(CardKind::Usb));
    state.handle_input(InputEvent::ShortPress, cards.len(), Some(CardKind::Usb));

    draw_with_state(&mut display, &cards, BatteryState::Unknown, &state);

    assert_eq!(state.selected_card(cards.len()), Some(2));
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
fn draw_with_state_renders_global_menu() {
    let mut display = MockDisplay::new();
    display.set_allow_overdraw(true);
    display.set_allow_out_of_bounds_drawing(true);
    let cards = [test_card("USB")];
    let mut state = UiState::new();
    state.handle_input(InputEvent::LongPress, cards.len(), Some(CardKind::Usb));

    draw_with_state(&mut display, &cards, BatteryState::Unknown, &state);

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
fn draw_with_state_renders_selected_interface_menu() {
    let mut display = MockDisplay::new();
    display.set_allow_overdraw(true);
    display.set_allow_out_of_bounds_drawing(true);
    let cards = [
        test_card("USB"),
        Card {
            id: InterfaceId::new([0; 8]),
            kind: CardKind::Ble,
            label: card_label("BLE"),
            selected: false,
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
    let mut state = UiState::new();
    state.handle_input(InputEvent::ShortPress, cards.len(), Some(CardKind::Usb));
    state.handle_input(InputEvent::ShortPress, cards.len(), Some(CardKind::Usb));
    state.handle_input(InputEvent::LongPress, cards.len(), Some(CardKind::Usb));

    draw_with_state(&mut display, &cards, BatteryState::Unknown, &state);

    assert_eq!(state.selected_card(cards.len()), Some(1));
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

#[test]
fn interface_menu_draws_detail_rows_below_actions() {
    let mut display = PanelDisplay::new();
    let mut card = test_card("WiFi/LAN");
    card.kind = CardKind::Wifi;
    let mut rows = InterfaceMenuDetailRows::new();
    push_interface_menu_info(&mut rows, "STA", "None");
    push_interface_menu_info(&mut rows, "AP", "Hopspot-EW53");
    let _ = push_supervisor_peer_rows(
        &mut rows,
        [SupervisorPeerMenuStatus {
            id: InterfaceId::new([0, 0xab, 0xcd, 0, 0, 0, 0, 0]),
            liveness: Liveness::Live,
        }],
    );

    draw_interface_menu(&mut display, &card, POWER_MENU_ITEM, &rows);

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
        let mut state = UiState::new();
        state.open_lora_editor(DEFAULT_915_PROFILE);
        if let UiMode::LoRaEditor { profile, .. } = state.mode {
            state.mode = UiMode::LoRaEditor { screen, profile };
        }

        draw_with_state(&mut display, &[], BatteryState::Unknown, &state);

        assert_eq!(
            display.get_pixel(Point::new(LORA_DOT_X, LORA_EDITOR_TOP + 3)),
            Some(BinaryColor::On)
        );
    }
}

#[test]
fn count_formatter_uses_blank_base_then_metric_suffixes() {
    assert_eq!(fmt_count(0).as_str(), "0");
    assert_eq!(fmt_count(999).as_str(), "999");
    assert_eq!(fmt_count(1_000).as_str(), "1.0K");
    assert_eq!(fmt_count(12_345).as_str(), "12K");
    assert_eq!(fmt_count(999_999).as_str(), "999K");
    assert_eq!(fmt_count(1_000_000).as_str(), "1.0M");
    assert_eq!(fmt_count(1_234_567_890).as_str(), "1.2B");
}

#[test]
fn live_stat_formatters_stay_compact() {
    assert_eq!(fmt_rate_bytes_per_sec(0).as_str(), "0B");
    assert_eq!(fmt_rate_bytes_per_sec(999).as_str(), "999B");
    assert_eq!(fmt_rate_bytes_per_sec(1_200).as_str(), "1.2K");
    assert_eq!(fmt_rate_bytes_per_sec(12_000).as_str(), "12K");
    assert_eq!(fmt_rate_bytes_per_sec(999_999).as_str(), "999K");
    assert_eq!(fmt_rate_bytes_per_sec(1_234_567).as_str(), "1.2M");
    assert_eq!(fmt_rate_bytes_per_sec(1_234_567_890).as_str(), "1.2G");

    assert_eq!(fmt_activity_age(None).as_str(), "-");
    assert_eq!(fmt_activity_age(Some(0)).as_str(), "now");
    assert_eq!(fmt_activity_age(Some(3)).as_str(), "3s");
    assert_eq!(fmt_activity_age(Some(123)).as_str(), "2m");
    assert_eq!(fmt_activity_age(Some(7200)).as_str(), "2h");
}

#[test]
fn compact_number_draws_decimal_as_single_pixel() {
    let mut display = MockDisplay::new();
    display.set_allow_overdraw(true);

    draw_compact_number(&mut display, "1.2K/s", Point::new(0, 0), BinaryColor::On);

    assert_eq!(compact_numeric_width("1.2K/s"), 25);
    assert_eq!(display.get_pixel(Point::new(5, 6)), Some(BinaryColor::On));
    assert_eq!(display.get_pixel(Point::new(6, 6)), None);
    assert_eq!(display.get_pixel(Point::new(19, 2)), Some(BinaryColor::On));
    assert_eq!(display.get_pixel(Point::new(18, 3)), Some(BinaryColor::On));
    assert_eq!(display.get_pixel(Point::new(17, 4)), Some(BinaryColor::On));
    assert_eq!(display.get_pixel(Point::new(19, 3)), None);
}

#[test]
fn usb_icon_draws_full_width_tongue() {
    let mut display = MockDisplay::new();
    display.set_allow_overdraw(true);

    draw_interface_icon(&mut display, 0, 0, CardKind::Usb, BinaryColor::On);

    display.assert_pattern(&[
        "    #    ",
        "    #    ",
        "#########",
        "#       #",
        "#       #",
        "#########",
        "#       #",
        "#########",
    ]);
}

#[test]
fn ble_icon_reads_as_bluetooth_rune() {
    let mut display = MockDisplay::new();

    draw_interface_icon(&mut display, 0, 0, CardKind::Ble, BinaryColor::On);

    display.assert_pattern(&[
        "    #    ",
        "    ##   ",
        "  # # #  ",
        "   ###   ",
        "    #    ",
        "   ###   ",
        "  # # #  ",
        "    ##   ",
        "    #    ",
    ]);
}

#[test]
fn unknown_battery_dash_is_symmetric() {
    let mut display = MockDisplay::new();

    draw_battery(&mut display, 2, 0, BatteryState::Unknown, true);

    assert_eq!(display.get_pixel(Point::new(5, 4)), None);
    for x in 6..=12 {
        assert_eq!(display.get_pixel(Point::new(x, 4)), Some(BinaryColor::Off));
    }
    assert_eq!(display.get_pixel(Point::new(13, 4)), None);
}

#[test]
fn charging_battery_blinks_the_current_tier() {
    let mut display = MockDisplay::new();
    display.set_allow_overdraw(true);

    draw_battery(&mut display, 2, 0, BatteryState::Charging(62), true);

    assert_eq!(display.get_pixel(Point::new(7, 4)), Some(BinaryColor::Off));
    assert_eq!(display.get_pixel(Point::new(10, 4)), Some(BinaryColor::Off));
    assert_eq!(display.get_pixel(Point::new(13, 4)), Some(BinaryColor::Off));
}

#[test]
fn charging_battery_hides_only_the_current_tier_on_the_off_phase() {
    let mut display = MockDisplay::new();
    display.set_allow_overdraw(true);

    draw_battery(&mut display, 2, 0, BatteryState::Charging(62), false);

    assert_eq!(display.get_pixel(Point::new(7, 4)), None);
    assert_eq!(display.get_pixel(Point::new(10, 4)), Some(BinaryColor::Off));
    assert_eq!(display.get_pixel(Point::new(13, 4)), Some(BinaryColor::Off));
}

#[test]
fn charging_battery_draws_right_side_plug_until_full() {
    let mut display = MockDisplay::new();
    display.set_allow_overdraw(true);

    draw_battery(&mut display, 2, 0, BatteryState::Charging(62), true);

    for x in 17..=20 {
        assert_eq!(display.get_pixel(Point::new(x, 4)), Some(BinaryColor::Off));
    }
    assert_eq!(display.get_pixel(Point::new(21, 3)), Some(BinaryColor::Off));
    assert_eq!(display.get_pixel(Point::new(23, 4)), None);
}

#[test]
fn full_charging_battery_uses_a_steady_filled_shape_without_the_plug() {
    let mut display = MockDisplay::new();
    display.set_allow_overdraw(true);

    draw_battery(&mut display, 2, 0, BatteryState::Charging(100), false);

    assert_eq!(display.get_pixel(Point::new(2, 0)), Some(BinaryColor::Off));
    assert_eq!(display.get_pixel(Point::new(16, 8)), Some(BinaryColor::Off));
    assert_eq!(display.get_pixel(Point::new(0, 4)), Some(BinaryColor::Off));
    assert_eq!(display.get_pixel(Point::new(21, 3)), None);
}

#[test]
fn person_icon_reads_as_peer_count_glyph() {
    let mut display = MockDisplay::new();

    draw_person(&mut display, 0, 0);

    display.assert_pattern(&[
        "   ###   ",
        "  #   #  ",
        "  #   #  ",
        "   ###   ",
        "  #   #  ",
        " #     # ",
    ]);
}

#[test]
fn link_icon_reads_as_chain_glyph() {
    let mut display = MockDisplay::new();
    display.set_allow_overdraw(true);

    draw_link(&mut display, 0, 0);

    display.assert_pattern(&[
        " ##  ## ", "#      #", "#   #  #", "#  #   #", "#      #", " ##  ## ",
    ]);
}

#[test]
fn lightning_icon_reads_as_rate_glyph() {
    let mut display = MockDisplay::new();

    draw_lightning(&mut display, 0, 0);

    display.assert_pattern(&["   # ", "  #  ", " ####", "  #  ", " #   ", "#    "]);
}

#[test]
fn clock_icon_reads_as_activity_age_glyph() {
    let mut display = MockDisplay::new();

    draw_clock(&mut display, 0, 0);

    display.assert_pattern(&[
        "  ###  ", " #   # ", "#  #  #", "#  ## #", "#     #", " #   # ", "  ###  ",
    ]);
}

#[test]
fn wifi_icon_reads_as_status_arc_glyph() {
    let mut display = MockDisplay::new();

    draw_interface_icon(&mut display, 0, 0, CardKind::Wifi, BinaryColor::On);

    display.assert_pattern(&[
        "  #####  ",
        " #     # ",
        "#       #",
        "         ",
        "   ###   ",
        "  #   #  ",
        "         ",
        "    #    ",
        "   ###   ",
    ]);
}

#[test]
fn lora_icon_reads_as_long_range_radio_glyph() {
    let mut display = MockDisplay::new();

    draw_interface_icon(&mut display, 0, 0, CardKind::LoRa, BinaryColor::On);

    display.assert_pattern(&[
        "#   #   #",
        " #  #  # ",
        "  # # #  ",
        "   ###   ",
        "    #    ",
        "    #    ",
        "    #    ",
        "   ###   ",
        "  #####  ",
    ]);
}

#[test]
fn esp_now_icon_reads_as_omni_broadcast_glyph() {
    let mut display = MockDisplay::new();

    draw_interface_icon(&mut display, 0, 0, CardKind::EspNow, BinaryColor::On);

    display.assert_pattern(&[
        "         ",
        "#       #",
        " #     # ",
        "  # # #  ",
        "   ###   ",
        "  # # #  ",
        " #     # ",
        "#       #",
    ]);
}

#[test]
fn card_stacks_traffic_and_moves_peers_right() {
    let mut display = MockDisplay::new();
    display.set_allow_overdraw(true);
    let card = Card {
        id: InterfaceId::new([0; 8]),
        kind: CardKind::Usb,
        label: card_label("USB"),
        selected: false,
        liveness: Liveness::Live,
        failure_reason: None,
        tx_bytes: 123,
        rx_bytes: 456,
        links: 5,
        destinations: 7,
        rate_bytes_per_sec: 12_345,
        last_activity_secs: Some(3),
    };

    draw_card(&mut display, 0, &card);

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
    assert_eq!(display.get_pixel(Point::new(5, 32)), Some(BinaryColor::On));
    assert_eq!(display.get_pixel(Point::new(38, 32)), Some(BinaryColor::On));
}

#[test]
fn large_link_and_peer_counts_fit_right_column() {
    let mut display = MockDisplay::new();
    display.set_allow_overdraw(true);
    let card = Card {
        id: InterfaceId::new([0; 8]),
        kind: CardKind::Wifi,
        label: card_label("WiFi"),
        selected: false,
        liveness: Liveness::Live,
        failure_reason: None,
        tx_bytes: 999_999_999,
        rx_bytes: 999_999_999,
        links: 999_999,
        destinations: 1_234_567_890,
        rate_bytes_per_sec: 999_999_999,
        last_activity_secs: Some(3599),
    };

    draw_card(&mut display, 0, &card);

    assert_eq!(compact_numeric_width("999K"), 20);
    assert_eq!(compact_numeric_width("1.2B"), 17);
    assert!(STAT_TEXT_X + compact_numeric_width("999K") < WIDTH);
    assert!(8 + compact_numeric_width("999M") < STAT_ICON_X);
    assert!(ACTIVITY_TEXT_X + compact_numeric_width("-") < WIDTH);
}

#[test]
fn offline_card_centers_status_and_hides_metrics() {
    let mut display = MockDisplay::new();
    display.set_allow_overdraw(true);
    let card = Card {
        id: InterfaceId::new([0; 8]),
        kind: CardKind::EspNow,
        label: card_label("ESP-NOW"),
        selected: false,
        liveness: Liveness::Failed,
        failure_reason: Some("BlueZ GATT Channels >1; set Channels=1"),
        tx_bytes: 123,
        rx_bytes: 456,
        links: 5,
        destinations: 7,
        rate_bytes_per_sec: 123,
        last_activity_secs: Some(12),
    };

    draw_card(&mut display, 0, &card);

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
        label: card_label("WiFi"),
        selected: true,
        liveness: Liveness::Live,
        failure_reason: None,
        tx_bytes: 0,
        rx_bytes: 0,
        links: 0,
        destinations: 0,
        rate_bytes_per_sec: 0,
        last_activity_secs: None,
    };

    draw_card(&mut display, 0, &card);

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
