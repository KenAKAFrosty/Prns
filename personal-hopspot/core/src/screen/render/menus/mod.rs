pub(in crate::screen) mod lora;

use core::fmt::Write as _;

use embedded_graphics::mono_font::ascii::{FONT_4X6, FONT_5X8, FONT_6X10};
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;
use embedded_graphics::text::{Baseline, Text};
use heapless::String as HString;

use crate::screen::{
    interface_menu_items, limit_page_count, Card, InterfaceMenuDetailKind, InterfaceMenuDetailRow,
    LimitRow, LimitValue, Liveness, UiNotice, GLOBAL_MENU_ITEMS, GLOBAL_MENU_ITEMS_AP,
    GLOBAL_MENU_ITEMS_AP_DISPLAY, GLOBAL_MENU_ITEMS_DISPLAY, LIMITS_PER_PAGE, POWER_MENU_ITEM,
    RADIO_MENU_ITEM, RADIO_MENU_ITEM_NO_DISPLAY,
};

use super::glyphs::{draw_global_icon, draw_interface_icon, draw_menu_cursor};
use super::layout::*;
use super::metrics::{fmt_bytes, fmt_rate_bytes_per_sec};
use super::primitives::{fill, line};

fn menu_item_backing_width(label: &str) -> u32 {
    let text_right = MENU_TEXT_X + label.chars().count() as i32 * FONT_5X8_CHAR_W + 1;
    (text_right - MENU_BACKING_X).max(0) as u32
}
fn draw_menu_item<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    y: i32,
    label: &str,
    selected: bool,
) {
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
    let style = MonoTextStyle::new(&FONT_5X8, color);
    draw_menu_cursor(display, MENU_MARK_X, y, color);
    let _ =
        Text::with_baseline(label, Point::new(MENU_TEXT_X, y), style, Baseline::Top).draw(display);
}

fn draw_failure_reason<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    mut y: i32,
    reason: &str,
) {
    const LINE_H: i32 = 7;
    const MAX_CHARS: usize = ((WIDTH - MENU_REASON_X) / FONT_4X6_CHAR_W) as usize;
    let style = MonoTextStyle::new(&FONT_4X6, BinaryColor::On);
    let draw_line = |display: &mut D, y: &mut i32, line: &str| {
        if *y > HEIGHT - LINE_H {
            return false;
        }
        let _ = Text::with_baseline(line, Point::new(MENU_REASON_X, *y), style, Baseline::Top)
            .draw(display);
        *y += LINE_H;
        true
    };

    if !draw_line(display, &mut y, "Fail:") {
        return;
    }

    let mut line: heapless::String<24> = heapless::String::new();
    for word in reason.split_whitespace() {
        let sep = usize::from(!line.is_empty());
        let would_len = line.chars().count() + sep + word.chars().count();
        if would_len > MAX_CHARS && !line.is_empty() {
            if !draw_line(display, &mut y, &line) {
                return;
            }
            line.clear();
        }
        if !line.is_empty() {
            let _ = line.push(' ');
        }
        for ch in word.chars().take(MAX_CHARS) {
            let _ = line.push(ch);
        }
    }
    if !line.is_empty() {
        let _ = draw_line(display, &mut y, &line);
    }
}

fn draw_interface_menu_details<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    mut y: i32,
    rows: &[InterfaceMenuDetailRow],
) -> i32 {
    let style = MonoTextStyle::new(&FONT_4X6, BinaryColor::On);
    for row in rows {
        if y > HEIGHT - MENU_DETAIL_STEP {
            break;
        }
        let x = match row.kind {
            InterfaceMenuDetailKind::Info => MENU_REASON_X,
            InterfaceMenuDetailKind::Peer => MENU_REASON_X + 4,
        };
        let _ =
            Text::with_baseline(row.text(), Point::new(x, y), style, Baseline::Top).draw(display);
        y += MENU_DETAIL_STEP;
    }
    y
}

pub(super) fn draw_global_menu<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    selected_item: usize,
    display_power_capable: bool,
    ap_capable: bool,
    ap_active: bool,
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

    let items = match (display_power_capable, ap_capable) {
        (true, true) => GLOBAL_MENU_ITEMS_AP_DISPLAY,
        (true, false) => GLOBAL_MENU_ITEMS_DISPLAY,
        (false, true) => GLOBAL_MENU_ITEMS_AP,
        (false, false) => GLOBAL_MENU_ITEMS,
    };
    let radio_menu_item = if display_power_capable {
        RADIO_MENU_ITEM
    } else {
        RADIO_MENU_ITEM_NO_DISPLAY
    };
    for (index, item) in items.iter().enumerate() {
        let label = if index == radio_menu_item && ap_capable {
            if ap_active {
                "BLE Mode"
            } else {
                "AP Mode"
            }
        } else {
            *item
        };
        draw_menu_item(
            display,
            MENU_ITEM_TOP + index as i32 * GLOBAL_MENU_ITEM_STEP,
            label,
            index == selected_item.min(items.len() - 1),
        );
    }
}

pub(super) fn draw_radio_confirm<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    confirm: bool,
    ap_active: bool,
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
    let prompt = if ap_active { "To BLE?" } else { "To AP?" };
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
            let _ = write!(s, "{value}");
        }
        LimitValue::Bytes(value) => {
            let _ = write!(s, "{}", fmt_bytes(value));
        }
        LimitValue::Range(low, high) => {
            let _ = write!(s, "{low}-{high}");
        }
        LimitValue::RateBytesPerSec(value) => {
            let rate = fmt_rate_bytes_per_sec(value.min(u64::from(u32::MAX)) as u32);
            let _ = write!(s, "{rate}/s");
        }
        LimitValue::Text(value) => {
            let _ = write!(s, "{value}");
        }
    }
    s
}

fn draw_limits_text<D: DrawTarget<Color = BinaryColor>>(display: &mut D, y: i32, text: &str) {
    let style = MonoTextStyle::new(&FONT_5X8, BinaryColor::On);
    let _ = Text::with_baseline(text, Point::new(2, y), style, Baseline::Top).draw(display);
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
        let value = fmt_limit_value(row.value);
        let mut line_buf: HString<16> = HString::new();
        let _ = write!(line_buf, "{} {value}", row.label);
        draw_limits_text(display, CARD_TOP + 29 + offset as i32 * 11, &line_buf);
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
    let label = notice.label();
    let char_count = label.chars().count() as i32;
    let x = ((WIDTH - char_count * FONT_5X8_CHAR_W) / 2).max(0);
    let style = MonoTextStyle::new(&FONT_5X8, BinaryColor::On);
    let _ = Text::with_baseline(label, Point::new(x, CARD_TOP + 27), style, Baseline::Top)
        .draw(display);
}

pub(in crate::screen) fn draw_interface_menu<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    card: &Card,
    selected_item: usize,
    details: &[InterfaceMenuDetailRow],
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
        "Menu",
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

    let items = interface_menu_items(card.kind);
    for (index, item) in items.iter().enumerate() {
        let label = if index == POWER_MENU_ITEM {
            if card.liveness == Liveness::Disabled {
                "Turn On"
            } else {
                "Turn Off"
            }
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
    let mut detail_y = MENU_ITEM_TOP + items.len() as i32 * MENU_ITEM_STEP + 1;
    if !details.is_empty() {
        detail_y = draw_interface_menu_details(display, detail_y, details);
    }
    if card.liveness.is_failed() {
        if let Some(reason) = card.failure_reason {
            draw_failure_reason(display, detail_y - 1, reason);
        }
    }
}
