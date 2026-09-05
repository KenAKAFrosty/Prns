use core::fmt::Write as _;

use embedded_graphics::mono_font::iso_8859_1::{FONT_4X6, FONT_5X8};
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;
use embedded_graphics::text::{Baseline, Text};
use personal_rns::interfaces::lora::{Modulation, RadioProfile};
use personal_rns::interfaces::subghz::SubGRegion;

use crate::screen::state::subg::{
    channel_count, current_channel, scroll_start, subg_mode_choices, subg_region_choice, CustomRow,
    EditMode, FreqPlace, FreqRow, PresetChoice, SubGModeChoice, SubGRegionChoice, SubGScreen,
    CUSTOM_ROWS, FREQ_ROWS, PRESET_CHOICES, SUBG_REGION_COUNT,
};

use super::super::layout::*;
use super::super::primitives::fill;

pub(in crate::screen) const SUBG_EDITOR_TOP: i32 = CARD_TOP + 2;
pub(in crate::screen) const SUBG_DOT_X: i32 = 1;
const SUBG_DOT_SIZE: u32 = 2;
const SUBG_ROW_TEXT_X: i32 = 6;
const SUBG_ROW_BACKING_H: u32 = 10;
const SUBG_VISIBLE_ROWS: usize = 7;

fn custom_row_label(row: CustomRow) -> &'static str {
    match row {
        CustomRow::SpreadingFactor => "SF",
        CustomRow::Bandwidth => "BW",
        CustomRow::CodingRate => "CR",
        CustomRow::FreqMhz => "MHz",
        CustomRow::FreqKhz => "kHz",
        CustomRow::TxPower => "Pwr",
        CustomRow::Save => "Save",
        CustomRow::Back => "Back",
    }
}

fn custom_row_value(row: CustomRow, profile: &RadioProfile) -> heapless::String<12> {
    let Modulation::Lora {
        spreading_factor,
        bandwidth,
        coding_rate,
    } = profile.modulation();
    let mut value = heapless::String::new();
    match row {
        CustomRow::SpreadingFactor => {
            let _ = write!(value, "{}", spreading_factor as u8);
        }
        CustomRow::Bandwidth => {
            let _ = write!(value, "{} kHz", bandwidth.hz() / 1_000);
        }
        CustomRow::CodingRate => {
            let _ = write!(value, "4/{}", coding_rate.denominator());
        }
        CustomRow::TxPower => {
            let _ = write!(value, "{} dBm", profile.tx_power().dbm());
        }
        CustomRow::FreqMhz | CustomRow::FreqKhz | CustomRow::Save | CustomRow::Back => {}
    }
    value
}

fn push_freq_digit(text: &mut heapless::String<16>, digit: u32, active: bool) {
    if active {
        let _ = write!(text, "[{digit}]");
    } else {
        let _ = write!(text, "{digit}");
    }
}

fn lora_freq_mhz_text(hz: u32, place: Option<FreqPlace>) -> heapless::String<16> {
    let mut text = heapless::String::new();
    push_freq_digit(
        &mut text,
        (hz / 100_000_000) % 10,
        place == Some(FreqPlace::Hundreds),
    );
    push_freq_digit(
        &mut text,
        (hz / 10_000_000) % 10,
        place == Some(FreqPlace::Tens),
    );
    push_freq_digit(
        &mut text,
        (hz / 1_000_000) % 10,
        place == Some(FreqPlace::Ones),
    );
    text
}

fn lora_freq_khz_text(hz: u32, place: Option<FreqPlace>) -> heapless::String<16> {
    let mut text = heapless::String::new();
    push_freq_digit(
        &mut text,
        (hz / 100_000) % 10,
        place == Some(FreqPlace::Tenths),
    );
    push_freq_digit(
        &mut text,
        (hz / 10_000) % 10,
        place == Some(FreqPlace::Hundredths),
    );
    push_freq_digit(
        &mut text,
        (hz / 1_000) % 10,
        place == Some(FreqPlace::Thousandths),
    );
    text
}

fn lora_custom_row_text(
    row: CustomRow,
    edit: EditMode,
    selected: bool,
    profile: &RadioProfile,
) -> heapless::String<16> {
    let mut text = heapless::String::new();
    if matches!(row, CustomRow::Save | CustomRow::Back) {
        let _ = text.push_str(custom_row_label(row));
        return text;
    }
    let label = custom_row_label(row);
    let hz = profile.frequency().hz();
    let active_place = match edit {
        EditMode::Freq { place } if selected => Some(place),
        _ => None,
    };
    match row {
        CustomRow::FreqMhz => {
            let value = lora_freq_mhz_text(hz, active_place);
            let _ = write!(text, "{value} {label}");
        }
        CustomRow::FreqKhz => {
            let value = lora_freq_khz_text(hz, active_place);
            let _ = write!(text, "{value} {label}");
        }
        _ => {
            let value = custom_row_value(row, profile);
            if selected && matches!(edit, EditMode::Field) {
                let _ = write!(text, "{label} [{value}]");
            } else {
                let _ = write!(text, "{label} {value}");
            }
        }
    }
    text
}

fn draw_subg_list_row<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    y: i32,
    text: &str,
    selected: bool,
) {
    let character_count = text.chars().count() as i32;
    let (font, character_width) = if lora_row_uses_compact_font(text) {
        (&FONT_4X6, FONT_4X6_CHAR_W)
    } else {
        (&FONT_5X8, FONT_5X8_CHAR_W)
    };
    let color = if selected {
        let width = (SUBG_ROW_TEXT_X + character_count * character_width + 1).max(0) as u32;
        let _ = Rectangle::new(Point::new(0, y - 1), Size::new(width, SUBG_ROW_BACKING_H))
            .into_styled(fill(BinaryColor::On))
            .draw(display);
        BinaryColor::Off
    } else {
        BinaryColor::On
    };
    let _ = Rectangle::new(
        Point::new(SUBG_DOT_X, y + 3),
        Size::new(SUBG_DOT_SIZE, SUBG_DOT_SIZE),
    )
    .into_styled(fill(color))
    .draw(display);
    let style = MonoTextStyle::new(font, color);
    let _ = Text::with_baseline(text, Point::new(SUBG_ROW_TEXT_X, y), style, Baseline::Top)
        .draw(display);
}

fn lora_row_uses_compact_font(text: &str) -> bool {
    SUBG_ROW_TEXT_X + text.chars().count() as i32 * FONT_5X8_CHAR_W > WIDTH
}

fn region_choice_label(index: usize) -> &'static str {
    match subg_region_choice(index) {
        SubGRegionChoice::Region(region) => region.label(),
        SubGRegionChoice::Cancel => "Cancel",
    }
}

fn draw_subg_region_picker<D: DrawTarget<Color = BinaryColor>>(display: &mut D, cursor: usize) {
    let start = scroll_start(cursor, SUBG_REGION_COUNT, SUBG_VISIBLE_ROWS);
    for slot in start..(start + SUBG_VISIBLE_ROWS).min(SUBG_REGION_COUNT) {
        let y = SUBG_EDITOR_TOP + (slot - start) as i32 * MENU_ITEM_STEP;
        draw_subg_list_row(display, y, region_choice_label(slot), slot == cursor);
    }
}

fn draw_subg_mode_picker<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    cursor: usize,
    region: SubGRegion,
) {
    let choices = subg_mode_choices(region);
    for (slot, choice) in choices.iter().enumerate() {
        let y = SUBG_EDITOR_TOP + slot as i32 * MENU_ITEM_STEP;
        let label = match choice {
            SubGModeChoice::AutoLoRa => "Auto LoRa",
            SubGModeChoice::ManualLoRa => "Manual LoRa",
            SubGModeChoice::Back => "Back",
        };
        draw_subg_list_row(display, y, label, slot == cursor);
    }
}

fn preset_choice_label(choice: PresetChoice) -> &'static str {
    match choice {
        PresetChoice::Preset(preset) => preset.label(),
        PresetChoice::Custom => "Custom",
        PresetChoice::Back => "Back",
    }
}

fn draw_lora_preset_picker<D: DrawTarget<Color = BinaryColor>>(display: &mut D, cursor: usize) {
    for (slot, &choice) in PRESET_CHOICES.iter().enumerate() {
        let y = SUBG_EDITOR_TOP + slot as i32 * MENU_ITEM_STEP;
        draw_subg_list_row(display, y, preset_choice_label(choice), slot == cursor);
    }
}

fn draw_lora_custom<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    cursor: CustomRow,
    edit: EditMode,
    profile: &RadioProfile,
) {
    for (slot, &row) in CUSTOM_ROWS.iter().enumerate() {
        let y = SUBG_EDITOR_TOP + slot as i32 * MENU_ITEM_STEP;
        let selected = row == cursor;
        let text = lora_custom_row_text(row, edit, selected, profile);
        draw_subg_list_row(display, y, &text, selected);
    }
}

fn lora_freq_row_text(
    row: FreqRow,
    edit: EditMode,
    selected: bool,
    profile: &RadioProfile,
) -> heapless::String<16> {
    let mut text = heapless::String::new();
    let hz = profile.frequency().hz();
    let active_place = match edit {
        EditMode::Freq { place } if selected => Some(place),
        _ => None,
    };
    match row {
        FreqRow::Channel => {
            let channel = current_channel(profile);
            if selected && matches!(edit, EditMode::Field) {
                let _ = write!(text, "Ch [{channel}]");
            } else {
                let count = channel_count(profile);
                let _ = write!(text, "Ch {channel}/{count}");
            }
        }
        FreqRow::Mhz => {
            let value = lora_freq_mhz_text(hz, active_place);
            let _ = write!(text, "{value} MHz");
        }
        FreqRow::Khz => {
            let value = lora_freq_khz_text(hz, active_place);
            let _ = write!(text, "{value} kHz");
        }
        FreqRow::Save => {
            let _ = text.push_str("Save");
        }
        FreqRow::Back => {
            let _ = text.push_str("Back");
        }
    }
    text
}

fn draw_lora_frequency<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    cursor: FreqRow,
    edit: EditMode,
    profile: &RadioProfile,
) {
    for (slot, &row) in FREQ_ROWS.iter().enumerate() {
        let y = SUBG_EDITOR_TOP + slot as i32 * MENU_ITEM_STEP;
        let selected = row == cursor;
        let text = lora_freq_row_text(row, edit, selected, profile);
        draw_subg_list_row(display, y, &text, selected);
    }
}

pub(in crate::screen::render) fn draw_subg_editor<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    screen: SubGScreen,
    profile: &RadioProfile,
) {
    match screen {
        SubGScreen::Region { cursor } => draw_subg_region_picker(display, cursor),
        SubGScreen::Mode { cursor } => draw_subg_mode_picker(display, cursor, profile.region()),
        SubGScreen::Preset { cursor } => draw_lora_preset_picker(display, cursor),
        SubGScreen::Frequency { cursor, edit } => {
            draw_lora_frequency(display, cursor, edit, profile)
        }
        SubGScreen::Custom { cursor, edit } => draw_lora_custom(display, cursor, edit, profile),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use personal_rns::interfaces::subghz::regions::us915::US915_AUTO_LORA_PROFILE;

    #[test]
    fn radio_values_use_their_natural_quantities_and_unit_order() {
        assert_eq!(
            custom_row_value(CustomRow::Bandwidth, &US915_AUTO_LORA_PROFILE).as_str(),
            "500 kHz"
        );
        assert_eq!(
            custom_row_value(CustomRow::TxPower, &US915_AUTO_LORA_PROFILE).as_str(),
            "22 dBm"
        );
        assert_eq!(
            lora_freq_row_text(
                FreqRow::Mhz,
                EditMode::Browsing,
                false,
                &US915_AUTO_LORA_PROFILE,
            )
            .as_str(),
            "921 MHz"
        );
        assert_eq!(
            lora_freq_row_text(
                FreqRow::Khz,
                EditMode::Browsing,
                false,
                &US915_AUTO_LORA_PROFILE,
            )
            .as_str(),
            "500 kHz"
        );
    }

    #[test]
    fn fractional_frequency_digits_are_rendered_as_kilohertz() {
        assert_eq!(lora_freq_khz_text(915_625_000, None).as_str(), "625");
        assert_eq!(
            lora_freq_khz_text(915_625_000, Some(FreqPlace::Tenths)).as_str(),
            "[6]25"
        );
    }

    #[test]
    fn selected_unit_bearing_rows_fit_the_constrained_display() {
        for row in [CustomRow::Bandwidth, CustomRow::TxPower] {
            let text = lora_custom_row_text(row, EditMode::Field, true, &US915_AUTO_LORA_PROFILE);
            let character_width = if lora_row_uses_compact_font(&text) {
                FONT_4X6_CHAR_W
            } else {
                FONT_5X8_CHAR_W
            };
            assert!(
                SUBG_ROW_TEXT_X + text.chars().count() as i32 * character_width <= WIDTH,
                "{text:?} exceeds the display width"
            );
        }
    }
}
