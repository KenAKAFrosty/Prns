pub(in crate::screen) mod lora;

use core::fmt::Write as _;

use embedded_graphics::mono_font::iso_8859_1::{FONT_4X6, FONT_5X8, FONT_6X10};
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;
use embedded_graphics::text::{Baseline, Text};
use heapless::String as HString;
use personal_rns::interfaces::ConnectionState;

use crate::interface_mode::interface_mode_menu_label;
use crate::screen::limits::{limit_page_count, LimitRow, LimitValue, LIMITS_PER_PAGE};
use crate::screen::model::{
    Card, CardKind, InterfaceMenuDetailKind, InterfaceMenuDetails,
};
use crate::screen::state::interface_detail::{
    interface_detail_page, interface_detail_status_line_count, InterfaceDetailFocus,
    InterfaceDetailPage, DETAIL_CONTROL_Y, DETAIL_DIVIDER_Y, DETAIL_OPTIONS_Y, DETAIL_STATUS_TOP,
};
use crate::screen::state::interface_mode::{
    interface_mode_editor_row_count, interface_mode_editor_row_label,
};
use crate::screen::state::{
    interface_menu_items, AccessPointState, SharedInstanceConfigExport, UiNotice, UiState,
    POWER_MENU_ITEM, STATION_UPLINK_MENU_ITEM,
};

use super::glyphs::{draw_global_icon, draw_interface_icon, draw_menu_cursor};
use super::layout::*;
use super::metrics::{fmt_bytes, fmt_count, fmt_rate_bytes_per_sec};
use super::primitives::{fill, line};

fn menu_item_backing_width(label: &str) -> u32 {
    (menu_item_text_right(label) + 1 - MENU_BACKING_X).max(0) as u32
}

pub(in crate::screen) const fn station_uplink_action_label(kind: CardKind) -> Option<&'static str> {
    match kind {
        CardKind::WifiStation => Some("Disconnect AP"),
        CardKind::WifiStationDisabled => Some("Reconnect AP"),
        CardKind::Wifi
        | CardKind::Usb
        | CardKind::Ble
        | CardKind::LoRa
        | CardKind::EspNow
        | CardKind::SharedInstance
        | CardKind::Tcp
        | CardKind::Peer => None,
    }
}

pub(in crate::screen) fn menu_item_char_width(label: &str) -> i32 {
    if MENU_TEXT_X + label.chars().count() as i32 * FONT_5X8_CHAR_W > WIDTH {
        FONT_4X6_CHAR_W
    } else {
        FONT_5X8_CHAR_W
    }
}

pub(in crate::screen) fn menu_item_text_right(label: &str) -> i32 {
    MENU_TEXT_X + label.chars().count() as i32 * menu_item_char_width(label)
}

fn draw_menu_item<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    y: i32,
    label: &str,
    selected: bool,
) {
    let char_width = menu_item_char_width(label);
    let font = if char_width == FONT_4X6_CHAR_W {
        &FONT_4X6
    } else {
        &FONT_5X8
    };
    let color = if selected {
        let _ = Rectangle::new(
            Point::new(MENU_BACKING_X, y - 1),
            Size::new(menu_item_backing_width(label), MENU_BACKING_H),
        )
        .into_styled(fill(BinaryColor::On))
        .draw(display);
        BinaryColor::Off
    } else {
        BinaryColor::On
    };
    let style = MonoTextStyle::new(font, color);
    draw_menu_cursor(display, MENU_MARK_X, y, color);
    let _ =
        Text::with_baseline(label, Point::new(MENU_TEXT_X, y), style, Baseline::Top).draw(display);
}

pub(super) fn draw_global_menu<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    selected_item: usize,
    state: &UiState,
) {
    draw_global_icon(display, NAME_ICON_X, MENU_HEADER_Y, BinaryColor::On);
    let header_style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let _ = Text::with_baseline(
        GLOBAL_LABEL,
        Point::new(NAME_TEXT_X, MENU_HEADER_Y),
        header_style,
        Baseline::Top,
    )
    .draw(display);

    let subtitle_style = MonoTextStyle::new(&FONT_5X8, BinaryColor::On);
    let _ = Text::with_baseline(
        "Global",
        Point::new(NAME_TEXT_X, MENU_SUBTITLE_Y),
        subtitle_style,
        Baseline::Top,
    )
    .draw(display);
    line(
        display,
        Point::new(0, MENU_DIVIDER_Y),
        Point::new(WIDTH - 1, MENU_DIVIDER_Y),
    );

    const VISIBLE_ITEMS: usize = 6;
    let item_count = state.global_menu_items().count();
    let visible_start = selected_item
        .saturating_sub(VISIBLE_ITEMS - 1)
        .min(item_count.saturating_sub(VISIBLE_ITEMS));
    for (visible_index, (index, item)) in state
        .global_menu_items()
        .enumerate()
        .skip(visible_start)
        .take(VISIBLE_ITEMS)
        .enumerate()
    {
        let label = state.global_menu_item_label(item);
        draw_menu_item(
            display,
            MENU_ITEM_TOP + visible_index as i32 * GLOBAL_MENU_ITEM_STEP,
            label,
            index == selected_item.min(item_count - 1),
        );
    }
}

pub(super) fn draw_radio_confirm<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    confirm: bool,
    access_point: AccessPointState,
) {
    let header_style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let _ = Text::with_baseline(
        "Radio",
        Point::new(NAME_TEXT_X, MENU_HEADER_Y),
        header_style,
        Baseline::Top,
    )
    .draw(display);
    line(
        display,
        Point::new(0, MENU_DIVIDER_Y),
        Point::new(WIDTH - 1, MENU_DIVIDER_Y),
    );
    let body = MonoTextStyle::new(&FONT_5X8, BinaryColor::On);
    let prompt = match access_point {
        AccessPointState::Active => "To BLE?",
        AccessPointState::Inactive => "To AP?",
        AccessPointState::Unsupported => "",
    };
    let _ = Text::with_baseline(prompt, Point::new(2, MENU_ITEM_TOP), body, Baseline::Top)
        .draw(display);
    let _ = Text::with_baseline(
        "BLE off,",
        Point::new(2, MENU_ITEM_TOP + 9),
        body,
        Baseline::Top,
    )
    .draw(display);
    let _ = Text::with_baseline(
        "restarts",
        Point::new(2, MENU_ITEM_TOP + 18),
        body,
        Baseline::Top,
    )
    .draw(display);
    draw_menu_item(display, MENU_ITEM_TOP + 31, "No", !confirm);
    draw_menu_item(display, MENU_ITEM_TOP + 44, "Yes", confirm);
}

fn fmt_limit_value(value: LimitValue) -> HString<12> {
    let mut s = HString::new();
    match value {
        LimitValue::Count(value) => {
            let _ = write!(s, "{}", fmt_count(value));
        }
        LimitValue::Bytes(value) => {
            let _ = write!(s, "{}", fmt_bytes(value));
        }
        LimitValue::Range(low, high) => {
            let _ = write!(s, "{low}-{high}");
        }
        LimitValue::RateBytesPerSec(value) => {
            let rate = fmt_rate_bytes_per_sec(value.min(u64::from(u32::MAX)) as u32);
            let _ = write!(s, "{rate}");
        }
        LimitValue::Text(value) => {
            let _ = write!(s, "{value}");
        }
    }
    s
}

pub(in crate::screen) fn limits_row_text(row: LimitRow) -> HString<16> {
    let mut s = HString::new();
    let _ = write!(s, "{} {}", row.label, fmt_limit_value(row.value));
    s
}

pub(in crate::screen) fn limits_row_drawable<'a>(
    text: &'a str,
    y: i32,
) -> Text<'a, MonoTextStyle<'static, BinaryColor>> {
    let style = MonoTextStyle::new(&FONT_5X8, BinaryColor::On);
    Text::with_baseline(text, Point::new(LIMITS_TEXT_X, y), style, Baseline::Top)
}

pub(super) fn draw_limits_page<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    page: usize,
    rows: &[LimitRow],
) {
    let page_count = limit_page_count(rows);
    let page = page.min(page_count - 1);
    let mut header: HString<16> = HString::new();
    let _ = write!(header, "Limits {}/{}", page + 1, page_count);
    let header_style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let _ = Text::with_baseline(
        &header,
        Point::new(2, CARD_TOP + 2),
        header_style,
        Baseline::Top,
    )
    .draw(display);
    line(
        display,
        Point::new(0, MENU_DIVIDER_Y),
        Point::new(WIDTH - 1, MENU_DIVIDER_Y),
    );

    let start = page * LIMITS_PER_PAGE;
    for (offset, row) in rows.iter().skip(start).take(LIMITS_PER_PAGE).enumerate() {
        let line_buf = limits_row_text(*row);
        let _ = limits_row_drawable(&line_buf, CARD_TOP + 29 + offset as i32 * 11).draw(display);
    }
}

pub(super) fn draw_sleeping<D: DrawTarget<Color = BinaryColor>>(display: &mut D) {
    let style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let _ = Text::with_baseline(
        "Sleeping",
        Point::new(7, CARD_TOP + 20),
        style,
        Baseline::Top,
    )
    .draw(display);
    let hint = MonoTextStyle::new(&FONT_5X8, BinaryColor::On);
    let _ = Text::with_baseline(
        "ifaces off",
        Point::new(7, CARD_TOP + 36),
        hint,
        Baseline::Top,
    )
    .draw(display);
    let _ = Text::with_baseline(
        "press wake",
        Point::new(7, CARD_TOP + 48),
        hint,
        Baseline::Top,
    )
    .draw(display);
}

pub(super) fn draw_notice<D: DrawTarget<Color = BinaryColor>>(display: &mut D, notice: UiNotice) {
    let lines = notice.lines();
    let line_count = lines.as_slice().len() as i32;
    let multiline = line_count > 1;
    let line_step = if multiline { 9 } else { 11 };
    let char_width = if multiline {
        FONT_4X6_CHAR_W
    } else {
        FONT_5X8_CHAR_W
    };
    let first_y = CARD_TOP + 27 - ((line_count - 1) * line_step) / 2;
    let style = MonoTextStyle::new(
        if multiline { &FONT_4X6 } else { &FONT_5X8 },
        BinaryColor::On,
    );
    for (index, label) in lines.as_slice().iter().enumerate() {
        let char_count = label.chars().count() as i32;
        let x = ((WIDTH - char_count * char_width) / 2).max(0);
        let _ = Text::with_baseline(
            label,
            Point::new(x, first_y + index as i32 * line_step),
            style,
            Baseline::Top,
        )
        .draw(display);
    }
}

pub(in crate::screen) fn draw_interface_detail<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    card: &Card,
    focus: InterfaceDetailFocus,
    status_page: usize,
    details: &InterfaceMenuDetails,
) {
    draw_interface_icon(
        display,
        NAME_ICON_X,
        MENU_HEADER_Y,
        card.kind,
        BinaryColor::On,
    );
    let header_style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let _ = Text::with_baseline(
        &card.label,
        Point::new(NAME_TEXT_X, MENU_HEADER_Y),
        header_style,
        Baseline::Top,
    )
    .draw(display);
    line(
        display,
        Point::new(0, DETAIL_DIVIDER_Y),
        Point::new(WIDTH - 1, DETAIL_DIVIDER_Y),
    );

    draw_menu_item(
        display,
        DETAIL_OPTIONS_Y,
        "Options",
        focus == InterfaceDetailFocus::Options,
    );

    let status_lines = interface_detail_status_line_count(
        details.as_slice().len(),
        card.connection == ConnectionState::Failed && card.failure_reason.is_some(),
    );
    let page = interface_detail_page(status_lines, status_page);
    draw_interface_detail_status_page(display, card, details, page);

    if page.shows_next {
        draw_menu_item(
            display,
            DETAIL_CONTROL_Y,
            "Next",
            focus == InterfaceDetailFocus::Next,
        );
    }
    if page.shows_back {
        let back_y = DETAIL_STATUS_TOP + page.status_count as i32 * MENU_DETAIL_STEP;
        draw_menu_item(
            display,
            back_y,
            "Back",
            focus == InterfaceDetailFocus::Back,
        );
    }
}

fn draw_interface_detail_status_page<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    card: &Card,
    details: &InterfaceMenuDetails,
    page: InterfaceDetailPage,
) {
    let style = MonoTextStyle::new(&FONT_4X6, BinaryColor::On);
    let mut drawn = 0usize;
    let mut y = DETAIL_STATUS_TOP;

    let emit = |display: &mut D,
                y: &mut i32,
                drawn: &mut usize,
                page: InterfaceDetailPage,
                x: i32,
                text: &str| {
        if *drawn < page.status_start {
            *drawn = drawn.saturating_add(1);
            return;
        }
        if *drawn >= page.status_start.saturating_add(page.status_count) {
            return;
        }
        if page.shows_next && *y >= DETAIL_CONTROL_Y {
            return;
        }
        let _ = Text::with_baseline(text, Point::new(x, *y), style, Baseline::Top).draw(display);
        *y += MENU_DETAIL_STEP;
        *drawn = drawn.saturating_add(1);
    };

    {
        let mut mode_text = heapless::String::<15>::new();
        let _ = write!(mode_text, "Mode {}", interface_mode_menu_label(card.mode()));
        emit(
            display,
            &mut y,
            &mut drawn,
            page,
            MENU_REASON_X,
            mode_text.as_str(),
        );
    }

    for row in details.as_slice() {
        let x = match row.kind() {
            InterfaceMenuDetailKind::Info => MENU_REASON_X,
            InterfaceMenuDetailKind::Peer => MENU_REASON_X + 4,
        };
        emit(display, &mut y, &mut drawn, page, x, row.text());
    }

    if card.connection == ConnectionState::Failed {
        if let Some(reason) = card.failure_reason {
            emit(display, &mut y, &mut drawn, page, MENU_REASON_X, "Fail:");
            let mut line: heapless::String<15> = heapless::String::new();
            for ch in reason.chars().take(15) {
                let _ = line.push(ch);
            }
            emit(display, &mut y, &mut drawn, page, MENU_REASON_X, &line);
        }
    }
}

pub(in crate::screen) fn draw_interface_options<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    card: &Card,
    selected_item: usize,
    shared_instance_config_export: SharedInstanceConfigExport,
) {
    draw_interface_icon(
        display,
        NAME_ICON_X,
        MENU_HEADER_Y,
        card.kind,
        BinaryColor::On,
    );
    let header_style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let _ = Text::with_baseline(
        &card.label,
        Point::new(NAME_TEXT_X, MENU_HEADER_Y),
        header_style,
        Baseline::Top,
    )
    .draw(display);

    let subtitle_style = MonoTextStyle::new(&FONT_5X8, BinaryColor::On);
    let _ = Text::with_baseline(
        "Options",
        Point::new(NAME_TEXT_X, MENU_SUBTITLE_Y),
        subtitle_style,
        Baseline::Top,
    )
    .draw(display);
    line(
        display,
        Point::new(0, MENU_DIVIDER_Y),
        Point::new(WIDTH - 1, MENU_DIVIDER_Y),
    );

    let items = interface_menu_items(card.kind, shared_instance_config_export);
    for (index, item) in items.iter().enumerate() {
        let label = if index == POWER_MENU_ITEM {
            if card.connection == ConnectionState::Disabled {
                "Turn On"
            } else {
                "Turn Off"
            }
        } else if index == STATION_UPLINK_MENU_ITEM {
            station_uplink_action_label(card.kind).unwrap_or(item)
        } else {
            item
        };
        draw_menu_item(
            display,
            MENU_ITEM_TOP + index as i32 * MENU_ITEM_STEP,
            label,
            index == selected_item.min(items.len() - 1),
        );
    }
}

pub(super) fn draw_interface_mode_editor<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    cursor: usize,
) {
    let header_style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let _ = Text::with_baseline(
        "Mode",
        Point::new(NAME_TEXT_X, MENU_HEADER_Y),
        header_style,
        Baseline::Top,
    )
    .draw(display);
    line(
        display,
        Point::new(0, MENU_DIVIDER_Y),
        Point::new(WIDTH - 1, MENU_DIVIDER_Y),
    );

    const VISIBLE_ITEMS: usize = 6;
    let item_count = interface_mode_editor_row_count();
    let cursor = cursor.min(item_count.saturating_sub(1));
    let visible_start = cursor
        .saturating_sub(VISIBLE_ITEMS - 1)
        .min(item_count.saturating_sub(VISIBLE_ITEMS));
    for visible_index in 0..VISIBLE_ITEMS.min(item_count.saturating_sub(visible_start)) {
        let index = visible_start + visible_index;
        let Some(label) = interface_mode_editor_row_label(index) else {
            break;
        };
        draw_menu_item(
            display,
            MENU_ITEM_TOP + visible_index as i32 * MENU_ITEM_STEP,
            label,
            index == cursor,
        );
    }
}
