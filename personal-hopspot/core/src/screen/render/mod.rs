pub(in crate::screen) mod cards;
pub(in crate::screen) mod glyphs;
pub(in crate::screen) mod layout;
pub(in crate::screen) mod menus;
pub(in crate::screen) mod metrics;
mod primitives;

use embedded_graphics::mono_font::ascii::FONT_6X10;
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::text::{Baseline, Text};

use crate::battery::BatteryState;

use super::limits::build_limit_rows;
use super::model::{Card, InterfaceMenuDetails, LocalDocsAccess};
use super::state::{focus_item_count_with_footer, visible_start_for, UiMode, UiState};
use cards::{draw_card_peek, draw_card_with_selection, draw_footer, draw_global_row};
use glyphs::draw_title_bar;
use layout::*;
use menus::lora::draw_lora_editor;
use menus::{
    draw_global_menu, draw_interface_menu, draw_limits_page, draw_notice, draw_radio_confirm,
    draw_sleeping,
};

pub struct RenderFrame<'frame, 'docs> {
    pub cards: &'frame [Card],
    pub battery: BatteryState,
    pub state: &'frame UiState,
    pub local_docs: Option<&'frame LocalDocsAccess<'docs>>,
    pub interface_menu_details: &'frame InterfaceMenuDetails,
    pub animation_ms: u64,
}

pub fn render<D: DrawTarget<Color = BinaryColor>>(display: &mut D, frame: RenderFrame<'_, '_>) {
    let RenderFrame {
        cards,
        battery,
        state,
        local_docs,
        interface_menu_details,
        animation_ms,
    } = frame;
    let _ = display.clear(BinaryColor::Off);
    draw_title_bar(display, battery, animation_ms);

    if let Some(notice) = state.notice() {
        draw_notice(display, notice);
        return;
    }

    if let UiMode::LoRaEditor { screen, profile } = state.mode {
        draw_lora_editor(display, screen, &profile);
        return;
    }

    if let UiMode::LimitsPage { page } = state.mode {
        let rows = build_limit_rows(state.storage_limits);
        draw_limits_page(display, page, &rows);
        return;
    }

    if state.mode == UiMode::Sleeping {
        draw_sleeping(display);
        return;
    }

    if let UiMode::ConfirmRadioSwap { confirm } = state.mode {
        draw_radio_confirm(display, confirm, state.access_point);
        return;
    }

    if let Some(selected_item) = state.global_menu_selected_item() {
        draw_global_menu(display, selected_item, state);
        return;
    }

    if let Some(selected_item) = state.interface_menu_selected_item() {
        if let Some(selected_card) = state.selected_card(cards) {
            draw_interface_menu(
                display,
                selected_card,
                selected_item,
                interface_menu_details.as_slice(),
            );
            return;
        }
    }

    let selected = state.selected_card_index(cards.len());
    let item_count = focus_item_count_with_footer(cards.len(), local_docs.is_some());
    let footer_focus = cards.len() + 1;
    let start = visible_start_for(item_count, state.selected_focus, state.visible_start);
    let mut top = CARD_TOP;
    let mut focus_index = start;
    if start == 0 {
        draw_global_row(display, GLOBAL_ROW_TOP, state.global_selected());
        top = FIRST_CARD_WITH_GLOBAL_TOP;
        focus_index = 1;
    }
    while top < HEIGHT && focus_index < item_count {
        if focus_index == footer_focus {
            if let Some(local_docs) = local_docs {
                draw_footer(
                    display,
                    top + 2,
                    local_docs,
                    state.selected_focus == footer_focus,
                );
            }
        } else {
            let card_index = focus_index - 1;
            let selected_card = selected == Some(card_index);
            if top + CARD_H <= HEIGHT {
                draw_card_with_selection(display, top, &cards[card_index], selected_card);
            } else {
                draw_card_peek(display, top, &cards[card_index], selected_card);
            }
        }
        top += CARD_SLOT_STEP;
        focus_index += 1;
    }
}

pub fn splash<D: DrawTarget<Color = BinaryColor>>(display: &mut D, status: &str) {
    let _ = display.clear(BinaryColor::Off);
    draw_title_bar(display, BatteryState::Unknown, 0);
    let style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let _ = Text::with_baseline(status, Point::new(2, CARD_TOP + 4), style, Baseline::Top)
        .draw(display);
}
