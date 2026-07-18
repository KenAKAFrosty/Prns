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

use super::*;
use cards::{draw_card, draw_card_peek, draw_card_with_selection, draw_footer, draw_global_row};
use glyphs::draw_title_bar;
use layout::*;
use menus::lora::draw_lora_editor;
use menus::{
    draw_global_menu, draw_interface_menu, draw_limits_page, draw_notice, draw_radio_confirm,
    draw_sleeping,
};

/// Render the full screen: title bar + a card per interface (up to what fits). Clears first; the caller flushes.
pub fn draw<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    cards: &[Card],
    battery: BatteryState,
) {
    draw_at(display, cards, battery, 0);
}

pub fn draw_at<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    cards: &[Card],
    battery: BatteryState,
    animation_ms: u64,
) {
    let _ = display.clear(BinaryColor::Off);
    draw_title_bar(display, battery, animation_ms);
    draw_global_row(display, GLOBAL_ROW_TOP, false);
    for (i, card) in cards.iter().enumerate() {
        let top = FIRST_CARD_WITH_GLOBAL_TOP + i as i32 * CARD_SLOT_STEP;
        if top >= HEIGHT {
            break;
        }
        if top + CARD_H <= HEIGHT {
            draw_card(display, top, card);
        } else {
            draw_card_peek(display, top, card, card.selected);
        }
    }
}

/// Render using [`UiState`] for selection and pagination: the real-interaction path. Plain [`draw`] remains for static/manual selected-card rendering.
pub fn draw_with_state<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    cards: &[Card],
    battery: BatteryState,
    state: &UiState,
) {
    draw_with_state_at(display, cards, battery, state, 0);
}

pub fn draw_with_state_at<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    cards: &[Card],
    battery: BatteryState,
    state: &UiState,
    animation_ms: u64,
) {
    draw_with_state_footer_at(display, cards, battery, state, None, animation_ms);
}

pub fn draw_with_state_footer_at<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    cards: &[Card],
    battery: BatteryState,
    state: &UiState,
    footer: Option<UiFooter<'_>>,
    animation_ms: u64,
) {
    draw_with_state_footer_details_at(display, cards, battery, state, footer, &[], animation_ms);
}

pub fn draw_with_state_footer_details_at<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    cards: &[Card],
    battery: BatteryState,
    state: &UiState,
    footer: Option<UiFooter<'_>>,
    interface_menu_details: &[InterfaceMenuDetailRow],
    animation_ms: u64,
) {
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
        draw_radio_confirm(display, confirm, state.ap_active);
        return;
    }

    if let Some(selected_item) = state.global_menu_selected_item() {
        draw_global_menu(
            display,
            selected_item,
            state.display_power_capable,
            state.ap_capable,
            state.ap_active,
        );
        return;
    }

    if let Some(selected_item) = state.interface_menu_selected_item() {
        if let Some(selected_card) = state.selected_card(cards.len()) {
            draw_interface_menu(
                display,
                &cards[selected_card],
                selected_item,
                interface_menu_details,
            );
            return;
        }
    }

    let selected = state.selected_card(cards.len());
    let item_count = focus_item_count_with_footer(cards.len(), footer.is_some());
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
            if let Some(footer) = footer {
                draw_footer(
                    display,
                    top + 2,
                    footer,
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

/// A boot/connecting splash: title bar + a centered status line.
pub fn splash<D: DrawTarget<Color = BinaryColor>>(display: &mut D, status: &str) {
    let _ = display.clear(BinaryColor::Off);
    draw_title_bar(display, BatteryState::Unknown, 0);
    let style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let _ = Text::with_baseline(status, Point::new(2, CARD_TOP + 4), style, Baseline::Top)
        .draw(display);
}
