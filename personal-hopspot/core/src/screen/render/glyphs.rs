use embedded_graphics::mono_font::iso_8859_1::{FONT_5X8, FONT_9X15_BOLD};
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Line, Rectangle};
use embedded_graphics::text::{Baseline, Text};

use crate::screen::CardKind;
use crate::{ExternalPowerState, PowerSnapshot};

use super::layout::*;
use super::primitives::{draw_pattern_colored, fill, line, line_colored, stroke};

const BATTERY_BODY_W: i32 = 15;
const BATTERY_BODY_H: u32 = 9;
const BATTERY_PLUG_W: i32 = 5;
const BATTERY_ZONE_W: i32 = BATTERY_BODY_W + BATTERY_PLUG_W;
const BATTERY_INNER_W: i32 = BATTERY_BODY_W - 2;

const TINY_DIGITS: [[&str; 5]; 10] = [
    ["###", "# #", "# #", "# #", "###"],
    [" # ", "## ", " # ", " # ", "###"],
    ["###", "  #", "###", "#  ", "###"],
    ["###", "  #", "###", "  #", "###"],
    ["# #", "# #", "###", "  #", "  #"],
    ["###", "#  ", "###", "  #", "###"],
    ["###", "#  ", "###", "# #", "###"],
    ["###", "  #", "  #", "  #", "  #"],
    ["###", "# #", "###", "# #", "###"],
    ["###", "# #", "###", "  #", "###"],
];

/// The battery readout on the inverted title bar. The original outlined silhouette stays intact,
/// but its four quantized bars are replaced by the exact level as one to three digits. The
/// enclosure makes `%` implicit; unknown remains `--`. External power restores the original
/// right-side plug as a steady, independent presence indicator.
pub(in crate::screen) fn draw_battery<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    x: i32,
    y: i32,
    state: PowerSnapshot,
) {
    let outline = stroke(BinaryColor::Off);
    let _ = Rectangle::new(
        Point::new(x, y),
        Size::new(BATTERY_BODY_W as u32, BATTERY_BODY_H),
    )
    .into_styled(outline)
    .draw(display);
    let solid = fill(BinaryColor::Off);
    let _ = Rectangle::new(Point::new(x - 2, y + 3), Size::new(2, 3))
        .into_styled(solid)
        .draw(display);
    draw_battery_level(display, x, y, state.battery().map(|percent| percent.get()));

    if matches!(state.external_power(), ExternalPowerState::Present { .. }) {
        draw_charging_plug(display, x, y);
    }
}

fn draw_battery_level<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    x: i32,
    y: i32,
    percent: Option<u8>,
) {
    let Some(percent) = percent else {
        let unknown = ["   ", "   ", "###", "   ", "   "];
        draw_pattern_colored(display, x + 4, y + 2, &unknown, BinaryColor::Off);
        draw_pattern_colored(display, x + 8, y + 2, &unknown, BinaryColor::Off);
        return;
    };

    let digits = if percent >= 100 {
        3
    } else if percent >= 10 {
        2
    } else {
        1
    };
    let text_w = digits * 3 + (digits - 1);
    let mut digit_x = x + 1 + (BATTERY_INNER_W - text_w) / 2;
    let mut divisor = match digits {
        3 => 100,
        2 => 10,
        _ => 1,
    };
    while divisor != 0 {
        let digit = (percent / divisor) % 10;
        draw_pattern_colored(
            display,
            digit_x,
            y + 2,
            &TINY_DIGITS[digit as usize],
            BinaryColor::Off,
        );
        digit_x += 4;
        divisor /= 10;
    }
}

fn draw_charging_plug<D: DrawTarget<Color = BinaryColor>>(display: &mut D, x: i32, y: i32) {
    let outline = stroke(BinaryColor::Off);
    let solid = fill(BinaryColor::Off);
    let _ = Rectangle::new(Point::new(x + BATTERY_BODY_W, y + 2), Size::new(4, 5))
        .into_styled(solid)
        .draw(display);
    let _ = Line::new(
        Point::new(x + BATTERY_BODY_W + 4, y + 3),
        Point::new(x + BATTERY_BODY_W - 1, y + 3),
    )
    .into_styled(outline)
    .draw(display);
    let _ = Line::new(
        Point::new(x + BATTERY_BODY_W + 4, y + 5),
        Point::new(x + BATTERY_BODY_W - 1, y + 5),
    )
    .into_styled(outline)
    .draw(display);
}

pub(super) fn draw_title_bar<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    battery: PowerSnapshot,
) {
    let _ = Rectangle::new(Point::new(0, 0), Size::new(WIDTH as u32, TITLE_H as u32))
        .into_styled(fill(BinaryColor::On))
        .draw(display);
    let small = MonoTextStyle::new(&FONT_5X8, BinaryColor::Off);
    let _ = Text::with_baseline("Personal", Point::new(2, 1), small, Baseline::Top).draw(display);
    draw_battery(display, WIDTH - BATTERY_ZONE_W, 1, battery);
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
