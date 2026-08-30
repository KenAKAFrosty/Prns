pub(in crate::screen) mod cards;
pub(in crate::screen) mod glyphs;
mod gnss;
pub(in crate::screen) mod layout;
pub(in crate::screen) mod menus;
pub(in crate::screen) mod metrics;
mod primitives;

use embedded_graphics::mono_font::iso_8859_1::FONT_6X10;
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::text::{Baseline, Text};

use crate::PowerSnapshot;

use super::face_64x128::{RenderInput, SplashContent};
use super::limits::build_limit_rows;
use super::state::{focus_item_count, visible_start_for, UiMode};
use cards::{draw_card_peek, draw_card_with_selection, draw_footer, draw_global_row};
use glyphs::draw_title_bar;
use gnss::draw_gnss_panel;
use layout::*;
use menus::ble_group::draw_ble_group_editor;
use menus::lora::draw_lora_editor;
use menus::{
    draw_global_menu, draw_interface_menu, draw_limits_page, draw_notice, draw_radio_confirm,
    draw_sleeping,
};

pub(super) fn draw<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    input: RenderInput<'_, '_>,
) {
    let RenderInput {
        content,
        battery,
        gnss,
        state,
        interface_menu_details,
    } = input;
    let cards = content.cards;
    let local_docs = content.local_docs;
    let _ = display.clear(BinaryColor::Off);
    draw_title_bar(display, battery);

    if let Some(notice) = state.notice() {
        draw_notice(display, notice);
        return;
    }

    if let UiMode::LoRaEditor { screen, profile } = state.mode {
        draw_lora_editor(display, screen, &profile);
        return;
    }

    if let UiMode::BleGroupEditor { screen, name } = state.mode {
        draw_ble_group_editor(display, screen, name);
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
                state.shared_instance_config_export,
                state.ble_group_editor,
                interface_menu_details,
            );
            return;
        }
    }

    if let Some(gnss) = gnss {
        draw_global_row(display, GLOBAL_ROW_TOP, state.global_selected());
        draw_gnss_panel(display, gnss);
        let selected = state.selected_card_index(cards.len());
        if let Some(primary) = selected.or_else(|| (!cards.is_empty()).then_some(0)) {
            draw_card_with_selection(
                display,
                FIRST_CARD_WITH_GNSS_TOP,
                &cards[primary],
                selected == Some(primary),
            );
            if cards.len() > 1 {
                let secondary = (primary + 1) % cards.len();
                draw_card_peek(
                    display,
                    FIRST_CARD_WITH_GNSS_TOP + CARD_SLOT_STEP,
                    &cards[secondary],
                    false,
                );
            }
        }
        return;
    }

    let selected = state.selected_card_index(cards.len());
    let item_count = focus_item_count(content);
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

pub(super) fn draw_splash<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    content: SplashContent,
) {
    let _ = display.clear(BinaryColor::Off);
    draw_title_bar(display, PowerSnapshot::UNKNOWN);
    let style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let mut y = CARD_TOP + 4;
    for line in content.lines() {
        let _ = Text::with_baseline(line, Point::new(SPLASH_TEXT_X, y), style, Baseline::Top)
            .draw(display);
        y += SPLASH_LINE_STEP;
    }
}
