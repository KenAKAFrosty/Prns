use embedded_graphics::mono_font::iso_8859_1::{FONT_5X8, FONT_6X10};
use embedded_graphics::mono_font::{MonoFont, MonoTextStyle};
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};
use embedded_graphics::text::{Baseline, Text};

use personal_rns::interfaces::ConnectionState;

use crate::screen::{Card, CardKind, LocalDocsAccess};

use super::glyphs::{
    draw_arrow, draw_clock, draw_global_icon, draw_interface_icon, draw_link, draw_offline_icon,
    draw_person,
};
use super::layout::*;
use super::metrics::{draw_compact_number, fmt_activity_age, fmt_bytes, fmt_count};
use super::primitives::{fill, line, stroke};

fn name_font(kind: CardKind) -> &'static MonoFont<'static> {
    match kind {
        CardKind::Peer => &FONT_5X8,
        _ => &FONT_6X10,
    }
}

const fn name_char_w(kind: CardKind) -> i32 {
    match kind {
        CardKind::Peer => FONT_5X8_CHAR_W,
        _ => FONT_6X10_CHAR_W,
    }
}

pub const fn card_label_max_chars(kind: CardKind) -> usize {
    ((WIDTH - NAME_TEXT_X) / name_char_w(kind)) as usize
}
fn selected_name_backing_width(label: &str, char_w: i32) -> u32 {
    let label_right = NAME_TEXT_X + label.chars().count() as i32 * char_w + 1;
    let icon_right = NAME_ICON_X + NAME_ICON_W + 1;
    let content_right = label_right.max(icon_right).min(WIDTH - 1);
    (content_right - NAME_BACKING_X).max(0) as u32
}
fn global_row_backing_width() -> u32 {
    let label_right = GLOBAL_TEXT_X + GLOBAL_LABEL.chars().count() as i32 * FONT_6X10_CHAR_W + 2;
    (label_right - GLOBAL_BACKING_X).max(0) as u32
}

pub(in crate::screen) const fn connection_status_label(
    kind: CardKind,
    connection: ConnectionState,
) -> Option<&'static str> {
    match connection {
        ConnectionState::Initializing => Some("Initializing"),
        ConnectionState::Connected => None,
        ConnectionState::Degraded => Some("Degraded"),
        ConnectionState::Reconnecting => Some("Retrying"),
        ConnectionState::Failed => Some("Failed"),
        ConnectionState::Disconnected => match kind {
            CardKind::Wifi | CardKind::WifiStation | CardKind::WifiStationDisabled => {
                Some("Waiting")
            }
            CardKind::Ble => Some("No Peers"),
            CardKind::Usb => Some("Waiting"),
            CardKind::LoRa
            | CardKind::EspNow
            | CardKind::SharedInstance
            | CardKind::Tcp
            | CardKind::Peer => Some("Disconnected"),
        },
        ConnectionState::Disabled => Some("Off"),
        ConnectionState::Unknown => Some("Unknown"),
    }
}

pub(in crate::screen) fn draw_card_with_selection<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    top: i32,
    card: &Card,
    selected: bool,
) {
    let _ = Rectangle::new(Point::new(0, top), Size::new(WIDTH as u32, CARD_H as u32))
        .into_styled(stroke(BinaryColor::On))
        .draw(display);

    let name_color = if selected {
        BinaryColor::Off
    } else {
        BinaryColor::On
    };
    if selected {
        let _ = Rectangle::new(
            Point::new(NAME_BACKING_X, top + NAME_BACKING_Y),
            Size::new(
                selected_name_backing_width(&card.label, name_char_w(card.kind)),
                NAME_BACKING_H,
            ),
        )
        .into_styled(fill(BinaryColor::On))
        .draw(display);
    }

    let label_style = MonoTextStyle::new(name_font(card.kind), name_color);
    let num_style = MonoTextStyle::new(&FONT_5X8, BinaryColor::On);

    if matches!(
        card.connection,
        ConnectionState::Failed | ConnectionState::Unknown
    ) {
        draw_offline_icon(display, NAME_ICON_X, top + NAME_LINE_Y + 1, name_color);
    } else {
        draw_interface_icon(
            display,
            NAME_ICON_X,
            top + NAME_LINE_Y,
            card.kind,
            name_color,
        );
    }
    let _ = Text::with_baseline(
        &card.label,
        Point::new(NAME_TEXT_X, top + NAME_LINE_Y),
        label_style,
        Baseline::Top,
    )
    .draw(display);

    let tx_y = top + 13;
    let rx_y = top + 22;
    let live_y = top + 31;
    let whole_card_word = connection_status_label(card.kind, card.connection);
    if let Some(word) = whole_card_word {
        let _ = Text::with_baseline(word, Point::new(2, top + 20), num_style, Baseline::Top)
            .draw(display);
        return;
    }

    draw_arrow(display, 2, tx_y + 1, true);
    let tx_bytes = fmt_bytes(card.tx_bytes);
    draw_compact_number(
        display,
        tx_bytes.as_str(),
        Point::new(8, tx_y),
        BinaryColor::On,
    );
    draw_arrow(display, 2, rx_y, false);
    let rx_bytes = fmt_bytes(card.rx_bytes);
    draw_compact_number(
        display,
        rx_bytes.as_str(),
        Point::new(8, rx_y),
        BinaryColor::On,
    );

    draw_person(display, STAT_ICON_X, tx_y + 1);
    let destinations = fmt_count(card.destinations);
    draw_compact_number(
        display,
        destinations.as_str(),
        Point::new(STAT_TEXT_X, tx_y),
        BinaryColor::On,
    );
    draw_link(display, STAT_ICON_X, rx_y + 1);
    let links = fmt_count(card.links);
    draw_compact_number(
        display,
        links.as_str(),
        Point::new(STAT_TEXT_X, rx_y),
        BinaryColor::On,
    );

    draw_clock(display, ACTIVITY_ICON_X, live_y + 1);
    let age = fmt_activity_age(card.last_activity_secs);
    draw_compact_number(
        display,
        age.as_str(),
        Point::new(ACTIVITY_TEXT_X, live_y),
        BinaryColor::On,
    );
}

pub(super) fn draw_card_peek<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    top: i32,
    card: &Card,
    selected: bool,
) {
    draw_card_peek_to(display, top, card, selected, HEIGHT);
}

fn draw_card_peek_to<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    top: i32,
    card: &Card,
    selected: bool,
    bottom: i32,
) {
    let bottom = bottom.clamp(top + 1, HEIGHT);
    line(display, Point::new(0, top), Point::new(WIDTH - 1, top));
    line(display, Point::new(0, top), Point::new(0, bottom - 1));
    line(
        display,
        Point::new(WIDTH - 1, top),
        Point::new(WIDTH - 1, bottom - 1),
    );

    if top + NAME_LINE_Y + 9 >= bottom {
        return;
    }

    let name_color = if selected {
        let _ = Rectangle::new(
            Point::new(NAME_BACKING_X, top + NAME_BACKING_Y),
            Size::new(
                selected_name_backing_width(&card.label, name_char_w(card.kind)),
                NAME_BACKING_H,
            ),
        )
        .into_styled(fill(BinaryColor::On))
        .draw(display);
        BinaryColor::Off
    } else {
        BinaryColor::On
    };
    let label_style = MonoTextStyle::new(name_font(card.kind), name_color);
    if matches!(
        card.connection,
        ConnectionState::Failed | ConnectionState::Unknown
    ) {
        draw_offline_icon(display, NAME_ICON_X, top + NAME_LINE_Y + 1, name_color);
    } else {
        draw_interface_icon(
            display,
            NAME_ICON_X,
            top + NAME_LINE_Y,
            card.kind,
            name_color,
        );
    }
    let _ = Text::with_baseline(
        &card.label,
        Point::new(NAME_TEXT_X, top + NAME_LINE_Y),
        label_style,
        Baseline::Top,
    )
    .draw(display);
}

pub(super) fn draw_footer<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    top: i32,
    local_docs: &LocalDocsAccess<'_>,
    selected: bool,
) {
    draw_footer_line(
        display,
        "Wi-Fi AP",
        top,
        &FONT_6X10,
        FONT_6X10_CHAR_W,
        selected,
    );
    draw_footer_line(
        display,
        local_docs.wifi_ssid,
        top + FOOTER_SECOND_LINE_OFFSET,
        &FONT_5X8,
        FONT_5X8_CHAR_W,
        selected,
    );
    draw_footer_line(
        display,
        local_docs.docs_host,
        top + FOOTER_FOURTH_LINE_OFFSET,
        &FONT_5X8,
        FONT_5X8_CHAR_W,
        selected,
    );
}

fn draw_footer_line<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    text: &str,
    y: i32,
    font: &'static MonoFont<'static>,
    char_w: i32,
    selected: bool,
) {
    if !(CARD_TOP..HEIGHT).contains(&y) {
        return;
    }
    let style = MonoTextStyle::new(
        font,
        if selected {
            BinaryColor::Off
        } else {
            BinaryColor::On
        },
    );
    let width = text.chars().count() as i32 * char_w;
    let x = ((WIDTH - width) / 2).max(0);
    if selected {
        let _ = Rectangle::new(
            Point::new(x.saturating_sub(2), y.saturating_sub(1)),
            Size::new(
                (width + 4).min(WIDTH) as u32,
                font.character_size.height + 2,
            ),
        )
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
        .draw(display);
    }
    let _ = Text::with_baseline(text, Point::new(x, y), style, Baseline::Top).draw(display);
}

pub(super) fn draw_global_row<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    top: i32,
    selected: bool,
) {
    let row_color = if selected {
        let _ = Rectangle::new(
            Point::new(GLOBAL_BACKING_X, top + GLOBAL_BACKING_Y),
            Size::new(global_row_backing_width(), GLOBAL_BACKING_H),
        )
        .into_styled(fill(BinaryColor::On))
        .draw(display);
        BinaryColor::Off
    } else {
        BinaryColor::On
    };
    let label_style = MonoTextStyle::new(&FONT_6X10, row_color);
    draw_global_icon(display, GLOBAL_ICON_X, top + NAME_LINE_Y, row_color);
    let _ = Text::with_baseline(
        GLOBAL_LABEL,
        Point::new(GLOBAL_TEXT_X, top + NAME_LINE_Y),
        label_style,
        Baseline::Top,
    )
    .draw(display);
}
