use embedded_graphics::mono_font::iso_8859_1::{FONT_5X8, FONT_9X15_BOLD};
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Line, Rectangle};
use embedded_graphics::text::{Baseline, Text};

use crate::screen::CardKind;
use crate::{ChargingState, ExternalPowerState, PowerSnapshot};

use super::layout::*;
use super::primitives::{draw_pattern_colored, fill, line, line_colored, stroke};

/// The battery glyph, drawn in the background color (it sits on the inverted title bar): a 15x9 outline + left terminal nub, then four filled segment bars to the nearest quarter, an incoming plug cue for charging, or a dash for unknown.
pub(in crate::screen) fn draw_battery<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    x: i32,
    y: i32,
    state: PowerSnapshot,
    charging_tier_visible: bool,
) {
    let outline = stroke(BinaryColor::Off);
    let solid = fill(BinaryColor::Off);
    let _ = Rectangle::new(Point::new(x, y), Size::new(15, 9))
        .into_styled(outline)
        .draw(display);
    let _ = Rectangle::new(Point::new(x - 2, y + 3), Size::new(2, 3))
        .into_styled(solid)
        .draw(display);
    match state.battery() {
        Some(pct)
            if matches!(state.external_power(), ExternalPowerState::Present { .. })
                && pct.get() >= 100 =>
        {
            draw_full_battery(display, x, y);
        }
        Some(pct)
            if matches!(
                state.external_power(),
                ExternalPowerState::Present {
                    charging: ChargingState::Charging
                }
            ) =>
        {
            let pct = pct.get();
            let filled = (pct as u32 * 4 / 100).min(3);
            for i in (4 - filled)..4 {
                draw_battery_segment(display, x, y, i);
            }
            if charging_tier_visible {
                draw_battery_segment(display, x, y, 3 - filled);
            }
        }
        Some(pct) => {
            // Segments fill to the nearest quarter, anchored at the RIGHT so the leftmost bar empties first as the cell drains (matching the panel's orientation).
            let pct = pct.get();
            let filled = ((pct as u32 * 4 + 50) / 100).min(4);
            for i in (4 - filled)..4 {
                draw_battery_segment(display, x, y, i);
            }
        }
        None => {
            let _ = Line::new(Point::new(x + 4, y + 4), Point::new(x + 10, y + 4))
                .into_styled(outline)
                .draw(display);
        }
    }
    if matches!(state.external_power(), ExternalPowerState::Present { .. })
        && state.battery().is_none_or(|percent| percent.get() < 100)
    {
        draw_charging_plug(display, x, y);
    }
}

fn draw_battery_segment<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    x: i32,
    y: i32,
    segment: u32,
) {
    let bar_x = x + 2 + segment as i32 * 3;
    let _ = Rectangle::new(Point::new(bar_x, y + 2), Size::new(2, 5))
        .into_styled(fill(BinaryColor::Off))
        .draw(display);
}

fn draw_full_battery<D: DrawTarget<Color = BinaryColor>>(display: &mut D, x: i32, y: i32) {
    let _ = Rectangle::new(Point::new(x, y), Size::new(15, 9))
        .into_styled(fill(BinaryColor::Off))
        .draw(display);
}

fn battery_charge_tier_visible(animation_ms: u64) -> bool {
    (animation_ms / BATTERY_CHARGE_BLINK_MS).is_multiple_of(2)
}

fn draw_charging_plug<D: DrawTarget<Color = BinaryColor>>(display: &mut D, x: i32, y: i32) {
    let outline = stroke(BinaryColor::Off);
    let solid = fill(BinaryColor::Off);
    let _ = Rectangle::new(Point::new(x + 15, y + 2), Size::new(4, 5))
        .into_styled(solid)
        .draw(display);
    let _ = Line::new(Point::new(x + 19, y + 3), Point::new(x + 14, y + 3))
        .into_styled(outline)
        .draw(display);
    let _ = Line::new(Point::new(x + 19, y + 5), Point::new(x + 14, y + 5))
        .into_styled(outline)
        .draw(display);
}

pub(super) fn draw_title_bar<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    battery: PowerSnapshot,
    animation_ms: u64,
) {
    let _ = Rectangle::new(Point::new(0, 0), Size::new(WIDTH as u32, TITLE_H as u32))
        .into_styled(fill(BinaryColor::On))
        .draw(display);
    let small = MonoTextStyle::new(&FONT_5X8, BinaryColor::Off);
    let _ = Text::with_baseline("Personal", Point::new(2, 1), small, Baseline::Top).draw(display);
    draw_battery(
        display,
        44,
        1,
        battery,
        battery_charge_tier_visible(animation_ms),
    );
    let big = MonoTextStyle::new(&FONT_9X15_BOLD, BinaryColor::Off);
    let _ = Text::with_baseline("Hopspot", Point::new(1, 10), big, Baseline::Top).draw(display);
}

pub(super) fn draw_arrow<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    x: i32,
    y: i32,
    up: bool,
) {
    let cx = x + 2;
    let shaft_start = if up { y } else { y + 1 };
    line(display, Point::new(cx, shaft_start), Point::new(cx, y + 5));
    let (tip, wing) = if up { (y, y + 2) } else { (y + 6, y + 4) };
    line(display, Point::new(cx, tip), Point::new(x, wing));
    line(display, Point::new(cx, tip), Point::new(x + 4, wing));
}

pub(in crate::screen) fn draw_person<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    x: i32,
    y: i32,
) {
    line(display, Point::new(x + 3, y), Point::new(x + 5, y));
    line(display, Point::new(x + 2, y + 1), Point::new(x + 2, y + 2));
    line(display, Point::new(x + 6, y + 1), Point::new(x + 6, y + 2));
    line(display, Point::new(x + 3, y + 3), Point::new(x + 5, y + 3));
    line(display, Point::new(x + 2, y + 4), Point::new(x + 1, y + 5));
    line(display, Point::new(x + 6, y + 4), Point::new(x + 7, y + 5));
}

pub(in crate::screen) fn draw_link<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    x: i32,
    y: i32,
) {
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

pub(in crate::screen) fn draw_clock<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    x: i32,
    y: i32,
) {
    draw_pattern_colored(
        display,
        x,
        y,
        &[
            "  ###  ", " #   # ", "#  #  #", "#  ## #", "#     #", " #   # ", "  ###  ",
        ],
        BinaryColor::On,
    );
}

pub(super) fn draw_offline_icon<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    x: i32,
    y: i32,
    color: BinaryColor,
) {
    line_colored(
        display,
        Point::new(x + 2, y + 6),
        Point::new(x + 8, y),
        color,
    );
}

pub(super) fn draw_menu_cursor<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    x: i32,
    y: i32,
    color: BinaryColor,
) {
    line_colored(
        display,
        Point::new(x, y + 2),
        Point::new(x + 3, y + 4),
        color,
    );
    line_colored(
        display,
        Point::new(x, y + 6),
        Point::new(x + 3, y + 4),
        color,
    );
}

pub(super) fn draw_global_icon<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    x: i32,
    y: i32,
    color: BinaryColor,
) {
    draw_pattern_colored(
        display,
        x,
        y,
        &[
            "#######  ",
            "         ",
            "  #####  ",
            "         ",
            "#######  ",
            "         ",
            "  #####  ",
            "         ",
            "#######  ",
        ],
        color,
    );
}

pub(in crate::screen) fn draw_interface_icon<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    x: i32,
    y: i32,
    kind: CardKind,
    color: BinaryColor,
) {
    match kind {
        CardKind::Wifi | CardKind::WifiStation | CardKind::WifiStationDisabled | CardKind::Peer => {
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
        CardKind::Ble => {
            draw_pattern_colored(
                display,
                x,
                y,
                &[
                    "    #    ",
                    "    ##   ",
                    "  # # #  ",
                    "   ###   ",
                    "    #    ",
                    "   ###   ",
                    "  # # #  ",
                    "    ##   ",
                    "    #    ",
                ],
                color,
            );
        }
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
        CardKind::SharedInstance | CardKind::Tcp => {
            draw_pattern_colored(
                display,
                x,
                y,
                &[
                    "         ",
                    "         ",
                    "      #  ",
                    " ####### ",
                    "      #  ",
                    "  #      ",
                    " ####### ",
                    "  #      ",
                    "         ",
                ],
                color,
            );
        }
    }
}
