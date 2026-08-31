use core::fmt::Write as _;

use embedded_graphics::mono_font::iso_8859_1::{FONT_4X6, FONT_5X8};
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;
use embedded_graphics::text::{Baseline, Text};

use crate::screen::state::ble_group::{
    BleGroupCustomRow, BleGroupEdit, BleGroupName, BleGroupScreen, BLE_GROUP_CHOICES,
    BLE_GROUP_CUSTOM_ROWS,
};

use super::super::layout::*;
use super::super::primitives::fill;
use super::lora::LORA_EDITOR_TOP;

const EDITOR_DOT_X: i32 = 1;
const EDITOR_DOT_SIZE: u32 = 2;
const EDITOR_ROW_TEXT_X: i32 = 6;
const EDITOR_ROW_BACKING_H: u32 = 10;

fn draw_editor_row<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    y: i32,
    text: &str,
    selected: bool,
) {
    let character_count = text.chars().count() as i32;
    let (font, character_width) = if EDITOR_ROW_TEXT_X + character_count * FONT_5X8_CHAR_W > WIDTH {
        (&FONT_4X6, FONT_4X6_CHAR_W)
    } else {
        (&FONT_5X8, FONT_5X8_CHAR_W)
    };
    let color = if selected {
        let width = (EDITOR_ROW_TEXT_X + character_count * character_width + 1).max(0) as u32;
        let _ = Rectangle::new(Point::new(0, y - 1), Size::new(width, EDITOR_ROW_BACKING_H))
            .into_styled(fill(BinaryColor::On))
            .draw(display);
        BinaryColor::Off
    } else {
        BinaryColor::On
    };
    let _ = Rectangle::new(
        Point::new(EDITOR_DOT_X, y + 3),
        Size::new(EDITOR_DOT_SIZE, EDITOR_DOT_SIZE),
    )
    .into_styled(fill(color))
    .draw(display);
    let style = MonoTextStyle::new(font, color);
    let _ = Text::with_baseline(text, Point::new(EDITOR_ROW_TEXT_X, y), style, Baseline::Top)
        .draw(display);
}

fn name_row_text(name: BleGroupName, edit: BleGroupEdit, selected: bool) -> heapless::String<40> {
    let mut text = heapless::String::new();
    if name.as_str().is_empty() {
        let _ = text.push_str(if selected && matches!(edit, BleGroupEdit::Char { .. }) {
            "[]"
        } else {
            "_"
        });
        return text;
    }
    let active = match edit {
        BleGroupEdit::Char { index } if selected => Some(index),
        _ => None,
    };
    for (index, byte) in name.as_str().bytes().enumerate() {
        let ch = byte as char;
        if active == Some(index) {
            let _ = write!(text, "[{ch}]");
        } else {
            let _ = text.push(ch);
        }
    }
    if active == Some(name.as_str().len()) {
        let _ = text.push_str("[_]");
    }
    text
}

pub(in crate::screen::render) fn draw_ble_group_editor<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    screen: BleGroupScreen,
    name: BleGroupName,
) {
    match screen {
        BleGroupScreen::Choice { cursor } => {
            for (slot, choice) in BLE_GROUP_CHOICES.iter().enumerate() {
                let y = LORA_EDITOR_TOP + slot as i32 * MENU_ITEM_STEP;
                draw_editor_row(display, y, choice.label(), slot == cursor);
            }
        }
        BleGroupScreen::Custom { cursor, edit } => {
            for (slot, row) in BLE_GROUP_CUSTOM_ROWS.iter().enumerate() {
                let y = LORA_EDITOR_TOP + slot as i32 * MENU_ITEM_STEP;
                let selected = *row == cursor;
                let text = match row {
                    BleGroupCustomRow::Name => name_row_text(name, edit, selected),
                    _ => {
                        let mut text = heapless::String::new();
                        let _ = text.push_str(row.label());
                        text
                    }
                };
                draw_editor_row(display, y, &text, selected);
            }
        }
    }
}
