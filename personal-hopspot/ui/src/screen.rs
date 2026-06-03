//! The "Personal Hopspot" status screen — portrait (64x128), drawn against any
//! `embedded_graphics` `DrawTarget<Color = BinaryColor>`, so the same pixels land
//! on the S3's SSD1306 OLED and on the Linux debug window's simulator display.
//!
//! A two-line inverted title bar (`Personal` over a **bold** `Hopspot`) above a
//! vertical stack of interface cards. Each card is a name line (icon + label)
//! with its data underneath: stacked up/down Reticulum traffic (3 significant
//! figures, rolling B->K->M->G), a link glyph/count, and a person glyph with the
//! count of destinations the routing table tracks via that interface. An
//! interface that's down shows a slashed icon and its traffic line is replaced
//! by `offline`. The glyphs (arrows, link, person, per-interface icon) are drawn
//! primitives, not font
//! characters — the icon mapping is one `match`, the single place to enrich.
//!
//! Portrait puts the cards down toward the unit's buttons; once more than a
//! couple of interfaces exist, the non-RST button scrolls the stack (TODO).

use core::fmt::Write as _;

use embedded_graphics::mono_font::ascii::{FONT_5X8, FONT_6X10, FONT_9X15_BOLD};
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Line, PrimitiveStyle, Rectangle};
use embedded_graphics::text::{Baseline, Text};
use heapless::String as HString;

const WIDTH: i32 = 64;
const TITLE_H: i32 = 26;
const CARD_TOP: i32 = 27;
const CARD_H: i32 = 31;
const CARD_GAP: i32 = 2;
/// Cards that fit below the title bar on a 128px-tall portrait panel.
const MAX_VISIBLE_CARDS: usize = 3;

/// What interface a card represents — the single source for its icon. Add a
/// variant (and its `match` arm in [`draw_interface_icon`]) as new interface
/// kinds land; never a wildcard, so the compiler flags the missing glyph.
#[derive(Clone, Copy)]
pub enum CardKind {
    Wifi,
    Usb,
    LoRa,
    EspNow,
}

/// One interface's card. The host fills the static bits (kind, label) and the
/// live numbers from the runtime snapshot's per-interface view.
pub struct Card {
    pub kind: CardKind,
    pub label: &'static str,
    /// Invert the name/icon row for selection or active focus.
    pub selected: bool,
    pub online: bool,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    /// Link sessions active on this interface.
    pub links: u32,
    /// Routing-table destinations reachable via this interface.
    pub destinations: u32,
}

/// What the title-bar battery glyph shows: `Level` (filled segment bars to the
/// given percent) for a present battery, `Charging` (level plus an incoming plug
/// cue), or `Unknown` (a dash) when no plausible battery is detected. Boards
/// without a charge-status signal should keep reporting `Level`/`Unknown`.
#[derive(Clone, Copy)]
pub enum BatteryState {
    Level(u8),
    Charging(u8),
    Unknown,
}

/// 3 significant figures, rolling unit B -> K -> M -> G (1000-based), max 3
/// numeric chars: `1.0K` up to `10K` up to `100K`, then `1.0M`, and so on.
/// Integer-only (no float), max 4 chars including the unit.
fn fmt_bytes(n: u64) -> HString<8> {
    let mut s = HString::new();
    if n < 1000 {
        let _ = write!(s, "{n}B");
        return s;
    }
    let (unit, unit_val) = if n < 1_000_000 {
        ('K', 1_000u64)
    } else if n < 1_000_000_000 {
        ('M', 1_000_000)
    } else {
        ('G', 1_000_000_000)
    };
    // value-in-the-unit scaled by 1000 (thousandths of the unit): [1000, 999_999]
    let thousandths = n * 1000 / unit_val;
    let int_part = thousandths / 1000; // [1, 999]
    if int_part < 10 {
        // one decimal: 1.0 .. 9.9
        let tenths = thousandths / 100;
        let _ = write!(s, "{}.{}{}", tenths / 10, tenths % 10, unit);
    } else {
        // whole: 10 .. 999
        let _ = write!(s, "{int_part}{unit}");
    }
    s
}

fn fill(color: BinaryColor) -> PrimitiveStyle<BinaryColor> {
    PrimitiveStyle::with_fill(color)
}

fn stroke(color: BinaryColor) -> PrimitiveStyle<BinaryColor> {
    PrimitiveStyle::with_stroke(color, 1)
}

fn line<D: DrawTarget<Color = BinaryColor>>(display: &mut D, a: Point, b: Point) {
    line_colored(display, a, b, BinaryColor::On);
}

fn line_colored<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    a: Point,
    b: Point,
    color: BinaryColor,
) {
    let _ = Line::new(a, b).into_styled(stroke(color)).draw(display);
}

fn draw_pattern_colored<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    x: i32,
    y: i32,
    rows: &[&str],
    color: BinaryColor,
) {
    for (row_index, row) in rows.iter().enumerate() {
        for (col_index, pixel) in row.as_bytes().iter().enumerate() {
            if *pixel == b'#' {
                let _ = Rectangle::new(
                    Point::new(x + col_index as i32, y + row_index as i32),
                    Size::new(1, 1),
                )
                .into_styled(fill(color))
                .draw(display);
            }
        }
    }
}

/// A battery glyph drawn in the background color (it sits on the inverted title
/// bar): a 15x9 outline + left terminal nub, then either four filled segment
/// bars (to the nearest quarter) for a present battery, an incoming plug cue
/// for charging, or a dash for unknown. The bars are inset 1px from the outline
/// on each side for breathing room.
fn draw_battery<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    x: i32,
    y: i32,
    state: BatteryState,
) {
    let outline = stroke(BinaryColor::Off);
    let solid = fill(BinaryColor::Off);
    let _ = Rectangle::new(Point::new(x, y), Size::new(15, 9))
        .into_styled(outline)
        .draw(display);
    let _ = Rectangle::new(Point::new(x - 2, y + 3), Size::new(2, 3))
        .into_styled(solid)
        .draw(display);
    match state {
        BatteryState::Level(pct) | BatteryState::Charging(pct) => {
            // Four segments (2px bar + 1px gap) inset 1px inside the outline, so
            // they span x+2..x+12; filled to the nearest quarter — coarse by
            // design.
            let filled = (pct as u32 * 4 + 50) / 100;
            for i in 0..filled.min(4) {
                let bar_x = x + 2 + i as i32 * 3;
                let _ = Rectangle::new(Point::new(bar_x, y + 2), Size::new(2, 5))
                    .into_styled(solid)
                    .draw(display);
            }
            if matches!(state, BatteryState::Charging(_)) {
                let _ = Line::new(Point::new(x + 15, y + 4), Point::new(x + 18, y + 4))
                    .into_styled(outline)
                    .draw(display);
                let _ = Rectangle::new(Point::new(x + 19, y + 3), Size::new(2, 3))
                    .into_styled(solid)
                    .draw(display);
            }
        }
        BatteryState::Unknown => {
            let _ = Line::new(Point::new(x + 4, y + 4), Point::new(x + 10, y + 4))
                .into_styled(outline)
                .draw(display);
        }
    }
}

/// The two-line inverted title bar: a small left-aligned `Personal` with a
/// battery glyph on the right, over a big bold `Hopspot`, knocked out of a
/// filled bar.
fn draw_title_bar<D: DrawTarget<Color = BinaryColor>>(display: &mut D, battery: BatteryState) {
    let _ = Rectangle::new(Point::new(0, 0), Size::new(WIDTH as u32, TITLE_H as u32))
        .into_styled(fill(BinaryColor::On))
        .draw(display);
    // Line 1: small left "Personal" (8*5=40px) + battery on the right.
    let small = MonoTextStyle::new(&FONT_5X8, BinaryColor::Off);
    let _ = Text::with_baseline("Personal", Point::new(2, 1), small, Baseline::Top).draw(display);
    // x=45: the 2px nub starts at col 43 and the 15px outline ends at col 59,
    // leaving the right edge free for a future charging/plug indicator.
    draw_battery(display, 45, 1, battery);
    // Line 2: big bold "Hopspot" (7*9=63px, fills the width).
    let big = MonoTextStyle::new(&FONT_9X15_BOLD, BinaryColor::Off);
    let _ = Text::with_baseline("Hopspot", Point::new(1, 10), big, Baseline::Top).draw(display);
}

/// A thin up (`up`) or down arrow: a shortened 1px shaft with a small chevron
/// head, 5px wide and 7px tall, fitting a text row at `y`.
fn draw_arrow<D: DrawTarget<Color = BinaryColor>>(display: &mut D, x: i32, y: i32, up: bool) {
    let cx = x + 2;
    // Shaft: down arrows omit the top pixel to open the stacked-row gap.
    let shaft_start = if up { y } else { y + 1 };
    line(display, Point::new(cx, shaft_start), Point::new(cx, y + 5));
    // head: chevron at the leading end
    let (tip, wing) = if up { (y, y + 2) } else { (y + 6, y + 4) };
    line(display, Point::new(cx, tip), Point::new(x, wing));
    line(display, Point::new(cx, tip), Point::new(x + 4, wing));
}

/// A tiny head-and-shoulders outline, ~9x7px.
fn draw_person<D: DrawTarget<Color = BinaryColor>>(display: &mut D, x: i32, y: i32) {
    line(display, Point::new(x + 3, y), Point::new(x + 5, y));
    line(display, Point::new(x + 2, y + 1), Point::new(x + 2, y + 2));
    line(display, Point::new(x + 6, y + 1), Point::new(x + 6, y + 2));
    line(display, Point::new(x + 3, y + 3), Point::new(x + 5, y + 3));
    line(display, Point::new(x + 2, y + 4), Point::new(x + 1, y + 5));
    line(display, Point::new(x + 6, y + 4), Point::new(x + 7, y + 5));
}

/// A tiny two-loop chain outline, ~8x6px.
fn draw_link<D: DrawTarget<Color = BinaryColor>>(display: &mut D, x: i32, y: i32) {
    line(display, Point::new(x + 1, y), Point::new(x + 2, y));
    line(display, Point::new(x, y + 1), Point::new(x, y + 4));
    line(display, Point::new(x + 1, y + 5), Point::new(x + 2, y + 5));
    line(display, Point::new(x + 5, y), Point::new(x + 6, y));
    line(display, Point::new(x + 7, y + 1), Point::new(x + 7, y + 4));
    line(display, Point::new(x + 5, y + 5), Point::new(x + 6, y + 5));
    let _ = Rectangle::new(Point::new(x + 4, y + 2), Size::new(1, 1))
        .into_styled(fill(BinaryColor::On))
        .draw(display);
    let _ = Rectangle::new(Point::new(x + 3, y + 3), Size::new(1, 1))
        .into_styled(fill(BinaryColor::On))
        .draw(display);
}

fn draw_offline_icon<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    x: i32,
    y: i32,
    color: BinaryColor,
) {
    line_colored(
        display,
        Point::new(x + 1, y + 7),
        Point::new(x + 8, y),
        color,
    );
}

/// The per-interface icon — the one place that maps a [`CardKind`] to a glyph.
fn draw_interface_icon<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    x: i32,
    y: i32,
    kind: CardKind,
    color: BinaryColor,
) {
    match kind {
        // WiFi: the familiar status-bar arc stack, pixel-reduced to 9px.
        CardKind::Wifi => {
            draw_pattern_colored(
                display,
                x,
                y,
                &[
                    "  #####  ",
                    " #     # ",
                    "#       #",
                    "         ",
                    "   ###   ",
                    "  #   #  ",
                    "         ",
                    "    #    ",
                    "   ###   ",
                ],
                color,
            );
        }
        // USB: a connector "mouth" with a full-width plastic tongue + cable stub.
        CardKind::Usb => {
            line_colored(
                display,
                Point::new(x + 4, y),
                Point::new(x + 4, y + 2),
                color,
            );
            let _ = Rectangle::new(Point::new(x, y + 2), Size::new(9, 6))
                .into_styled(stroke(color))
                .draw(display);
            let _ = Line::new(Point::new(x + 1, y + 5), Point::new(x + 7, y + 5))
                .into_styled(stroke(color))
                .draw(display);
        }
        // LoRa: long-range radio, rendered as a mast with symmetric RF lobes.
        CardKind::LoRa => {
            draw_pattern_colored(
                display,
                x,
                y,
                &[
                    "#   #   #",
                    " #  #  # ",
                    "  # # #  ",
                    "   ###   ",
                    "    #    ",
                    "    #    ",
                    "    #    ",
                    "   ###   ",
                    "  #####  ",
                ],
                color,
            );
        }
        // ESP-NOW: an omni broadcast node — a center dot with a wave opening to
        // each side (distinct from WiFi's upward arcs and LoRa's antenna).
        CardKind::EspNow => {
            draw_pattern_colored(
                display,
                x,
                y,
                &[
                    "         ",
                    "#       #",
                    " #     # ",
                    "  # # #  ",
                    "   ###   ",
                    "  # # #  ",
                    " #     # ",
                    "#       #",
                    "         ",
                ],
                color,
            );
        }
    }
}

/// Draw one card: an outlined box with a name line (icon + label) and, beneath
/// it, traffic and peers. `top` is the box's top edge.
fn draw_card<D: DrawTarget<Color = BinaryColor>>(display: &mut D, top: i32, card: &Card) {
    let _ = Rectangle::new(Point::new(0, top), Size::new(WIDTH as u32, CARD_H as u32))
        .into_styled(stroke(BinaryColor::On))
        .draw(display);

    let name_color = if card.selected {
        BinaryColor::Off
    } else {
        BinaryColor::On
    };
    if card.selected {
        let _ = Rectangle::new(Point::new(1, top), Size::new((WIDTH - 2) as u32, 12))
            .into_styled(fill(BinaryColor::On))
            .draw(display);
        let _ = Line::new(Point::new(0, top), Point::new(WIDTH - 1, top))
            .into_styled(stroke(BinaryColor::Off))
            .draw(display);
        let _ = Line::new(Point::new(0, top), Point::new(0, top + 11))
            .into_styled(stroke(BinaryColor::Off))
            .draw(display);
        let _ = Line::new(Point::new(WIDTH - 1, top), Point::new(WIDTH - 1, top + 11))
            .into_styled(stroke(BinaryColor::Off))
            .draw(display);
        let _ = Line::new(
            Point::new(0, top + CARD_H - 1),
            Point::new(WIDTH - 1, top + CARD_H - 1),
        )
        .into_styled(stroke(BinaryColor::Off))
        .draw(display);
        let _ = Line::new(Point::new(0, top + 12), Point::new(0, top + CARD_H - 1))
            .into_styled(stroke(BinaryColor::Off))
            .draw(display);
        let _ = Line::new(
            Point::new(WIDTH - 1, top + 12),
            Point::new(WIDTH - 1, top + CARD_H - 1),
        )
        .into_styled(stroke(BinaryColor::Off))
        .draw(display);
    }

    let label_style = MonoTextStyle::new(&FONT_6X10, name_color);
    let num_style = MonoTextStyle::new(&FONT_5X8, BinaryColor::On);

    // Name line: [icon] label.
    if card.online {
        draw_interface_icon(display, 3, top + 2, card.kind, name_color);
    } else {
        draw_offline_icon(display, 3, top + 3, name_color);
    }
    let _ = Text::with_baseline(
        card.label,
        Point::new(14, top + 2),
        label_style,
        Baseline::Top,
    )
    .draw(display);

    // Traffic rows, or a whole-card offline state when disconnected.
    let tx_y = top + 13;
    let rx_y = top + 22;
    if !card.online {
        let _ = Text::with_baseline(
            "Offline",
            Point::new(16, top + 16),
            num_style,
            Baseline::Top,
        )
        .draw(display);
        return;
    }

    draw_arrow(display, 2, tx_y + 1, true);
    let _ = Text::with_baseline(
        &fmt_bytes(card.tx_bytes),
        Point::new(8, tx_y),
        num_style,
        Baseline::Top,
    )
    .draw(display);
    draw_arrow(display, 2, rx_y, false);
    let _ = Text::with_baseline(
        &fmt_bytes(card.rx_bytes),
        Point::new(8, rx_y),
        num_style,
        Baseline::Top,
    )
    .draw(display);

    // Link and destination counters sit in a compact right-side stats column.
    draw_link(display, 43, tx_y + 1);
    let mut links: HString<4> = HString::new();
    let _ = write!(links, "{}", card.links.min(99));
    let _ =
        Text::with_baseline(&links, Point::new(53, tx_y), num_style, Baseline::Top).draw(display);
    draw_person(display, 42, rx_y + 1);
    let mut destinations: HString<4> = HString::new();
    let _ = write!(destinations, "{}", card.destinations.min(99));
    let _ = Text::with_baseline(
        &destinations,
        Point::new(53, rx_y),
        num_style,
        Baseline::Top,
    )
    .draw(display);
}

/// Render the full screen: title bar + a card per interface (up to what fits).
/// Clears first; the caller flushes.
pub fn draw<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    cards: &[Card],
    battery: BatteryState,
) {
    let _ = display.clear(BinaryColor::Off);
    draw_title_bar(display, battery);
    for (i, card) in cards.iter().take(MAX_VISIBLE_CARDS).enumerate() {
        draw_card(display, CARD_TOP + i as i32 * (CARD_H + CARD_GAP), card);
    }
}

/// A boot/connecting splash: title bar + a centered status line.
pub fn splash<D: DrawTarget<Color = BinaryColor>>(display: &mut D, status: &str) {
    let _ = display.clear(BinaryColor::Off);
    draw_title_bar(display, BatteryState::Unknown);
    let style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let _ = Text::with_baseline(status, Point::new(2, CARD_TOP + 4), style, Baseline::Top)
        .draw(display);
}

#[cfg(test)]
mod tests {
    use embedded_graphics::mock_display::MockDisplay;

    use super::*;

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
    fn unknown_battery_dash_is_symmetric() {
        let mut display = MockDisplay::new();

        draw_battery(&mut display, 2, 0, BatteryState::Unknown);

        assert_eq!(display.get_pixel(Point::new(5, 4)), None);
        for x in 6..=12 {
            assert_eq!(display.get_pixel(Point::new(x, 4)), Some(BinaryColor::Off));
        }
        assert_eq!(display.get_pixel(Point::new(13, 4)), None);
    }

    #[test]
    fn charging_battery_draws_right_side_plug() {
        let mut display = MockDisplay::new();
        display.set_allow_overdraw(true);

        draw_battery(&mut display, 2, 0, BatteryState::Charging(100));

        for x in 17..=20 {
            assert_eq!(display.get_pixel(Point::new(x, 4)), Some(BinaryColor::Off));
        }
        assert_eq!(display.get_pixel(Point::new(21, 3)), Some(BinaryColor::Off));
        assert_eq!(display.get_pixel(Point::new(23, 4)), None);
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
            kind: CardKind::Usb,
            label: "USB",
            selected: false,
            online: true,
            tx_bytes: 123,
            rx_bytes: 456,
            links: 5,
            destinations: 7,
        };

        draw_card(&mut display, 0, &card);

        assert_eq!(display.get_pixel(Point::new(4, 14)), Some(BinaryColor::On));
        assert_eq!(display.get_pixel(Point::new(4, 20)), None);
        assert_eq!(display.get_pixel(Point::new(4, 22)), None);
        assert_eq!(display.get_pixel(Point::new(4, 23)), Some(BinaryColor::On));
        assert_eq!(display.get_pixel(Point::new(4, 28)), Some(BinaryColor::On));
        assert_eq!(display.get_pixel(Point::new(4, 29)), None);
        assert_eq!(display.get_pixel(Point::new(39, 14)), None);
        assert_eq!(display.get_pixel(Point::new(43, 14)), None);
        assert_eq!(display.get_pixel(Point::new(44, 14)), Some(BinaryColor::On));
        assert_eq!(display.get_pixel(Point::new(45, 23)), Some(BinaryColor::On));
    }

    #[test]
    fn offline_card_centers_status_and_hides_metrics() {
        let mut display = MockDisplay::new();
        display.set_allow_overdraw(true);
        let card = Card {
            kind: CardKind::EspNow,
            label: "ESP-NOW",
            selected: false,
            online: false,
            tx_bytes: 123,
            rx_bytes: 456,
            links: 5,
            destinations: 7,
        };

        draw_card(&mut display, 0, &card);

        assert_eq!(display.get_pixel(Point::new(18, 17)), Some(BinaryColor::On));
        assert_eq!(display.get_pixel(Point::new(3, 11)), None);
        assert_eq!(display.get_pixel(Point::new(4, 10)), Some(BinaryColor::On));
        assert_eq!(display.get_pixel(Point::new(3, 4)), None);
        assert_eq!(display.get_pixel(Point::new(4, 14)), None);
        assert_eq!(display.get_pixel(Point::new(44, 14)), None);
        assert_eq!(display.get_pixel(Point::new(45, 23)), None);
    }

    #[test]
    fn selected_card_inverts_name_row() {
        let mut display = MockDisplay::new();
        display.set_allow_overdraw(true);
        let card = Card {
            kind: CardKind::Wifi,
            label: "WiFi",
            selected: true,
            online: true,
            tx_bytes: 0,
            rx_bytes: 0,
            links: 0,
            destinations: 0,
        };

        draw_card(&mut display, 0, &card);

        assert_eq!(display.get_pixel(Point::new(0, 0)), Some(BinaryColor::Off));
        assert_eq!(display.get_pixel(Point::new(63, 0)), Some(BinaryColor::Off));
        assert_eq!(display.get_pixel(Point::new(0, 11)), Some(BinaryColor::Off));
        assert_eq!(
            display.get_pixel(Point::new(63, 11)),
            Some(BinaryColor::Off)
        );
        assert_eq!(display.get_pixel(Point::new(1, 1)), Some(BinaryColor::On));
        assert_eq!(display.get_pixel(Point::new(0, 12)), Some(BinaryColor::Off));
        assert_eq!(display.get_pixel(Point::new(0, 30)), Some(BinaryColor::Off));
        assert_eq!(
            display.get_pixel(Point::new(63, 30)),
            Some(BinaryColor::Off)
        );
        assert_eq!(
            display.get_pixel(Point::new(31, 30)),
            Some(BinaryColor::Off)
        );
        assert_eq!(display.get_pixel(Point::new(2, 2)), Some(BinaryColor::On));
        assert_eq!(display.get_pixel(Point::new(5, 2)), Some(BinaryColor::Off));
    }
}
