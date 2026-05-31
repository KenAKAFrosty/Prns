//! The "Personal Hopspot" OLED status screen — portrait (64x128), SSD1306.
//!
//! A two-line inverted title bar (`Personal` over a **bold** `Hopspot`) above a
//! vertical stack of interface cards. Each card is a name line (icon + label)
//! with its data underneath: up/down Reticulum traffic (3 significant figures,
//! rolling B->K->M->G) and a person glyph with the count of destinations the
//! routing table tracks via that interface. An interface that's down shows a
//! slashed icon and its traffic line is replaced by `offline`. The glyphs
//! (arrows, person, per-interface icon) are drawn primitives, not font
//! characters — the icon mapping is one `match`, the single place to enrich.
//!
//! Portrait puts the cards down toward the unit's buttons; once more than a
//! couple of interfaces exist, the non-RST button scrolls the stack (TODO).

use core::fmt::Write as _;

use embedded_graphics::mono_font::ascii::{FONT_5X8, FONT_6X10, FONT_9X15_BOLD};
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Circle, Line, PrimitiveStyle, Rectangle, Triangle};
use embedded_graphics::text::{Baseline, Text};
use heapless::String as HString;

const WIDTH: i32 = 64;
const TITLE_H: i32 = 26;
const CARD_TOP: i32 = 28;
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
}

/// One interface's card. The host fills the static bits (kind, label) and the
/// live numbers from the runtime snapshot's per-interface view.
pub struct Card {
    pub kind: CardKind,
    pub label: &'static str,
    pub online: bool,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    /// Routing-table destinations reachable via this interface.
    pub destinations: u32,
}

/// What the title-bar battery glyph shows: `Level` (filled segment bars to the
/// given percent) for a present battery, or `Unknown` (a dash) when no plausible
/// battery is detected. No charging/bolt state — the V4 has no charge-status pin
/// and its ~4.10 V float is indistinguishable from a full pack draining.
#[derive(Clone, Copy)]
pub enum BatteryState {
    Level(u8),
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
    let _ = Line::new(a, b)
        .into_styled(stroke(BinaryColor::On))
        .draw(display);
}

/// A battery glyph drawn in the background color (it sits on the inverted title
/// bar): a 15x9 outline + terminal nub, then either four filled segment bars
/// (to the nearest quarter) for a present battery, or a dash for unknown. The
/// bars are inset 1px from the outline on each side for breathing room.
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
    let _ = Rectangle::new(Point::new(x + 15, y + 3), Size::new(2, 3))
        .into_styled(solid)
        .draw(display);
    match state {
        BatteryState::Level(pct) => {
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
        }
        BatteryState::Unknown => {
            let _ = Line::new(Point::new(x + 5, y + 4), Point::new(x + 10, y + 4))
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
    // x=46: the 15px outline + 2px nub ends at col 62, a 1px margin from the
    // 64px-wide panel edge, with ~4px clearance from "Personal" on the left.
    draw_battery(display, 46, 1, battery);
    // Line 2: big bold "Hopspot" (7*9=63px, fills the width).
    let big = MonoTextStyle::new(&FONT_9X15_BOLD, BinaryColor::Off);
    let _ = Text::with_baseline("Hopspot", Point::new(1, 10), big, Baseline::Top).draw(display);
}

/// A thin up (`up`) or down arrow: a 1px shaft with a small chevron head, 5px
/// wide and 7px tall, fitting a text row at `y`.
fn draw_arrow<D: DrawTarget<Color = BinaryColor>>(display: &mut D, x: i32, y: i32, up: bool) {
    let cx = x + 2;
    // shaft
    line(display, Point::new(cx, y), Point::new(cx, y + 6));
    // head: chevron at the leading end
    let (tip, wing) = if up { (y, y + 2) } else { (y + 6, y + 4) };
    line(display, Point::new(cx, tip), Point::new(x, wing));
    line(display, Point::new(cx, tip), Point::new(x + 4, wing));
}

/// A tiny person silhouette (head + torso), ~6px wide.
fn draw_person<D: DrawTarget<Color = BinaryColor>>(display: &mut D, x: i32, y: i32) {
    let _ = Circle::new(Point::new(x + 1, y), 3)
        .into_styled(fill(BinaryColor::On))
        .draw(display);
    let _ = Triangle::new(
        Point::new(x, y + 9),
        Point::new(x + 5, y + 9),
        Point::new(x + 2, y + 4),
    )
    .into_styled(fill(BinaryColor::On))
    .draw(display);
}

/// The per-interface icon — the one place that maps a [`CardKind`] to a glyph.
/// When `!online`, a slash is drawn through it (the "no signal" cue that pairs
/// with the occluded traffic line).
fn draw_interface_icon<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    x: i32,
    y: i32,
    kind: CardKind,
    online: bool,
) {
    match kind {
        // WiFi: a dot with two signal arcs above it.
        CardKind::Wifi => {
            let _ = Rectangle::new(Point::new(x + 3, y + 8), Size::new(2, 2))
                .into_styled(fill(BinaryColor::On))
                .draw(display);
            for chevron in [
                Triangle::new(
                    Point::new(x + 2, y + 6),
                    Point::new(x + 4, y + 4),
                    Point::new(x + 6, y + 6),
                ),
                Triangle::new(
                    Point::new(x, y + 4),
                    Point::new(x + 4, y + 1),
                    Point::new(x + 8, y + 4),
                ),
            ] {
                let _ = chevron.into_styled(stroke(BinaryColor::On)).draw(display);
            }
        }
        // USB: a connector "mouth" with the plastic tongue + a short cable stub.
        CardKind::Usb => {
            line(display, Point::new(x + 4, y), Point::new(x + 4, y + 2));
            let _ = Rectangle::new(Point::new(x + 1, y + 2), Size::new(7, 6))
                .into_styled(stroke(BinaryColor::On))
                .draw(display);
            let _ = Rectangle::new(Point::new(x + 2, y + 5), Size::new(4, 2))
                .into_styled(fill(BinaryColor::On))
                .draw(display);
        }
    }
    if !online {
        line(display, Point::new(x, y + 9), Point::new(x + 8, y));
    }
}

/// Draw one card: an outlined box with a name line (icon + label) and, beneath
/// it, traffic and peers. `top` is the box's top edge.
fn draw_card<D: DrawTarget<Color = BinaryColor>>(display: &mut D, top: i32, card: &Card) {
    let _ = Rectangle::new(Point::new(0, top), Size::new(WIDTH as u32, CARD_H as u32))
        .into_styled(stroke(BinaryColor::On))
        .draw(display);

    let label_style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let num_style = MonoTextStyle::new(&FONT_5X8, BinaryColor::On);

    // Name line: [icon] label.
    draw_interface_icon(display, 3, top + 2, card.kind, card.online);
    let _ = Text::with_baseline(
        card.label,
        Point::new(14, top + 2),
        label_style,
        Baseline::Top,
    )
    .draw(display);

    // Traffic line (or `offline`, occluding the metrics when down).
    let traffic_y = top + 13;
    if card.online {
        draw_arrow(display, 2, traffic_y + 1, true);
        let _ = Text::with_baseline(
            &fmt_bytes(card.tx_bytes),
            Point::new(8, traffic_y),
            num_style,
            Baseline::Top,
        )
        .draw(display);
        draw_arrow(display, 37, traffic_y + 1, false);
        let _ = Text::with_baseline(
            &fmt_bytes(card.rx_bytes),
            Point::new(43, traffic_y),
            num_style,
            Baseline::Top,
        )
        .draw(display);
    } else {
        let _ = Text::with_baseline(
            "offline",
            Point::new(8, traffic_y),
            num_style,
            Baseline::Top,
        )
        .draw(display);
    }

    // Destinations line: how many routing-table destinations ride this iface.
    draw_person(display, 3, top + 21);
    let mut destinations: HString<4> = HString::new();
    let _ = write!(destinations, "{}", card.destinations.min(99));
    let _ = Text::with_baseline(&destinations, Point::new(13, top + 22), num_style, Baseline::Top)
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
